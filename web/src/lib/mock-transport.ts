import { HttpError, type ApiTransport, type RequestOptions } from './api';
import type { User } from './session';
import type { Entry, Folder } from './listing';
import type { UploadStatus } from './transfers';

const root: Folder = { id: 'root', parent_id: null, name: 'My files', kind: 'folder', etag: '"root-1"' };
const seed: Entry[] = [
  { id: 'folder-projects', parent_id: root.id, name: 'Projects', kind: 'folder', etag: '"folder-1"' },
  { id: 'file-welcome', parent_id: root.id, name: 'Welcome.txt', kind: 'file', size: 238, etag: '"file-1"' },
];

/** Deterministic in-memory contract fixture. It never persists credentials. */
export class MockTransport implements ApiTransport {
  private setupRequired = true;
  private user: User | null = null;
  private entries = [...seed];
  private deletedEntries: Entry[] = [];
  private sequence = 0;
  private uploads = new Map<string, UploadStatus>();
  private uploadNames = new Map<string, { parentId: string; name: string; replacement?: { entry_id: string; etag: string } }>();

  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const method = options.method ?? 'GET';
    const route = path.split('?')[0]!;
    if (options.signal?.aborted) throw new DOMException('Canceled', 'AbortError');
    if (route === '/setup' && method === 'GET') return { setupRequired: this.setupRequired, user: this.user, csrfToken: 'mock-csrf-token' } as T;
    if (route === '/auth/csrf' && method === 'GET') return { csrf_token: 'mock-csrf-token' } as T;
    if (route === '/mutation-tickets' && method === 'POST') return { mutation_id: `upload-${++this.sequence}`, ticket: `mock-ticket-${this.sequence}`, expires_at: 'mock-only' } as T;
    if (route === '/setup/admin' && method === 'POST') {
      this.setupRequired = false;
      return { id: 'mock-user', handle: 'owner', role: 'administrator' } as T;
    }
    if (route === '/auth/local' && method === 'POST') {
      const body = options.body as { handle?: string };
      this.user = { id: 'mock-user', handle: body.handle || 'owner', role: 'administrator' };
      return { account_id: this.user.id, handle: this.user.handle, role: this.user.role } as T;
    }
    if (route === '/auth/logout' && method === 'POST') { this.user = null; return undefined as T; }
    if (route === '/folders/children' && method === 'GET') return { root, items: [...this.entries.filter((entry) => entry.parent_id === root.id)], next_cursor: null, has_more: false } as T;
    if (route.startsWith('/folders/') && route.endsWith('/children') && method === 'GET') {
      const folderId = decodeURIComponent(route.split('/')[2]!);
      const items = this.entries.filter((entry) => entry.parent_id === folderId);
      return { items: [...items], next_cursor: null, has_more: false } as T;
    }
    if (route.endsWith('/children') && method === 'POST') {
      const body = options.body as { name: string; parent_id: string };
      const parentId = body.parent_id;
      const folder: Folder = { id: `folder-${++this.sequence}`, parent_id: parentId, name: body.name, kind: 'folder', etag: `"folder-${this.sequence + 1}"` };
      this.entries = [...this.entries, folder];
      return folder as T;
    }
    if (route === '/trash' && method === 'GET') return { items: this.deletedEntries.map((entry) => {
      const original_path: string[] = [];
      let parentId = entry.parent_id;
      while (parentId && parentId !== root.id) {
        const parent = [...this.entries, ...this.deletedEntries].find((item) => item.id === parentId);
        if (!parent) throw new Error('Missing parent');
        original_path.unshift(parent.name); parentId = parent.parent_id;
      }
      return { ...entry, original_path };
    }) } as T;
    if (route.startsWith('/trash/') && route.endsWith('/restore') && method === 'POST') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const item = this.deletedEntries.find((entry) => entry.id === id);
      if (!item || !this.entries.some((entry) => entry.id === item.parent_id) && item.parent_id !== root.id || this.entries.some((entry) => entry.parent_id === item.parent_id && entry.name === item.name)) throw new Error('Cannot restore');
      this.entries = [...this.entries, item]; this.deletedEntries = this.deletedEntries.filter((entry) => entry.id !== id); return undefined as T;
    }
    if (route.startsWith('/entries/') && route.endsWith('/move') && method === 'POST') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const body = options.body as { parent_id: string };
      const target = this.entries.find((entry) => entry.id === id);
      if (!target) throw new Error('Missing move item');
      this.entries = this.entries.map((entry) => entry.id === id ? { ...entry, parent_id: body.parent_id, etag: `"move-${++this.sequence}"` } as Entry : entry);
      return this.entries.find((entry) => entry.id === id) as T;
    }
    if (route.startsWith('/entries/') && method === 'DELETE') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const item = this.entries.find((entry) => entry.id === id);
      if (!item || this.entries.some((entry) => entry.parent_id === id)) throw new Error('Cannot delete');
      this.entries = this.entries.filter((entry) => entry.id !== id); this.deletedEntries = [...this.deletedEntries, item]; return undefined as T;
    }
    if (route.startsWith('/entries/') && method === 'PATCH') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const body = options.body as { name: string };
      this.entries = this.entries.map((entry) => entry.id === id ? { ...entry, name: body.name } as Entry : entry);
      return this.entries.find((entry) => entry.id === id) as T;
    }
    if (route === '/uploads' && method === 'POST') {
      const id = options.headers?.['X-Mutation-ID']?.replace('mock-ticket-', 'upload-');
      if (!id) throw new Error('Missing mock mutation ticket');
      const body = options.body as { length: number; name: string; parent_id: string; replacement?: { entry_id: string; etag: string } };
      const existing = this.entries.find((entry) => entry.parent_id === body.parent_id && entry.name.normalize('NFC') === body.name.normalize('NFC'));
      if (existing && !body.replacement) throw new HttpError(409, { code: 'name_collision', message: 'Name conflict', request_id: 'mock', existing });
      if (body.replacement && (!existing || existing.kind !== 'file' || existing.id !== body.replacement.entry_id || existing.etag !== body.replacement.etag)) throw new HttpError(412);
      this.uploads.set(id, { upload_id: id, mutation_id: id, state: 'admitted', status: 'admitted', bytes_received: 0, length: body.length, writer_active: false });
      this.uploadNames.set(id, { parentId: body.parent_id, name: body.name, replacement: body.replacement });
      return { id, status_url: `/uploads/${id}` } as T;
    }
    if (route.includes('/content') && method === 'PUT') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const previous = this.uploads.get(id)!;
      const bytes = options.body as Blob;
      const status: UploadStatus = { ...previous, state: 'publishing', status: 'staged', bytes_received: bytes.size };
      this.uploads.set(id, status);
      const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', await bytes.arrayBuffer()))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
      return { ...status, length: bytes.size, sha256, published: false } as T;
    }
    if (route.endsWith('/commit') && method === 'POST') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const previous = this.uploads.get(id)!;
      const details = this.uploadNames.get(id)!;
      const committed: UploadStatus = { ...previous, state: 'committed', status: 'committed' };
      if (details.replacement) {
        const existing = this.entries.find((entry) => entry.id === details.replacement?.entry_id);
        if (!existing || existing.etag !== details.replacement.etag) throw new HttpError(412);
        this.entries = this.entries.filter((entry) => entry.id !== existing.id);
        this.deletedEntries = [...this.deletedEntries, existing];
      }
      this.uploads.set(id, committed);
      this.entries = [...this.entries, { id: `file-${++this.sequence}`, parent_id: details.parentId, name: details.name, kind: 'file', size: previous.length, etag: '"file-new"' }];
      return { id, name: details.name, status: committed } as T;
    }
    if (route.startsWith('/uploads/') && method === 'DELETE') {
      const id = decodeURIComponent(route.split('/')[2]!);
      this.uploads.set(id, { ...this.uploads.get(id)!, state: 'rejected', status: 'canceled' }); return undefined as T;
    }
    if (route.startsWith('/uploads/') && method === 'GET') return this.uploads.get(decodeURIComponent(route.split('/')[2]!)) as T;
    if (route.startsWith('/entries/') && route.endsWith('/content') && method === 'GET') return new Blob(['Mock file content\n'], { type: 'application/octet-stream' }) as T;
    throw new Error(`Mock has no response for ${method} ${path}`);
  }
}

export const mockRoot = root;

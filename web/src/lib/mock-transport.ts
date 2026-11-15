import { defaultAppearance, type Appearance, type AppearanceUpdate } from './appearance';
import { HttpError, type ApiTransport, type RequestOptions } from './api';
import type { User, Account } from './session';
import type { Entry, Folder, ResultSort, SearchHit } from './listing';
import type { UploadStatus } from './transfers';

const root: Folder = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root-1"' };
const seed: Entry[] = [
  { id: 'folder-projects', parent_id: root.id, name: 'Projects', kind: 'folder', etag: '"folder-1"' },
  { id: 'file-welcome', parent_id: root.id, name: 'Welcome.txt', kind: 'file', size: 238, modified_at: 1735689600, etag: '"file-1"' },
];

const resultSorts = ['name_asc', 'name_desc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc', 'location_asc', 'location_desc'] as const;
const fold = (value: string) => value.normalize('NFC').toLowerCase().normalize('NFC');
const compareBytes = (a: string, b: string): number => {
  const left = new TextEncoder().encode(a), right = new TextEncoder().encode(b);
  for (let i = 0; i < Math.min(left.length, right.length); i++) if (left[i] !== right[i]) return left[i] - right[i];
  return left.length - right.length;
};
const resultSort = (params: URLSearchParams): ResultSort => {
  const sort = params.get('sort') ?? 'name_asc';
  if (!resultSorts.includes(sort as ResultSort)) throw new HttpError(400, { code: 'invalid_sort', message: 'Invalid sort', request_id: 'mock' });
  return sort as ResultSort;
};
const resultMetadata = (entry: Entry): Entry => ({
  ...entry,
  size: entry.kind === 'file' && Number.isFinite(entry.size) ? entry.size : null,
  modified_at: entry.kind === 'file' && typeof entry.modified_at === 'number' && Number.isFinite(entry.modified_at) ? entry.modified_at : null,
} as Entry);
const fingerprintOf = (value: unknown): number => {
  let fingerprint = 2166136261;
  for (const byte of new TextEncoder().encode(JSON.stringify(value))) fingerprint = Math.imul(fingerprint ^ byte, 16777619) >>> 0;
  return fingerprint;
};
const encodeResultCursor = (value: object): string => btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(value))));
const invalidResultCursor = () => new HttpError(400, { code: 'invalid_cursor', message: 'Invalid cursor', request_id: 'mock' });
const decodeResultCursor = (value: string): unknown => {
  try {
    if (value.length > 4096) throw new Error('Cursor too long');
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Uint8Array.from(atob(value), char => char.charCodeAt(0))));
  } catch { throw invalidResultCursor(); }
};

function compareResults(a: Entry, b: Entry, sort: ResultSort, leftPath: string[], rightPath: string[], search: boolean): number {
  const name = (entry: Entry) => search ? fold(entry.name) : entry.name;
  const location = (parts: string[]) => parts.map(part => search ? fold(part) : part.normalize('NFC')).join('\0');
  const names = () => compareBytes(name(a), name(b));
  const locations = () => compareBytes(location(leftPath), location(rightPath));
  let order: number;
  if (sort.startsWith('location_')) order = locations() * (sort === 'location_desc' ? -1 : 1) || names();
  else if (sort.startsWith('name_')) order = names() * (sort === 'name_desc' ? -1 : 1);
  else if (a.kind !== b.kind) order = a.kind === 'folder' ? -1 : 1;
  else if (a.kind === 'folder') order = names();
  else {
    const left = sort.startsWith('size_') ? a.size : a.modified_at;
    const right = sort.startsWith('size_') ? b.size : b.modified_at;
    const leftKnown = typeof left === 'number' && Number.isFinite(left);
    const rightKnown = typeof right === 'number' && Number.isFinite(right);
    order = leftKnown !== rightKnown ? (leftKnown ? -1 : 1) :
      (leftKnown && rightKnown ? (left - right) * (sort.endsWith('_desc') ? -1 : 1) : 0) || names();
  }
  return order || (search ? locations() : 0) || compareBytes(a.id, b.id);
}

/** Deterministic in-memory contract fixture. It never persists credentials. */
export class MockTransport implements ApiTransport {
  private setupRequired = true;
  private user: User | null = null;
  private instanceName = 'Sylux';
  private appearance: Appearance = { ...defaultAppearance };
  private accounts: Account[] = [{ account_id: 'mock-user', handle: 'owner', role: 'administrator', enabled: true }];
  private root = { ...root };
  private entries = seed.map((entry) => ({ ...entry }));
  private deletedEntries: Entry[] = [];
  private sequence = 0;
  private uploads = new Map<string, UploadStatus>();
  private committedUploads = new Map<string, { id: string; name: string; status: UploadStatus }>();
  private uploadNames = new Map<string, { parentId: string; parentEtag: string; name: string; replacement?: { entry_id: string; etag: string } }>();

  private folderPath(id: string): Folder[] {
    const items: Folder[] = [];
    const seen = new Set<string>();
    let current: string | null = id;
    while (current && items.length <= 128 && !seen.has(current)) {
      seen.add(current);
      const folder: Folder | undefined = current === this.root.id ? this.root : this.entries.find((entry): entry is Folder => entry.kind === 'folder' && entry.id === current);
      if (!folder) throw new HttpError(404);
      items.unshift(folder); current = folder.parent_id;
    }
    if (current || items[0]?.id !== this.root.id) throw new HttpError(404);
    return items;
  }

  private activeEntry(id: string): Entry {
    const entry = this.entries.find((entry) => entry.id === id);
    if (!entry?.parent_id) throw new HttpError(404);
    this.folderPath(entry.parent_id);
    return entry;
  }

  private bump(id: string): void {
    const etag = `"mock-${++this.sequence}"`;
    if (id === this.root.id) this.root = { ...this.root, etag };
    else this.entries = this.entries.map((entry) => entry.id === id ? { ...entry, etag } : entry);
  }

  private checkMatch(options: RequestOptions, entry: Entry): void {
    if (options.headers?.['If-Match'] !== entry.etag) throw new HttpError(412);
  }

  private uploadParent(parent: string, etag: string): void {
    if (this.folderPath(parent).at(-1)?.etag !== etag) throw new HttpError(412);
  }

  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const root = this.root;
    const method = options.method ?? 'GET';
    const route = path.split('?')[0]!;
    if (options.signal?.aborted) throw new DOMException('Canceled', 'AbortError');
    if (route === '/instance/appearance' && method === 'GET') return { ...this.appearance } as T;
    if (route === '/instance/appearance' && method === 'PUT') {
      if (this.user?.role !== 'administrator') throw new HttpError(403);
      const value = options.body as AppearanceUpdate;
      // Asset delivery needs the real server; this fixture models color-only changes.
      if ((value.logo && 'data' in value.logo) || (value.loginImage && 'data' in value.loginImage)) throw new HttpError(400);
      this.appearance = { ...this.appearance, accentColor: value.accentColor, loginBackgroundColor: value.loginBackgroundColor };
      return { ...this.appearance } as T;
    }
    if (route === '/instance' && method === 'GET') return { name: this.instanceName } as T;
    if (route === '/instance' && method === 'PATCH') {
      if (this.user?.role !== 'administrator') throw new HttpError(403);
      const name = (options.body as { name: string }).name.trim();
      if (!name || [...name].length > 40 || /[\u0000-\u001f\u007f]/.test(name)) throw new HttpError(400);
      this.instanceName = name;
      return { name } as T;
    }
    if (route === '/accounts' && method === 'GET') return [...this.accounts] as T;
    if (route === '/accounts' && method === 'POST') {
      const { handle } = options.body as { handle: string };
      if (this.accounts.some((account) => account.handle === handle)) throw new HttpError(409);
      this.accounts.push({ account_id: `account-${++this.sequence}`, handle, role: 'user', enabled: true });
      return undefined as T;
    }
    if (route.startsWith('/accounts/') && method === 'POST') {
      const account = this.accounts.find((item) => item.account_id === route.split('/')[2]);
      if (this.user?.role !== 'administrator') throw new HttpError(403);
      if (!account) throw new HttpError(404);
      if (account.account_id === this.user.id) throw new HttpError(409);
      if (route.endsWith('/enable')) {
        if (account.enabled) throw new HttpError(409);
        account.enabled = true;
      } else if (route.endsWith('/role')) {
        const { role } = options.body as { role: Account['role'] };
        if (role !== 'administrator' && role !== 'user') throw new HttpError(400);
        if (!account.enabled || role === account.role) throw new HttpError(409);
        account.role = role;
      } else if (route.endsWith('/disable')) {
        if (!account.enabled) throw new HttpError(409);
        account.enabled = false;
      } else if (!route.endsWith('/reset') || account.role !== 'user' || !account.enabled) throw new HttpError(404);
      return undefined as T;
    }
    if (route === '/me/password' && method === 'POST') { this.user = null; return undefined as T; }
    if (route === '/setup' && method === 'GET') return { setupRequired: this.setupRequired, user: this.user, csrfToken: 'mock-csrf-token' } as T;
    if (route === '/auth/csrf' && method === 'GET') return { csrf_token: 'mock-csrf-token' } as T;
    if (route === '/archives' && method === 'POST') {
      const ids = new Set((options.body as { entry_ids: string[] }).entry_ids);
      const notFound = () => new HttpError(404, { code: 'entry_not_found', message: 'entry_not_found', request_id: 'mock' });
      const selected = [...ids].map((id) => { try { return this.activeEntry(id); } catch { throw notFound(); } });
      const roots = selected.filter((entry) => !this.folderPath(entry.parent_id!).some((folder) => ids.has(folder.id)));
      if (new Set(roots.map((entry) => entry.name.normalize('NFC'))).size !== roots.length) throw new HttpError(409, { code: 'archive_name_conflict', message: 'archive_name_conflict', request_id: 'mock' });
      // Mock mode has no archive bytes; the link 404s like mock file downloads.
      return { url: `/archives/mock-${++this.sequence}`, name: roots.length === 1 ? `${roots[0]!.name}.zip` : 'download.zip', entry_count: roots.length } as T;
    }
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
    if (/^\/folders\/[^/]+\/path$/.test(route) && method === 'GET') {
      if (!this.user) throw new HttpError(401);
      return { items: this.folderPath(decodeURIComponent(route.split('/')[2]!)) } as T;
    }
    if (route === '/entries/search' && method === 'GET') {
      if (!this.user) throw new HttpError(401);
      const params = new URLSearchParams(path.split('?')[1]);
      const query = params.get('q') ?? '';
      const sort = resultSort(params);
      const limit = Number(params.get('limit') ?? '100');
      if (!query || new TextEncoder().encode(query).length > 255 || /[\u0000-\u001f\u007f]/.test(query)) throw new HttpError(400);
      if (!Number.isInteger(limit) || limit < 1 || limit > 100) throw new HttpError(400);
      const locationFor = (entry: Entry): Folder[] | null => {
        const path: Folder[] = [];
        let parentId: string | null = entry.parent_id;
        const seen = new Set<string>();
        while (parentId) {
          if (seen.has(parentId)) return null;
          seen.add(parentId);
          const parent: Folder | undefined = parentId === root.id ? root : this.entries.find((item): item is Folder => item.kind === 'folder' && item.id === parentId);
          if (!parent) return null;
          path.unshift(parent); parentId = parent.parent_id;
        }
        return path[0]?.id === root.id ? path : null;
      };
      const hits: SearchHit[] = this.entries.flatMap((entry) => {
        const location = locationFor(entry);
        return location && fold(entry.name).includes(fold(query)) ? [{ entry: resultMetadata(entry), location }] : [];
      }).sort((a, b) => compareResults(a.entry, b.entry, sort, a.location.slice(1).map(part => part.name), b.location.slice(1).map(part => part.name), true));
      const fingerprint = fingerprintOf([this.root, this.entries]);
      let offset = 0;
      if (params.has('cursor')) {
        let cursor: { account: string; query: string; sort: ResultSort; offset: number; fingerprint: number };
        let parsed: unknown;
        try { parsed = decodeResultCursor(params.get('cursor')!); } catch { throw invalidResultCursor(); }
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw invalidResultCursor();
        cursor = parsed as typeof cursor;
        if (cursor.account !== this.user.id || cursor.sort !== sort || cursor.query !== query || !Number.isInteger(cursor.offset) || cursor.offset < 1) throw invalidResultCursor();
        if (cursor.fingerprint !== fingerprint) throw new HttpError(409, { code: 'listing_changed', message: 'Search changed', request_id: 'mock' });
        if (cursor.offset >= hits.length) throw invalidResultCursor();
        offset = cursor.offset;
      }
      const end = Math.min(offset + limit, hits.length);
      return { items: hits.slice(offset, end), next_cursor: end < hits.length ? encodeResultCursor({ account: this.user.id, query, sort, offset: end, fingerprint }) : null, has_more: end < hits.length } as T;
    }
    if ((route === '/folders/children' || /^\/folders\/[^/]+\/children$/.test(route)) && method === 'GET') {
      const params = new URLSearchParams(path.split('?')[1]);
      const sort = params.get('sort') ?? 'name_asc';
      if (!['name_asc', 'name_desc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc'].includes(sort)) throw new HttpError(400);
      const folderId = route === '/folders/children' ? root.id : decodeURIComponent(route.split('/')[2]!);
      this.folderPath(folderId);
      const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
      const nameKey = (entry: Entry) => [...new TextEncoder().encode(entry.name.normalize('NFC'))];
      const items = this.entries.filter((entry) => entry.parent_id === folderId).sort((a, b) => {
        if (a.kind !== b.kind) return a.kind === 'folder' ? -1 : 1;
        const left = nameKey(a), right = nameKey(b);
        const difference = left.findIndex((byte, index) => byte !== right[index]);
        const name = difference < 0 ? left.length - right.length : left[difference] - (right[difference] ?? -1);
        if (a.kind === 'folder' || b.kind === 'folder') return name || compare(a.id, b.id);
        if (sort.startsWith('modified_')) {
          const leftTime = a.modified_at, rightTime = b.modified_at;
          const leftKnown = typeof leftTime === 'number' && Number.isFinite(leftTime);
          const rightKnown = typeof rightTime === 'number' && Number.isFinite(rightTime);
          if (leftKnown !== rightKnown) return leftKnown ? -1 : 1;
          return (leftKnown && rightKnown ? (leftTime - rightTime) * (sort === 'modified_desc' ? -1 : 1) : 0) || name || compare(a.id, b.id);
        }
        if (sort.startsWith('size_')) return Number(a.size === null) - Number(b.size === null) || ((a.size ?? 0) - (b.size ?? 0)) * (sort === 'size_desc' ? -1 : 1) || name || compare(a.id, b.id);
        return name * (sort === 'name_desc' ? -1 : 1) || compare(a.id, b.id);
      });
      const children = new Map<string, Entry[]>();
      for (const entry of this.entries) {
        if (!entry.parent_id) continue;
        let siblings = children.get(entry.parent_id);
        if (!siblings) { siblings = []; children.set(entry.parent_id, siblings); }
        siblings.push(entry);
      }
      const total = (folder: Folder): number | null => {
        let bytes = 0;
        const pending = [folder.id];
        const seen = new Set<string>();
        while (pending.length) {
          const id = pending.pop()!;
          if (seen.has(id)) throw new Error('Folder cycle');
          seen.add(id);
          for (const entry of children.get(id) ?? []) {
            if (entry.kind === 'folder') pending.push(entry.id);
            else if (typeof entry.size !== 'number' || !Number.isFinite(entry.size)) return null;
            else bytes += entry.size;
          }
        }
        return bytes;
      };
      return { ...(route === '/folders/children' ? { root } : {}), items: items.map(entry => entry.kind === 'folder' ? { ...entry, size: total(entry) } : entry), next_cursor: null, has_more: false } as T;
    }
    if (route.endsWith('/children') && method === 'POST') {
      const body = options.body as { name: string; parent_id: string };
      const parentId = body.parent_id;
      this.folderPath(parentId);
      if (this.entries.some((entry) => entry.parent_id === parentId && entry.name === body.name)) throw new HttpError(409);
      const folder: Folder = { id: `folder-${++this.sequence}`, parent_id: parentId, name: body.name, kind: 'folder', etag: `"folder-${this.sequence + 1}"` };
      this.entries = [...this.entries, folder]; this.bump(parentId);
      return folder as T;
    }
    if (route === '/trash' && method === 'GET') {
      if (!this.user) throw new HttpError(401);
      const params = new URLSearchParams(path.split('?')[1]);
      const sort = resultSort(params);
      const limit = Number(params.get('limit') ?? '100');
      if (!Number.isInteger(limit) || limit < 1 || limit > 200) throw new HttpError(400);
      const originalPath = (entry: Entry): string[] => {
        const parts: string[] = [];
        const seen = new Set<string>();
        let parentId = entry.parent_id;
        while (parentId && parentId !== root.id) {
          if (seen.has(parentId)) throw new Error('Folder cycle');
          seen.add(parentId);
          const parent = [...this.entries, ...this.deletedEntries].find(item => item.id === parentId);
          if (!parent) throw new Error('Missing parent');
          parts.unshift(parent.name); parentId = parent.parent_id;
        }
        return parts;
      };
      const sorted = [...this.deletedEntries].sort((a, b) => compareResults(a, b, sort,
        sort.startsWith('location_') ? originalPath(a) : [], sort.startsWith('location_') ? originalPath(b) : [], false));
      const fingerprint = fingerprintOf([this.root, this.entries, this.deletedEntries]);
      let offset = 0;
      if (params.has('cursor')) {
        let cursor: { account: string; sort: ResultSort; offset: number; fingerprint: number };
        try { cursor = decodeResultCursor(params.get('cursor')!) as typeof cursor; }
        catch { throw invalidResultCursor(); }
        if (!cursor || typeof cursor !== 'object' || Array.isArray(cursor) || cursor.account !== this.user.id || cursor.sort !== sort || !Number.isInteger(cursor.offset) || cursor.offset < 1) throw invalidResultCursor();
        if (cursor.fingerprint !== fingerprint) throw new HttpError(409, { code: 'listing_changed', message: 'Trash changed', request_id: 'mock' });
        if (cursor.offset >= sorted.length) throw invalidResultCursor();
        offset = cursor.offset;
      }
      const items = sorted.slice(offset, offset + limit).map(entry => {
        const original_path = originalPath(entry);
        const needs_reference = sorted.some(other => other.id !== entry.id && other.name === entry.name && JSON.stringify(originalPath(other)) === JSON.stringify(original_path));
        return { ...resultMetadata(entry), original_path, ...(needs_reference ? { needs_reference: true } : {}) };
      });
      const end = offset + items.length;
      const has_more = end < sorted.length;
      const next_cursor = has_more ? encodeResultCursor({ account: this.user.id, sort, offset: end, fingerprint }) : null;
      return { items, next_cursor, has_more } as T;
    }
    if (route.startsWith('/trash/') && route.endsWith('/restore') && method === 'POST') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const item = this.deletedEntries.find((entry) => entry.id === id);
      if (!item) throw new HttpError(404);
      this.checkMatch(options, item);
      try { this.folderPath(item.parent_id!); } catch { throw new HttpError(409); }
      if (this.entries.some((entry) => entry.parent_id === item.parent_id && entry.name === item.name)) throw new HttpError(409);
      this.entries = [...this.entries, { ...item, etag: `"mock-${++this.sequence}"` }];
      this.deletedEntries = this.deletedEntries.filter((entry) => entry.id !== id);
      this.bump(item.parent_id!); return undefined as T;
    }
    if (route.startsWith('/entries/') && route.endsWith('/move') && method === 'POST') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const body = options.body as { parent_id: string };
      const target = this.activeEntry(id);
      this.checkMatch(options, target);
      if (this.folderPath(body.parent_id).some((entry) => entry.id === id) || target.parent_id === body.parent_id) throw new HttpError(409);
      if (this.entries.some((entry) => entry.parent_id === body.parent_id && entry.name === target.name)) throw new HttpError(409);
      this.entries = this.entries.map((entry) => entry.id === id ? { ...entry, parent_id: body.parent_id, etag: `"mock-${++this.sequence}"` } as Entry : entry);
      this.bump(target.parent_id!); this.bump(body.parent_id);
      return this.entries.find((entry) => entry.id === id) as T;
    }
    if (route.startsWith('/entries/') && method === 'DELETE') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const item = this.activeEntry(id);
      this.checkMatch(options, item);
      const pending = [id];
      const tree = new Set<string>();
      while (pending.length) {
        const current = pending.pop()!;
        if (tree.has(current)) throw new Error('Folder cycle');
        tree.add(current);
        const entry = this.entries.find((entry) => entry.id === current)!;
        if (entry.kind === 'folder') pending.push(...this.entries.filter((entry) => entry.parent_id === current).map((entry) => entry.id));
      }
      for (const current of tree) this.bump(current);
      const deleted = this.entries.find((entry) => entry.id === id)!;
      this.entries = this.entries.filter((entry) => entry.id !== id); this.deletedEntries = [...this.deletedEntries, deleted];
      this.bump(item.parent_id!); return undefined as T;
    }
    if (route.startsWith('/entries/') && method === 'PATCH') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const body = options.body as { name: string };
      const item = this.activeEntry(id); this.checkMatch(options, item);
      if (this.entries.some((entry) => entry.id !== id && entry.parent_id === item.parent_id && entry.name === body.name)) throw new HttpError(409);
      this.entries = this.entries.map((entry) => entry.id === id ? { ...entry, name: body.name, etag: `"mock-${++this.sequence}"` } as Entry : entry);
      this.bump(item.parent_id!);
      return this.entries.find((entry) => entry.id === id) as T;
    }
    if (route === '/uploads' && method === 'POST') {
      const id = options.headers?.['X-Mutation-ID']?.replace('mock-ticket-', 'upload-');
      if (!id) throw new Error('Missing mock mutation ticket');
      if (this.uploads.has(id)) return { id, status_url: `/uploads/${id}` } as T;
      const body = options.body as { length: number; name: string; parent_id: string; parent_etag: string; replacement?: { entry_id: string; etag: string } };
      this.uploadParent(body.parent_id, body.parent_etag);
      const existing = this.entries.find((entry) => entry.parent_id === body.parent_id && entry.name.normalize('NFC') === body.name.normalize('NFC'));
      if (existing && !body.replacement) throw new HttpError(409, { code: 'name_collision', message: 'Name conflict', request_id: 'mock', existing });
      if (body.replacement && (!existing || existing.kind !== 'file' || existing.id !== body.replacement.entry_id || existing.etag !== body.replacement.etag)) throw new HttpError(412);
      this.uploads.set(id, { upload_id: id, mutation_id: id, state: 'admitted', status: 'admitted', bytes_received: 0, length: body.length, writer_active: false });
      this.uploadNames.set(id, { parentId: body.parent_id, parentEtag: body.parent_etag, name: body.name, replacement: body.replacement });
      return { id, status_url: `/uploads/${id}` } as T;
    }
    if (route.includes('/content') && method === 'PUT') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const details = this.uploadNames.get(id)!;
      this.uploadParent(details.parentId, details.parentEtag);
      const previous = this.uploads.get(id)!;
      const bytes = options.body as Blob;
      const status: UploadStatus = { ...previous, state: 'publishing', status: 'staged', bytes_received: bytes.size };
      this.uploads.set(id, status);
      const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', await bytes.arrayBuffer()))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
      return { ...status, length: bytes.size, sha256, published: false } as T;
    }
    if (route.endsWith('/commit') && method === 'POST') {
      const id = decodeURIComponent(route.split('/')[2]!);
      const stored = this.committedUploads.get(id);
      if (stored) return stored as T;
      const previous = this.uploads.get(id)!;
      const details = this.uploadNames.get(id)!;
      this.uploadParent(details.parentId, details.parentEtag);
      const committed: UploadStatus = { ...previous, state: 'committed', status: 'committed' };
      if (details.replacement) {
        const existing = this.entries.find((entry) => entry.id === details.replacement?.entry_id);
        if (!existing || existing.etag !== details.replacement.etag) throw new HttpError(412);
        this.entries = this.entries.filter((entry) => entry.id !== existing.id);
        this.deletedEntries = [...this.deletedEntries, existing];
      }
      this.uploads.set(id, committed);
      this.entries = [...this.entries, { id: `file-${++this.sequence}`, parent_id: details.parentId, name: details.name, kind: 'file', size: previous.length, modified_at: Math.floor(Date.now() / 1000), etag: '"file-new"' }];
      this.bump(details.parentId);
      const result = { id, name: details.name, status: committed };
      this.committedUploads.set(id, result);
      return result as T;
    }
    if (route.startsWith('/uploads/') && method === 'DELETE') {
      const id = decodeURIComponent(route.split('/')[2]!);
      this.uploads.set(id, { ...this.uploads.get(id)!, state: 'rejected', status: 'canceled' }); return undefined as T;
    }
    if (route.startsWith('/uploads/') && method === 'GET') return this.uploads.get(decodeURIComponent(route.split('/')[2]!)) as T;
    if (route.startsWith('/entries/') && route.endsWith('/content') && method === 'GET') { this.activeEntry(decodeURIComponent(route.split('/')[2]!)); return new Blob(['Mock file content\n'], { type: 'application/octet-stream' }) as T; }
    throw new Error(`Mock has no response for ${method} ${path}`);
  }
}

export const mockRoot = root;

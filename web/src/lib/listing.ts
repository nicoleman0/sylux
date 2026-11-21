import type { ApiClient } from './api';

export type Folder = { id: string; parent_id: string | null; name: string; kind: 'folder'; size?: number | null; modified_at?: number | null; etag: string };
export type FileEntry = { id: string; parent_id: string; name: string; kind: 'file'; size: number | null; modified_at?: number | null; etag: string };
export type Entry = Folder | FileEntry;
export type FolderSort = 'name_asc' | 'name_desc' | 'size_asc' | 'size_desc' | 'modified_asc' | 'modified_desc';
export type ResultSort = FolderSort | 'location_asc' | 'location_desc';
export type DeletedEntry = Entry & { original_path: string[]; needs_reference?: boolean };
export type RemovalSelection = { kind: 'all' } | { kind: 'selected'; items: { id: string; etag: string }[] };
export type RemovalPreview = { confirmation: string; count: number };
export type RemovalResult = { operation_id: string; state: 'committed'; count: number; cleanup: 'pending' | 'complete' };
export type RemovalStatus = RemovalResult | { operation_id: string; state: 'unknown' };
export type TrashPage = { items: DeletedEntry[]; next_cursor: string | null; has_more: boolean };

export function compareDeleted(left: DeletedEntry, right: DeletedEntry, sort: ResultSort = 'name_asc'): number {
  const compare = (a: string, b: string) => {
    const x = new TextEncoder().encode(a), y = new TextEncoder().encode(b);
    for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] - y[i];
    return x.length - y.length;
  };
  const name = compare(left.name, right.name);
  const tie = () => name || compare(left.id, right.id);
  if (sort.startsWith('name_')) return name * (sort === 'name_desc' ? -1 : 1) || compare(left.id, right.id);
  if (sort.startsWith('location_')) {
    const path = (item: DeletedEntry) => item.original_path.map(part => part.normalize('NFC')).join('\0');
    return compare(path(left), path(right)) * (sort === 'location_desc' ? -1 : 1) || tie();
  }
  if (left.kind !== right.kind) return left.kind === 'folder' ? -1 : 1;
  if (left.kind === 'folder') return tie();
  const value = (item: DeletedEntry) => sort.startsWith('size_') ? item.size : item.modified_at;
  const a = value(left), b = value(right);
  const knownA = typeof a === 'number', knownB = typeof b === 'number';
  if (knownA !== knownB) return knownA ? -1 : 1;
  return (knownA && knownB ? (a - b) * (sort.endsWith('_desc') ? -1 : 1) : 0) || tie();
}
export type Listing<T extends Entry = Entry> = { items: T[]; next_cursor: string | null; has_more: boolean };
export type RootListing = { items: Entry[]; root: Folder; next_cursor: string | null; has_more: boolean };
export type SearchHit = { entry: Entry; location: Folder[] };
export type SearchListing = { items: SearchHit[]; next_cursor: string | null; has_more: boolean };
export type ArchiveTicket = { url: string; name: string; entry_count: number };

export class ListingApi {
  constructor(private readonly client: ApiClient) {}

  async roots(cursor?: string | null, sort: FolderSort = 'name_asc', signal?: AbortSignal): Promise<RootListing> {
    const roots = await this.client.request<RootListing>(`/folders/children?limit=100&sort=${sort}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`, { signal });
    if (!roots.root) throw new Error('Backend did not return the private root descriptor');
    return roots;
  }
  children(folderId: string, cursor?: string | null, sort: FolderSort = 'name_asc', signal?: AbortSignal): Promise<Listing> {
    return this.client.request(`/folders/${encodeURIComponent(folderId)}/children?limit=100&sort=${sort}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`, { signal });
  }
  path(folderId: string, signal?: AbortSignal): Promise<{ items: Folder[] }> {
    return this.client.request(`/folders/${encodeURIComponent(folderId)}/path`, { signal });
  }
  search(query: string, folderId: string | null, cursor?: string | null, signal?: AbortSignal, sort: ResultSort = 'name_asc'): Promise<SearchListing> {
    const params = new URLSearchParams({ q: query, limit: '100', sort });
    if (folderId !== null) params.set('folder_id', folderId);
    if (cursor) params.set('cursor', cursor);
    return this.client.request(`/entries/search?${params}`, { signal });
  }
  async createFolder(parentId: string, name: string, _etag: string): Promise<Folder> {
    return this.client.request('/folders/children', {
      method: 'POST', body: { parent_id: parentId, name },
    });
  }
  async rename(entryId: string, name: string, etag: string): Promise<Entry> {
    return this.client.request(`/entries/${encodeURIComponent(entryId)}`, {
      method: 'PATCH', headers: { 'If-Match': etag }, body: { name },
    });
  }
  trash(entryId: string, etag: string): Promise<void> {
    return this.client.request(`/entries/${encodeURIComponent(entryId)}`, { method: 'DELETE', headers: { 'If-Match': etag } });
  }
  move(entryId: string, parentId: string, etag: string): Promise<Entry> {
    return this.client.request(`/entries/${encodeURIComponent(entryId)}/move`, {
      method: 'POST', headers: { 'If-Match': etag }, body: { parent_id: parentId },
    });
  }
  async prepareCopy(entryId: string, parentId: string, etag: string): Promise<string> {
    const result = await this.client.request<{ ticket: string }>(`/entries/${encodeURIComponent(entryId)}/copy-ticket`, {
      method: 'POST', headers: { 'If-Match': etag }, body: { parent_id: parentId },
    });
    if (typeof result?.ticket !== 'string' || !result.ticket || result.ticket.length > 1024) throw new Error('Invalid copy ticket');
    return result.ticket;
  }
  async copyStatus(ticket: string, checkOnly: boolean): Promise<'committed' | 'unknown'> {
    const result = await this.client.request<{ state: string }>(checkOnly ? '/copies/status' : '/copies', {
      method: 'POST', body: { ticket },
    });
    if (result?.state !== 'committed' && result?.state !== 'unknown') throw new Error('Invalid copy result');
    return result.state;
  }
  async deleted(cursor?: string | null, sort: ResultSort = 'name_asc', signal?: AbortSignal): Promise<TrashPage> {
    const page = await this.client.request<TrashPage>(`/trash?limit=100&sort=${sort}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`, { signal });
    if (!Array.isArray(page.items) || page.items.length > 100 || typeof page.has_more !== 'boolean' ||
      (page.has_more ? !page.items.length || typeof page.next_cursor !== 'string' || !page.next_cursor || page.next_cursor === cursor : page.next_cursor !== null)) {
      throw new Error('Invalid Trash pagination response');
    }
    const numeric = (value: unknown, size: boolean) => value === undefined || value === null ||
      (typeof value === 'number' && Number.isSafeInteger(value) && (size ? value >= 0 : Math.abs(value) <= 8_640_000_000_000));
    const text = (value: unknown): value is string => typeof value === 'string' && value.length > 0;
    for (const item of page.items) {
      if (!item || !text(item.id) || !text(item.name) || !text(item.etag) || !text(item.parent_id) ||
        !['file', 'folder'].includes(item.kind) || !Array.isArray(item.original_path) || !item.original_path.every(text) ||
        (item.needs_reference !== undefined && typeof item.needs_reference !== 'boolean') ||
        !numeric(item.size, true) || !numeric(item.modified_at, false)) throw new Error('Invalid Trash entry response');
    }
    if (new Set(page.items.map(item => item.id)).size !== page.items.length) throw new Error('Trash pagination repeated an item');
    for (let i = 1; i < page.items.length; i++) if (compareDeleted(page.items[i - 1], page.items[i], sort) >= 0) throw new Error('Trash pagination did not advance');
    return page;
  }
  async deletedTargets(ids: Iterable<string>, isCurrent: () => boolean): Promise<DeletedEntry[]> {
    const requested = new Set(ids);
    if (!requested.size) return [];
    const found = new Map<string, DeletedEntry>();
    const cursors = new Set<string>();
    let cursor: string | null = null;
    let previous: DeletedEntry | undefined;
    do {
      if (!isCurrent()) throw new Error('Trash inspection session changed');
      const page = await this.deleted(cursor);
      if (!isCurrent()) throw new Error('Trash inspection session changed');
      for (const item of page.items) {
        if (previous && compareDeleted(previous, item) >= 0) throw new Error('Trash pagination did not advance');
        previous = item;
        if (requested.has(item.id)) found.set(item.id, item);
      }
      cursor = page.next_cursor;
      if (cursor && cursors.has(cursor)) throw new Error('Trash pagination repeated a cursor');
      if (cursor) cursors.add(cursor);
      if (found.size === requested.size) return [...found.values()];
    } while (cursor);
    return [...found.values()];
  }
  async removalPreview(selection: RemovalSelection): Promise<RemovalPreview> {
    const result = await this.client.request<RemovalPreview>('/trash/removals/preview', { method: 'POST', body: { selection } });
    if (!result || typeof result.confirmation !== 'string' || !result.confirmation || !Number.isSafeInteger(result.count) || result.count <= 0) throw new Error('Invalid removal preview');
    return result;
  }
  private validateRemoval(result: RemovalStatus, operationId: string): RemovalStatus {
    if (!result || result.operation_id !== operationId || (result.state !== 'unknown' && (result.state !== 'committed' || !Number.isSafeInteger(result.count) || result.count <= 0 || !['pending', 'complete'].includes(result.cleanup)))) throw new Error('Invalid removal result');
    return result;
  }
  async removePermanently(operationId: string, confirmation: string): Promise<RemovalStatus> {
    const result = this.validateRemoval(await this.client.request('/trash/removals', { method: 'POST', body: { operation_id: operationId, confirmation } }), operationId);
    return result;
  }
  async removalStatus(operationId: string): Promise<RemovalStatus> {
    return this.validateRemoval(await this.client.request(`/trash/removals/${encodeURIComponent(operationId)}`), operationId);
  }
  restore(entryId: string, etag: string): Promise<void> {
    return this.client.request(`/trash/${encodeURIComponent(entryId)}/restore`, { method: 'POST', headers: { 'If-Match': etag } });
  }
  /** Checks a selection and returns a single-use link that streams it as one ZIP. */
  archive(entryIds: string[]): Promise<ArchiveTicket> {
    return this.client.request('/archives', { method: 'POST', body: { entry_ids: entryIds } });
  }
  downloadUrl(entryId: string): string {
    return `/entries/${encodeURIComponent(entryId)}/content`;
  }
}

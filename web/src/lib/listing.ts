import type { ApiClient } from './api';

export type Folder = { id: string; parent_id: string | null; name: string; kind: 'folder'; etag: string };
export type FileEntry = { id: string; parent_id: string; name: string; kind: 'file'; size: number; etag: string };
export type Entry = Folder | FileEntry;
export type DeletedEntry = Entry & { original_path: string[] };
export type Listing<T extends Entry = Entry> = { items: T[]; next_cursor: string | null; has_more: boolean };
export type RootListing = { items: Entry[]; root: Folder; next_cursor: string | null; has_more: boolean };

export class ListingApi {
  constructor(private readonly client: ApiClient) {}

  async roots(cursor?: string | null): Promise<RootListing> {
    const roots = await this.client.request<RootListing>(`/folders/children?limit=100${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`);
    if (!roots.root) throw new Error('Backend did not return the private root descriptor');
    return roots;
  }
  children(folderId: string, cursor?: string | null): Promise<Listing> {
    return this.client.request(`/folders/${encodeURIComponent(folderId)}/children?limit=100${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`);
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
  deleted(): Promise<{ items: DeletedEntry[] }> { return this.client.request('/trash'); }
  restore(entryId: string, etag: string): Promise<void> {
    return this.client.request(`/trash/${encodeURIComponent(entryId)}/restore`, { method: 'POST', headers: { 'If-Match': etag } });
  }
  downloadUrl(entryId: string): string {
    return `/entries/${encodeURIComponent(entryId)}/content`;
  }
}

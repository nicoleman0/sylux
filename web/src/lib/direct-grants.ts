import { HttpError, type ApiClient } from './api';
import type { FileEntry, Folder, Listing, SearchListing } from './listing';

export type SharedRoot = { entry: Folder; owner: string };
export type DirectGrant = { id: string; handle: string };
export type SharingConfirmation = { token: string; new_recipients: string[]; lost_recipients: string[]; carried_recipients: string[] };

export function sharingConfirmation(error: unknown): SharingConfirmation | undefined {
  if (!(error instanceof HttpError) || error.code !== 'sharing_confirmation_required') return;
  const value = error.details?.confirmation;
  if (!value || !/^[A-Za-z0-9_-]{43}$/.test(value.token) ||
      ![value.new_recipients, value.lost_recipients, value.carried_recipients]
        .every(group => Array.isArray(group) && group.every(handle => typeof handle === 'string'))) return;
  return value;
}

export class DirectSharingApi {
  constructor(private readonly client: ApiClient) {}

  incoming(signal: AbortSignal): Promise<{ items: SharedRoot[] }> {
    return this.client.request('/shared', { signal });
  }

  path(id: string, signal: AbortSignal): Promise<{ items: Folder[] }> {
    return this.client.request(`/shared/folders/${encodeURIComponent(id)}/path`, { signal });
  }

  children(id: string, cursor: string | null, signal: AbortSignal): Promise<Listing> {
    const params = new URLSearchParams({ limit: '100' });
    if (cursor) params.set('cursor', cursor);
    return this.client.request(`/shared/folders/${encodeURIComponent(id)}/children?${params}`, { signal });
  }

  search(query: string, folder: string, cursor: string | null, signal: AbortSignal): Promise<SearchListing> {
    const params = new URLSearchParams({ q: query, limit: '100' });
    if (folder) params.set('folder_id', folder);
    if (cursor) params.set('cursor', cursor);
    return this.client.request(`/shared/search?${params}`, { signal });
  }

  file(id: string, signal: AbortSignal): Promise<{ entry: FileEntry; location: Folder[] }> {
    return this.client.request(`/shared/entries/${encodeURIComponent(id)}`, { signal });
  }

  content(id: string): string {
    return `/shared/entries/${encodeURIComponent(id)}/content`;
  }

  grants(id: string, signal: AbortSignal): Promise<{ items: DirectGrant[] }> {
    return this.client.request(`/entries/${encodeURIComponent(id)}/direct-grants`, { signal });
  }

  grant(id: string, handle: string, signal: AbortSignal): Promise<DirectGrant> {
    return this.client.request(`/entries/${encodeURIComponent(id)}/direct-grants`, { method: 'POST', body: { handle }, signal });
  }

  revoke(id: string, signal: AbortSignal): Promise<void> {
    return this.client.request(`/direct-grants/${encodeURIComponent(id)}`, { method: 'DELETE', signal });
  }
}

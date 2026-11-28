import type { ApiClient } from './api';

export type PublicLink = { kind: 'file' | 'folder'; id: string; entry_id: string; name: string; created_at: number; expires_at: number };

export async function publicFolderLinkStatus(client: ApiClient, folder: string, signal?: AbortSignal): Promise<boolean> {
  const result = await client.request<{ public_link_active?: boolean }>(`/folders/${encodeURIComponent(folder)}/path?public_links=true`, { signal });
  return result.public_link_active === true;
}

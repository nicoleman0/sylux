import type { ApiClient } from './api';
import { publicFolderLinkStatus } from './public-folder-links';

export type PublicFolderNoticeState = 'idle' | 'checking' | 'private' | 'public' | 'unavailable';
type CheckedStatus = 'private' | 'public' | 'unavailable';

export function publicFolderNotice(client: () => ApiClient, destination: () => string | null) {
  const folder = $derived(destination());
  let checked = $state<{ folder: string; status: CheckedStatus } | null>(null);

  $effect(() => {
    checked = null;
    if (!folder) return;
    const target = folder;
    const controller = new AbortController();
    function finish(status: CheckedStatus) {
      if (!controller.signal.aborted) checked = { folder: target, status };
    }
    void publicFolderLinkStatus(client(), target, controller.signal).then(
      active => finish(active ? 'public' : 'private'),
      () => finish('unavailable'),
    );
    return () => controller.abort();
  });

  return {
    get status(): PublicFolderNoticeState {
      if (!folder) return 'idle';
      return checked?.folder === folder ? checked.status : 'checking';
    },
    get pending() { return this.status === 'checking'; },
  };
}

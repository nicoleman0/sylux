<script lang="ts">
  import type { ApiClient } from './api';
  import { publicFolderLinkStatus } from './public-folder-links';

  let { client, folder, subject }: { client: ApiClient; folder: string | null; subject: string } = $props();
  let active = $state<boolean | null>(null);

  $effect(() => {
    active = null;
    if (!folder) return;
    const controller = new AbortController();
    void publicFolderLinkStatus(client, folder, controller.signal).then(value => {
      if (!controller.signal.aborted) active = value;
    }, () => {
      if (!controller.signal.aborted) active = null;
    });
    return () => controller.abort();
  });
</script>

{#if folder && active !== false}
  <p class="text-sm text-muted-foreground">
    {subject} will be available to anyone with {active ? 'a public link to this location' : 'a public folder link, if one is active here'}.
  </p>
{/if}

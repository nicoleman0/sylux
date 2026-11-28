<script lang="ts">
  import type { PublicFolderNoticeState } from './public-folder-notice.svelte';

  let { status, subject }: { status: PublicFolderNoticeState; subject: string } = $props();
</script>

{#if status === 'checking'}
  <p role="status" class="text-sm text-muted-foreground">Checking public links…</p>
{:else if status === 'public' || status === 'unavailable'}
  <p class="text-sm text-muted-foreground">
    {subject} will be available to anyone with {status === 'public' ? 'a public link to this location' : 'a public folder link, if one is active here'}.
  </p>
{/if}

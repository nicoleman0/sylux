<script lang="ts">
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  let { url }: { url: string | null } = $props();
  let failed = $state<string | null>(null);
</script>

{#if url && failed !== url}
  <img class="instance-logo" src={url} alt="" onerror={() => failed = url} />
{:else}
  <FolderOpenIcon aria-hidden="true" />
{/if}

<style>
  .instance-logo { width: 25px; height: 25px; object-fit: contain; flex-shrink: 0; }
</style>

<script module lang="ts">
  export type EntryAction = 'download' | 'share' | 'public-link' | 'rename' | 'move' | 'copy' | 'delete';
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { DropdownMenu } from 'bits-ui';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import { Button } from '$lib/components/ui/button/index.js';
  import { modifiedTime } from './modified-time';
  import type { Entry } from './listing';

  let { entry, disabled = false, moveDisabled = false, copyDisabled = false, downloadDisabled = false, downloading = false, onaction }: {
    entry: Entry;
    disabled?: boolean;
    moveDisabled?: boolean;
    copyDisabled?: boolean;
    downloadDisabled?: boolean;
    downloading?: boolean;
    onaction: (action: EntryAction, trigger: HTMLElement) => void;
  } = $props();
  let trigger = $state<HTMLButtonElement | null>(null);
  let open = $state(false);
  let openingDialog = false;
  const modified = $derived(modifiedTime(entry));

  async function select(action: EntryAction): Promise<void> {
    openingDialog = action !== 'download';
    open = false;
    await tick();
    if (trigger) onaction(action, trigger);
  }
</script>

<DropdownMenu.Root bind:open onOpenChange={(value) => { if (value) openingDialog = false; }}>
  <DropdownMenu.Trigger {disabled} bind:ref={trigger}>
    {#snippet child({ props })}
      <Button {...props} variant="ghost" size="icon" class="size-11" aria-busy={downloading} aria-label={downloading ? `Preparing download for ${entry.name}` : `Options for ${entry.name}`}>{#if downloading}<LoaderCircleIcon class="animate-spin motion-reduce:animate-none" />{:else}<EllipsisIcon />{/if}</Button>
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content preventScroll={false} sideOffset={4} align="end" class="bg-popover text-popover-foreground z-50 min-w-40 rounded-md border p-1 shadow-md outline-none" onInteractOutside={(event) => { if (event.target instanceof Node && trigger?.contains(event.target)) event.preventDefault(); }} onCloseAutoFocus={(event) => { if (openingDialog) event.preventDefault(); }}>
      {#if modified}<div class="px-3 py-2 text-xs text-muted-foreground max-w-72 border-b mb-1"><span class="block">Modified</span><time datetime={modified.datetime}>{modified.full}</time></div>{/if}
      <DropdownMenu.Item class="menu-item" disabled={downloadDisabled} onSelect={() => select('download')}>Download</DropdownMenu.Item>
      <DropdownMenu.Item class="menu-item" onSelect={() => select('share')}>Share</DropdownMenu.Item>
      {#if entry.kind === 'folder'}<DropdownMenu.Item class="menu-item" onSelect={() => select('public-link')}>Public link</DropdownMenu.Item>{/if}
      <DropdownMenu.Item class="menu-item" onSelect={() => select('rename')}>Rename</DropdownMenu.Item>
      <DropdownMenu.Item class="menu-item" disabled={moveDisabled} onSelect={() => select('move')}>Move</DropdownMenu.Item>
      <DropdownMenu.Item class="menu-item" disabled={copyDisabled} onSelect={() => select('copy')}>Copy</DropdownMenu.Item>
      <DropdownMenu.Item class="menu-item text-destructive" onSelect={() => select('delete')}>Delete</DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>

<style>
  :global(.menu-item) { display: flex; min-height: 2.75rem; align-items: center; border-radius: .2rem; padding: .5rem .75rem; cursor: default; outline: none; }
  :global(.menu-item[data-disabled]) { opacity: .5; }
  :global(.menu-item[data-highlighted]) { background: var(--accent); color: var(--accent-foreground); }
</style>

<script lang="ts">
  import { tick } from 'svelte';
  import { DropdownMenu } from 'bits-ui';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import { Button } from '$lib/components/ui/button/index.js';
  import type { Entry } from './listing';

  let { entry, disabled = false, moveDisabled = false, onaction }: {
    entry: Entry;
    disabled?: boolean;
    moveDisabled?: boolean;
    onaction: (action: 'download' | 'rename' | 'move' | 'delete', trigger: HTMLElement) => void;
  } = $props();
  let trigger = $state<HTMLButtonElement | null>(null);
  let open = $state(false);
  let openingDialog = false;

  async function select(action: 'download' | 'rename' | 'move' | 'delete'): Promise<void> {
    openingDialog = action !== 'download';
    open = false;
    await tick();
    if (trigger) onaction(action, trigger);
  }
</script>

<DropdownMenu.Root bind:open onOpenChange={(value) => { if (value) openingDialog = false; }}>
  <DropdownMenu.Trigger {disabled} bind:ref={trigger}>
    {#snippet child({ props })}
      <Button {...props} variant="ghost" size="icon" class="size-11" aria-label={`Options for ${entry.name}`}><EllipsisIcon /></Button>
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content preventScroll={false} sideOffset={4} align="end" class="bg-popover text-popover-foreground z-50 min-w-40 rounded-md border p-1 shadow-md outline-none" onInteractOutside={(event) => { if (event.target instanceof Node && trigger?.contains(event.target)) event.preventDefault(); }} onCloseAutoFocus={(event) => { if (openingDialog) event.preventDefault(); }}>
      {#if entry.kind === 'file'}<DropdownMenu.Item class="menu-item" onSelect={() => select('download')}>Download</DropdownMenu.Item>{/if}
      <DropdownMenu.Item class="menu-item" onSelect={() => select('rename')}>Rename</DropdownMenu.Item>
      <DropdownMenu.Item class="menu-item" disabled={moveDisabled} onSelect={() => select('move')}>Move</DropdownMenu.Item>
      <DropdownMenu.Item class="menu-item text-destructive" onSelect={() => select('delete')}>Delete</DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>

<style>
  :global(.menu-item) { display: flex; min-height: 2.75rem; align-items: center; border-radius: .2rem; padding: .5rem .75rem; cursor: default; outline: none; }
  :global(.menu-item[data-disabled]) { opacity: .5; }
  :global(.menu-item[data-highlighted]) { background: var(--accent); color: var(--accent-foreground); }
</style>

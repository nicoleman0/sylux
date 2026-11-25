<script lang="ts">
  import { tick } from 'svelte';
  import { DropdownMenu } from 'bits-ui';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import { Button } from '$lib/components/ui/button/index.js';
  import type { Account } from './session';

  export type AccountAction = 'personal' | 'reset' | 'role' | 'disable' | 'enable';
  let { account, self = false, lastAdministrator = false, disabled = false, onaction }: {
    account: Account;
    self?: boolean;
    lastAdministrator?: boolean;
    disabled?: boolean;
    onaction: (action: AccountAction, trigger: HTMLElement) => void;
  } = $props();
  let trigger = $state<HTMLButtonElement | null>(null);
  let open = $state(false);
  let openingDialog = false;
  async function select(action: AccountAction): Promise<void> {
    openingDialog = true;
    open = false;
    await tick();
    if (trigger) onaction(action, trigger);
  }
</script>

<DropdownMenu.Root bind:open onOpenChange={(value) => { if (value) openingDialog = false; }}>
  <DropdownMenu.Trigger {disabled} bind:ref={trigger}>
    {#snippet child({ props })}<Button {...props} variant="ghost" size="icon" class="size-11" aria-label={`Actions for ${account.handle}`} title={`Actions for ${account.handle}`}><EllipsisIcon /></Button>{/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content preventScroll={false} sideOffset={4} align="end" class="bg-popover text-popover-foreground z-50 min-w-48 max-w-72 rounded-md border p-1 shadow-md outline-none" onCloseAutoFocus={(event) => { if (openingDialog) event.preventDefault(); }}>
      {#if self}
        <DropdownMenu.Item class="account-menu-item" onSelect={() => select('personal')}>Personal account settings</DropdownMenu.Item>
        <p class="px-3 py-2 text-xs text-muted-foreground">You cannot disable your own account or change your own role.{lastAdministrator ? ' This is the last enabled administrator.' : ''}</p>
      {:else if account.enabled}
        {#if account.role === 'user' && account.credential_mode !== 'external'}<DropdownMenu.Item class="account-menu-item" onSelect={() => select('reset')}>Reset password</DropdownMenu.Item>{/if}
        {#if account.credential_mode !== 'external'}<DropdownMenu.Item class="account-menu-item" disabled={lastAdministrator} onSelect={() => select('role')}>{account.role === 'user' ? 'Make administrator' : 'Make user'}</DropdownMenu.Item>{/if}
        <DropdownMenu.Item class="account-menu-item text-destructive" disabled={lastAdministrator} onSelect={() => select('disable')}>Disable</DropdownMenu.Item>
        {#if lastAdministrator}<p class="px-3 py-2 text-xs text-muted-foreground">Keep at least one enabled administrator.</p>{/if}
      {:else}
        <DropdownMenu.Item class="account-menu-item" onSelect={() => select('enable')}>Enable</DropdownMenu.Item>
      {/if}
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>

<style>
  :global(.account-menu-item) { display: flex; min-height: 2.75rem; align-items: center; border-radius: .2rem; padding: .5rem .75rem; cursor: default; outline: none; }
  :global(.account-menu-item[data-disabled]) { opacity: .5; }
  :global(.account-menu-item[data-highlighted]) { background: var(--accent); color: var(--accent-foreground); }
</style>

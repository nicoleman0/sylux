<script lang="ts">
  import { Tooltip } from 'bits-ui';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import SquareXIcon from '@lucide/svelte/icons/square-x';
  let { enabled }: { enabled: boolean } = $props();
</script>

<Tooltip.Provider delayDuration={200}>
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <button {...props} type="button" aria-label={enabled ? 'Active' : 'Disabled'} class="account-status" class:enabled>
          {#if enabled}<CircleCheckIcon aria-hidden="true" size={18} />{:else}<SquareXIcon aria-hidden="true" size={18} />{/if}
        </button>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Content role="tooltip" sideOffset={6} class="bg-popover text-popover-foreground z-50 rounded-md border px-3 py-2 text-xs shadow-md">{enabled ? 'Active' : 'Disabled'}</Tooltip.Content></Tooltip.Portal>
  </Tooltip.Root>
</Tooltip.Provider>

<style>
  .account-status { display: inline-flex; min-width: 44px; min-height: 44px; align-items: center; justify-content: center; padding: 4px; vertical-align: middle; background: transparent; border: 0; color: var(--destructive); }
  .account-status:focus-visible { outline: 2px solid var(--ring); outline-offset: 4px; border-radius: 2px; }
  .account-status.enabled { color: var(--primary); }
</style>

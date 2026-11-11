<script lang="ts">
  import type { Snippet } from 'svelte';
  import { Tooltip } from 'bits-ui';
  import { Button } from '$lib/components/ui/button/index.js';
  let { label, pressed, disabled = false, onclick, children }: {
    label: string;
    pressed?: boolean;
    disabled?: boolean;
    onclick: () => void;
    children: Snippet;
  } = $props();
</script>

<Tooltip.Provider delayDuration={200}>
  <Tooltip.Root>
    <Tooltip.Trigger {disabled}>
      {#snippet child({ props })}<Button {...props} variant="outline" size="icon" class="header-icon-button" aria-label={label} aria-pressed={pressed} {onclick}>{@render children()}</Button>{/snippet}
    </Tooltip.Trigger>
    <Tooltip.Portal><Tooltip.Content sideOffset={6} class="bg-popover text-popover-foreground z-50 rounded-md border px-3 py-2 text-xs shadow-md">{label}</Tooltip.Content></Tooltip.Portal>
  </Tooltip.Root>
</Tooltip.Provider>

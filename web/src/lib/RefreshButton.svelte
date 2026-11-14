<script lang="ts">
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import { Button, type ButtonProps, type ButtonSize, type ButtonVariant } from '$lib/components/ui/button/index.js';

  type Props = Omit<ButtonProps, 'children' | 'variant' | 'size' | 'onclick' | 'disabled'> & {
    busy: boolean;
    label: string;
    busyLabel?: string;
    disabled?: boolean;
    onclick: (event: MouseEvent) => void;
    id?: string;
    variant?: ButtonVariant;
    size?: ButtonSize;
  };

  let {
    busy,
    label,
    busyLabel = 'Refreshing…',
    disabled = false,
    onclick,
    id,
    variant = 'outline',
    size = 'sm',
    class: className,
    ...rest
  }: Props = $props();
</script>

<Button {id} {variant} {size} class={`refresh-button ${className ?? ''}`} onclick={(event) => { if (!busy && !disabled) onclick(event); }} disabled={disabled || busy} {...rest} aria-label={rest['aria-label'] ?? label} aria-busy={busy}>
  <span class="refresh-button-label">
    <span class="refresh-button-state" class:refresh-button-hidden={busy} aria-hidden={busy}><RefreshCwIcon aria-hidden="true" />{label}</span>
    <span class="refresh-button-state" class:refresh-button-hidden={!busy} aria-hidden={!busy}>
      <LoaderCircleIcon class="refreshing" data-refreshing={busy} aria-hidden="true" />{busyLabel}
    </span>
  </span>
</Button>

<style>
  .refresh-button-label { display: grid; }
  .refresh-button-state { grid-area: 1 / 1; display: inline-flex; align-items: center; gap: 0.375rem; }
  .refresh-button-hidden { visibility: hidden; }
  .refresh-button-state :global(.refreshing) { animation: refresh-turn 1s linear infinite; }
  @keyframes refresh-turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .refresh-button-state :global(.refreshing) { animation: none; } }
</style>

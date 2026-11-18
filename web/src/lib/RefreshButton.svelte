<script lang="ts">
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import { Button, type ButtonProps, type ButtonSize, type ButtonVariant } from '$lib/components/ui/button/index.js';

  type Props = Omit<ButtonProps, 'children' | 'variant' | 'size' | 'onclick' | 'disabled'> & {
    busy: boolean;
    label: string;
    busyLabel?: string;
    disabled?: boolean;
    keepFocus?: boolean;
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
    keepFocus = false,
    onclick,
    id,
    variant = 'outline',
    size = 'sm',
    class: className,
    ...rest
  }: Props = $props();
</script>

<Button {id} {variant} size={size === 'sm' ? 'icon-sm' : size === 'default' ? 'icon' : size} class={`refresh-button ${className ?? ''}`} onclick={(event) => { if (!busy && !disabled) onclick(event); }} disabled={disabled || (busy && !keepFocus)} aria-disabled={keepFocus && busy ? 'true' : undefined} {...rest} aria-label={rest['aria-label'] ?? label} title={busy ? `${rest['aria-label'] ?? label} · ${busyLabel}` : rest['aria-label'] ?? label} aria-busy={busy}>
  {#if busy}<LoaderCircleIcon class="refresh-spinner" data-refreshing="true" aria-hidden="true" />{:else}<RefreshCwIcon aria-hidden="true" />{/if}
</Button>

<style>
  :global(.refresh-button[aria-disabled='true']) { opacity: 0.5; cursor: default; }
  :global(.refresh-button .refresh-spinner) { animation: refresh-turn 1s linear infinite; }
  @keyframes refresh-turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { :global(.refresh-button .refresh-spinner) { animation: none; } }
</style>

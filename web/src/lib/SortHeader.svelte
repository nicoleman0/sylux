<script lang="ts">
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
  import { Button } from '$lib/components/ui/button/index.js';
  let { label, column, sort, disabled = false, busy = false, onclick, class: className = '' }: {
    label: string; column: string; sort: string; disabled?: boolean; busy?: boolean; onclick: () => void; class?: string;
  } = $props();
  const active = $derived(sort.startsWith(`${column}_`));
  const descending = $derived(sort.endsWith('_desc'));
</script>

<span class={`sort-header ${className}`} role="columnheader" aria-label={label} aria-sort={active ? descending ? 'descending' : 'ascending' : undefined}>
  <Button variant="ghost" size="sm" class="sort-heading" disabled={disabled} aria-disabled={disabled || busy} aria-busy={busy} onclick={() => { if (!disabled && !busy) onclick(); }} aria-label={`Sort by ${label.toLowerCase()}, ${active && !descending ? 'descending' : 'ascending'}`}>
    {label}
    {#if busy}<LoaderCircle class="sort-spinner" aria-hidden="true" />{:else if active}{#if descending}<ArrowDown aria-hidden="true" />{:else}<ArrowUp aria-hidden="true" />{/if}{:else}<ChevronsUpDown aria-hidden="true" />{/if}
  </Button>
</span>

<style>
  :global(.sort-heading .sort-spinner) { animation: sort-turn 1s linear infinite; }
  @keyframes sort-turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { :global(.sort-heading .sort-spinner) { animation: none; } }
</style>

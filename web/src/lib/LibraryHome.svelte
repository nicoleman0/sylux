<script lang="ts">
  import FolderIcon from '@lucide/svelte/icons/folder';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { HttpError } from '$lib/api';
  import type { Folder } from '$lib/listing';

  let { items, pending, error, onopen, onretry, oncreate, onshare }: {
    items: Folder[];
    pending: boolean;
    error: string;
    onopen: (folder: Folder) => void;
    onretry: () => void;
    oncreate: (name: string) => Promise<Folder>;
    onshare: (folder: Folder, trigger: HTMLElement) => void;
  } = $props();

  let open = $state(false);
  let busy = $state(false);
  let name = $state('');
  let formError = $state('');
  let notice = $state('');
  let trigger = $state<HTMLButtonElement | null>(null);

  $effect(() => { if (pending) notice = ''; });

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy || pending || !name.trim()) return;
    busy = true;
    formError = '';
    try {
      const library = await oncreate(name.trim());
      notice = `Created ${library.name}.`;
      open = false;
    } catch (failure) {
      formError = failure instanceof HttpError && failure.code === 'name_conflict'
        ? 'You already have a library with this name.'
        : failure instanceof HttpError && failure.code === 'invalid_name'
          ? 'Enter a library name without slashes or control characters, up to 255 bytes.'
          : 'Could not confirm creation. Reopen My libraries to check before trying again.';
    } finally {
      busy = false;
    }
  }

  function openFromLink(event: MouseEvent, folder: Folder): void {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    onopen(folder);
  }
</script>

<div class="flex flex-wrap items-center justify-between gap-3">
  <h1 id="libraries-title" tabindex="-1">My libraries</h1>
  <Button bind:ref={trigger} disabled={pending || !!error} onclick={() => { name = ''; formError = ''; notice = ''; open = true; }}>
    <PlusIcon />New library
  </Button>
</div>
{#if error}
  <p role="alert">{error}</p>
  <Button variant="outline" class="justify-self-start" onclick={onretry}>Try again</Button>
{:else}
  {#if notice && !pending}<p role="status" class="text-sm break-words">{notice}</p>{/if}
  {#if items.length}
    <ul aria-label="My libraries" class="divide-y rounded-lg border">
      {#each items as folder (folder.id)}
        <li class="flex items-center gap-2 pr-2"><a class="flex min-h-11 min-w-0 flex-1 items-center gap-3 rounded-md px-3 py-2 font-medium hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring"
          href={`#files/${encodeURIComponent(folder.id)}`} onclick={(event) => openFromLink(event, folder)}>
          <FolderIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span class="min-w-0 [overflow-wrap:anywhere]">{folder.name}</span>
        </a><Button variant="ghost" size="sm" aria-label={`Share ${folder.name}`} onclick={(event) => onshare(folder, event.currentTarget)}>Share</Button></li>
      {/each}
    </ul>
  {/if}
{/if}

<Dialog.Root {open} onOpenChange={(next) => { if (!busy) open = next; }}>
  <Dialog.Content aria-describedby={undefined} showCloseButton={!busy}
    escapeKeydownBehavior={busy ? 'ignore' : 'close'} interactOutsideBehavior={busy ? 'ignore' : 'close'}
    onCloseAutoFocus={(event) => { event.preventDefault(); if (trigger?.isConnected) trigger.focus(); }}>
    <Dialog.Header><Dialog.Title>New library</Dialog.Title></Dialog.Header>
    <form class="grid gap-4" onsubmit={create}>
      <div class="grid gap-2">
        <Label for="library-name">Library name</Label>
        <Input id="library-name" maxlength={255} bind:value={name} disabled={busy} required />
      </div>
      {#if formError}<p role="alert">{formError}</p>{/if}
      <Dialog.Footer>
        <Button type="button" variant="outline" disabled={busy} onclick={() => open = false}>Cancel</Button>
        <Button type="submit" disabled={busy || !name.trim()} aria-busy={busy}>{busy ? 'Creating…' : 'Create library'}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

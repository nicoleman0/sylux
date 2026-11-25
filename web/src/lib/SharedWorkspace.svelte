<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import FileIcon from '@lucide/svelte/icons/file';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { HttpError, type ApiClient } from './api';
  import { DirectSharingApi, type SharedRoot } from './direct-grants';
  import FilePreview from './FilePreview.svelte';
  import RefreshButton from './RefreshButton.svelte';
  import type { Entry, FileEntry, Folder, SearchHit } from './listing';
  import type { TextEditState } from './text-edit';

  let { client, location, onnavigate }: {
    client: ApiClient;
    location: { folder: string; file?: string };
    onnavigate: (folder?: string, file?: string) => void;
  } = $props();

  const sharing = $derived(new DirectSharingApi(client));
  let path = $state<Folder[]>([]);
  let roots = $state<SharedRoot[]>([]);
  let entries = $state<Entry[]>([]);
  let results = $state<SearchHit[]>([]);
  let file = $state<FileEntry | null>(null);
  let queryInput = $state('');
  let query = $state('');
  let next = $state<string | null>(null);
  let busy = $state(false);
  let error = $state('');
  let request: AbortController | undefined;
  const editSession = new AbortController();
  let edit = $state<TextEditState>({ draft: null, operation: null, stale: false });
  const folder = $derived(path.at(-1));
  const pdf = $derived(file?.name.toLowerCase().endsWith('.pdf') ? file : null);

  onDestroy(() => { request?.abort(); editSession.abort(); });

  $effect(() => {
    const target = location;
    untrack(() => {
      path = []; roots = []; entries = []; results = []; file = null; query = ''; queryInput = ''; next = null;
      void load(target);
    });
  });

  async function load(target = location, cursor: string | null = null): Promise<void> {
    request?.abort();
    const controller = request = new AbortController();
    const { signal } = controller;
    const search = query;
    busy = true; error = '';
    if (!cursor) next = null;
    try {
      if (target.file) {
        const found = await sharing.file(target.file, signal);
        if (!signal.aborted) { file = found.entry; path = found.location; }
      } else if (search) {
        const [current, response] = await Promise.all([
          target.folder ? sharing.path(target.folder, signal) : null,
          sharing.search(search, target.folder, cursor, signal),
        ]);
        if (!signal.aborted) {
          if (current) path = current.items;
          results = cursor ? [...results, ...response.items] : response.items;
          next = response.next_cursor;
        }
      } else if (target.folder) {
        const response = await Promise.all([
          cursor ? Promise.resolve({ items: path }) : sharing.path(target.folder, signal),
          sharing.children(target.folder, cursor, signal),
        ]);
        if (!signal.aborted) {
          path = response[0].items;
          entries = cursor ? [...entries, ...response[1].items] : response[1].items;
          next = response[1].next_cursor;
        }
      } else {
        const response = await sharing.incoming(signal);
        if (!signal.aborted) roots = response.items;
      }
    } catch (failure) {
      if (signal.aborted) return;
      if (failure instanceof HttpError && (failure.status === 404 || failure.status === 401)) {
        path = []; roots = []; entries = []; results = []; file = null; next = null;
        error = 'This share is unavailable.';
      } else if (cursor && failure instanceof HttpError && (failure.status === 400 || failure.status === 409)) {
        return load(target);
      } else {
        error = 'Could not load shared files. Try again.';
      }
    } finally { if (!signal.aborted) busy = false; }
  }

  function search(event: SubmitEvent): void {
    event.preventDefault();
    const value = queryInput.trim();
    if (!value || new TextEncoder().encode(value).length > 255 || /\p{Cc}/u.test(value)) return;
    query = value; results = [];
    void load();
  }

  function link(folder?: string, file?: string): string {
    return file ? `#shared/file/${encodeURIComponent(file)}` : folder ? `#shared/${encodeURIComponent(folder)}` : '#shared';
  }

  function follow(event: MouseEvent, folder?: string, file?: string): void {
    if (event.button || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    onnavigate(folder, file);
  }

  function download(entry: FileEntry): void {
    const anchor = document.createElement('a');
    anchor.href = sharing.content(entry.id);
    anchor.download = entry.name;
    anchor.click();
  }
</script>

{#snippet row(entry: Entry, detail = '')}
  <li class="flex min-h-11 items-center gap-3 px-3 py-1">
    <a class="flex min-h-9 min-w-0 flex-1 items-center gap-3 rounded-sm [overflow-wrap:anywhere] hover:underline focus-visible:outline-2 focus-visible:outline-ring"
      href={entry.kind === 'folder' ? link(entry.id) : link(undefined, entry.id)}
      onclick={(event) => entry.kind === 'folder' ? follow(event, entry.id) : follow(event, undefined, entry.id)}>
      {#if entry.kind === 'folder'}<FolderIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />{:else}<FileIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />{/if}
      <span><span class="font-medium">{entry.name}</span>{#if detail}<span class="block text-xs text-muted-foreground">{detail}</span>{/if}</span>
    </a>
    {#if entry.kind === 'file'}<Button variant="ghost" size="icon" class="size-11" aria-label={`Download ${entry.name}`} title={`Download ${entry.name}`} onclick={() => download(entry)}><DownloadIcon /></Button>{/if}
  </li>
{/snippet}

<section class="files-content grid gap-4" aria-labelledby="shared-title" aria-busy={busy}>
  <div>
    {#if location.folder || location.file}
      <nav class="breadcrumb flex-wrap" aria-label="Shared folder path">
        <a href={link()} onclick={follow}>Shared with me</a>
        {#each (file ? path : path.slice(0, -1)) as part (part.id)}
          <span aria-hidden="true">/</span><a href={link(part.id)} onclick={(event) => follow(event, part.id)}>{part.name}</a>
        {/each}
      </nav>
    {/if}
    <div class="flex items-center justify-between gap-3">
      <h1 id="shared-title" tabindex="-1" class="min-w-0 [overflow-wrap:anywhere]">{file?.name ?? folder?.name ?? 'Shared with me'}</h1>
      <RefreshButton label="Refresh shared files" busyLabel="Refreshing shared files…" keepFocus {busy} onclick={() => load()} />
    </div>
  </div>
  {#if !location.file}
    <form class="flex flex-wrap items-center gap-2" onsubmit={search}>
      <Input class="min-w-0 flex-1 sm:max-w-72" aria-label="Search shared files" type="search" placeholder={location.folder ? 'Search this folder' : 'Search shared files'} bind:value={queryInput} maxlength={255} />
      <Button type="submit" variant="outline" disabled={!queryInput.trim() || busy}>Search</Button>
      {#if query}<Button type="button" variant="ghost" onclick={() => { query = ''; queryInput = ''; results = []; void load(); }}>Clear search</Button>{/if}
    </form>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if busy}<p role="status">Loading shared files…</p>{/if}
  {#if file && !pdf}<Button variant="outline" class="justify-self-start" onclick={() => file && download(file)}><DownloadIcon />Download file</Button>{/if}
  {#if pdf}
    <FilePreview entry={pdf} url={sharing.content(pdf.id)} {client} readOnly bind:edit editSignal={editSession.signal}
      onclose={() => onnavigate(folder?.id)} ondownload={() => pdf && download(pdf)} onresume={() => {}} onsaved={() => {}} />
  {:else if !location.file && !error}
    {#if query && results.length}
      <ul class="divide-y rounded-lg border" aria-label="Shared search results">
        {#each results as hit (hit.entry.id)}{@render row(hit.entry, hit.location.map(part => part.name).join(' / '))}{/each}
      </ul>
    {:else if !query && location.folder && entries.length}
      <ul class="divide-y rounded-lg border" aria-label="Files in shared folder">
        {#each entries as entry (entry.id)}{@render row(entry)}{/each}
      </ul>
    {:else if !query && !location.folder && roots.length}
      <ul class="divide-y rounded-lg border" aria-label="Folders shared with me">
        {#each roots as root (root.entry.id)}{@render row(root.entry, `Shared by ${root.owner} · Read only`)}{/each}
      </ul>
    {:else if !busy}
      <p>{query ? 'No matching files or folders.' : location.folder ? 'This folder is empty.' : 'No folders or libraries have been shared with you.'}</p>
    {/if}
  {/if}
  {#if next && !error}<Button variant="outline" class="justify-self-start" disabled={busy} onclick={() => load(location, next)}>Load more</Button>{/if}
</section>

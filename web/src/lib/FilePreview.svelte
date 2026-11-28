<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import type { ApiClient } from './api';
  import { tick } from 'svelte';
  import { ListingApi, type Entry, type FileEntry } from './listing';
  import PublicFolderNotice from './PublicFolderNotice.svelte';
  import { TEXT_EDIT_LIMIT, TextSave, draftBytes, editorText, preserveLineEndings, type TextEditState } from './text-edit';
  import { loadPreview, mediaPreviewKind, type MediaPreview, type Preview } from './preview';
  import { getPdfPreviewCompatibilityError } from './pdf-compatibility';
  import XIcon from '@lucide/svelte/icons/x';
  import DownloadIcon from '@lucide/svelte/icons/download';

  let { entry, url, client, readOnly = false, edit = $bindable(), editSignal, onclose, ondownload, onresume, onsaved }: {
    entry: Entry | null;
    url: string;
    client: ApiClient;
    readOnly?: boolean;
    edit: TextEditState;
    editSignal: AbortSignal;
    onclose: () => void;
    ondownload: () => void;
    onresume: (entry: FileEntry, trigger: HTMLElement) => void;
    onsaved: (entry: FileEntry) => void;
  } = $props();
  let preview = $state<Preview | null>(null);
  let imageUrl = $state('');
  let error = $state('');
  let attempt = $state(0);
  let downloadButton: HTMLElement | null = $state(null);
  const descriptionId = $props.id();
  const isPdf = $derived(entry?.name.split('.').at(-1)?.toLowerCase() === 'pdf');
  const mediaKind = $derived(entry?.kind === 'file' ? mediaPreviewKind(entry.name) : null);

  let editBusy = $state<'save' | 'cancel' | null>(null);
  let saveError = $state('');
  let textarea = $state<HTMLTextAreaElement | null>(null);
  let editButton = $state<HTMLElement | null>(null);
  const draft = $derived(edit.draft);
  const editing = $derived(!!draft && entry?.id === draft.entry.id);
  const dirty = $derived(!!draft && draft.wire !== draft.original);
  const payload = $derived(draft ? draftBytes(draft.wire, draft.source.bom) : null);
  const tooLarge = $derived((payload?.size ?? 0) > TEXT_EDIT_LIMIT);
  const unresolved = $derived(!!edit.operation && !editBusy);
  const editError = $derived(edit.stale
    ? 'This file or folder changed. Download your draft, then reopen the current file to apply your changes.'
    : saveError || (unresolved ? 'Could not confirm the save. Retry to check the same save, or download your draft.' : ''));

  $effect(() => {
    if (!dirty && !unresolved && !editBusy) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = true;
    };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  });

  async function beginEdit() {
    const signal = editSignal;
    if (readOnly || signal.aborted || draft || entry?.kind !== 'file' || preview?.kind !== 'text' || !preview.edit) return;
    edit.draft = { entry, source: preview.edit, original: preview.text, text: editorText(preview.text), wire: preview.text };
    await tick();
    if (!signal.aborted) textarea?.focus();
  }

  function updateDraft(event: Event & { currentTarget: HTMLTextAreaElement }) {
    if (editSignal.aborted || !draft || editBusy || unresolved) return;
    const text = event.currentTarget.value;
    draft.wire = text === editorText(draft.original)
      ? draft.original : preserveLineEndings(draft.wire, draft.text, text);
    draft.text = text;
    saveError = '';
  }

  function clearDraft() {
    edit.draft = null;
    edit.operation = null;
    edit.stale = false;
    saveError = '';
  }

  async function saveDraft() {
    const signal = editSignal;
    if (signal.aborted || !editing || !draft || !dirty || !payload || tooLarge || edit.stale || editBusy) return;
    const { entry: target, source } = draft;
    const bytes = payload;
    editBusy = 'save';
    saveError = '';
    try {
      if (!edit.operation) {
        const path = await new ListingApi(client).path(target.parent_id, signal);
        const parent = path.items.at(-1);
        if (parent?.id !== target.parent_id) throw new Error('Unexpected edit location');
        signal.throwIfAborted();
        edit.operation = new TextSave(target, source.revision, parent.etag, bytes);
      }
      const operation = edit.operation;
      const outcome = await operation.run(client, signal);
      if (signal.aborted) return;
      if (outcome === 'saved') {
        clearDraft();
        onsaved(target);
      } else if (outcome === 'stale') {
        edit.operation = null;
        edit.stale = true;
      } else if (!operation.inProgress) {
        edit.operation = null;
        saveError = 'Could not save. Your draft is kept here. Try saving again.';
      }
    } catch {
      if (!signal.aborted && !edit.operation?.inProgress) {
        edit.operation = null;
        saveError = 'Could not save. Your draft is kept here. Try saving again.';
      }
    } finally {
      if (!signal.aborted) editBusy = null;
    }
  }

  async function cancelDraft() {
    const signal = editSignal;
    if (signal.aborted || !draft || editBusy) return;
    if (edit.operation?.inProgress) {
      const target = draft.entry;
      editBusy = 'cancel';
      const result = await edit.operation.cancel(client, signal);
      if (signal.aborted) return;
      editBusy = null;
      if (result === 'unknown') {
        saveError = 'Could not confirm cancellation. Your draft is kept here.';
        return;
      }
      if (result === 'saved') {
        clearDraft();
        onsaved(target);
        return;
      }
    }
    const closeStale = edit.stale;
    clearDraft();
    if (closeStale) {
      onclose();
      return;
    }
    await tick();
    if (!signal.aborted && !editing) (editButton ?? downloadButton)?.focus();
  }

  function downloadDraft() {
    if (editSignal.aborted || !draft || !payload) return;
    const anchor = document.createElement('a');
    const objectUrl = URL.createObjectURL(payload);
    anchor.href = objectUrl;
    anchor.download = draft.entry.name.replace(/(\.[^.]+)$/, '.draft$1');
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(objectUrl), 1000);
  }

  function close() {
    if (editSignal.aborted) return;
    if (editing && !dirty && !editBusy && !unresolved) clearDraft();
    onclose();
  }

  function attachMedia(element: HTMLMediaElement, media: MediaPreview) {
    const target = entry;
    const controller = new AbortController();
    let probing = false;
    async function fail() {
      if (!element.error || probing || entry !== target || preview !== media || url !== media.url) return;
      probing = true;
      const code = element.error.code;
      let message = code === MediaError.MEDIA_ERR_NETWORK
        ? 'Could not load this media. Try again or download the file.'
        : 'This media could not be played. Download the file to open it.';
      try {
        await client.request('/me', { signal: controller.signal });
      } catch {
        message = 'Could not verify your session. Try again or download the file.';
      }
      if (controller.signal.aborted || entry !== target || preview !== media || url !== media.url) return;
      if (document.activeElement === element) downloadButton?.focus();
      preview = { kind: 'unsupported', message };
    }
    element.addEventListener('error', fail);
    element.setAttribute('src', media.url);
    return {
      destroy() {
        controller.abort();
        element.removeEventListener('error', fail);
        element.pause();
        // Reset resource selection so the browser cancels the old request.
        element.removeAttribute('src');
        element.load();
      },
    };
  }

  $effect(() => {
    const target = entry;
    const path = url;
    const api = client;
    void attempt;
    preview = null;
    error = '';
    imageUrl = '';
    if (!target || target.kind !== 'file') return;
    const controller = new AbortController();
    let objectUrl = '';
    void loadPreview(api, target, path, controller.signal).then((result) => {
      if (controller.signal.aborted) return;
      if (result.kind === 'image') imageUrl = objectUrl = URL.createObjectURL(result.blob);
      preview = result;
    }).catch(() => {
      if (!controller.signal.aborted) error = 'Could not load this preview. Try again or download the file.';
    });
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  });
</script>

{#if draft && !editing}
  <div class="action-feedback workspace-feedback" aria-busy={!!editBusy}>
    <p role="status" class="min-w-0 break-all">{editBusy ? 'Working on your edit' : 'Draft available'} for {draft.entry.name}.</p>
    <Button variant="outline" size="sm" onclick={(event) => { if (!editSignal.aborted && draft) onresume(draft.entry, event.currentTarget); }}>Resume edit</Button>
  </div>
{/if}

<Dialog.Root open={!!entry} onOpenChange={(open) => { if (!open) close(); }}>
  <Dialog.Content
    aria-describedby={isPdf || mediaKind || editing ? descriptionId : undefined}
    class={isPdf
      ? 'pdf-reader-shell !max-w-none !w-full !h-[100dvh] !max-h-none !rounded-none !gap-0 !p-0 !ring-0 overflow-hidden flex flex-col'
      : mediaKind === 'audio'
        ? '!max-w-xl w-[calc(100vw-1rem)] max-h-[calc(100dvh-1rem)] flex flex-col gap-4 p-4 sm:p-6'
        : '!max-w-[calc(100vw-1rem)] w-[calc(100vw-1rem)] h-[calc(100dvh-1rem)] max-h-[calc(100dvh-1rem)] overflow-hidden flex flex-col gap-4 p-4 sm:!max-w-[min(92vw,1400px)] sm:w-[92vw] sm:h-[88dvh] sm:max-h-[88dvh] sm:gap-6 sm:p-6'}
    showCloseButton={false}
    onOpenAutoFocus={(event) => {
      if (editing && textarea) {
        event.preventDefault();
        textarea.focus();
      }
    }}
    onCloseAutoFocus={(event) => event.preventDefault()}
  >
    {#if isPdf}
      <header class="reader-header">
        <Button variant="ghost" size="icon" aria-label="Close preview" title="Close preview" onclick={onclose}><XIcon /></Button>
        <Dialog.Title class="min-w-0 flex-1 truncate text-sm" title={entry?.name}><span class="sr-only">Preview </span>{entry?.name}</Dialog.Title>
        <p id={descriptionId} class="sr-only">PDF reader</p>
        <Button variant="ghost" size="icon" aria-label="Download file" title="Download file" onclick={ondownload}><DownloadIcon /></Button>
      </header>
    {:else if mediaKind}
      <Dialog.Header class="shrink-0">
        <Dialog.Title class="truncate" title={entry?.name}><span class="sr-only">Preview </span>{entry?.name}</Dialog.Title>
        <p id={descriptionId} class="sr-only">Media preview</p>
      </Dialog.Header>
    {:else}
    <Dialog.Header class="shrink-0">
      <Dialog.Title class="break-all">{editing ? 'Edit' : 'Preview'} {entry?.name}</Dialog.Title>
      {#if editing}
        <p id={descriptionId} class="text-muted-foreground text-sm">Saving moves the previous version to Trash and ends any download links for it.</p>
      {/if}
    </Dialog.Header>
    {/if}
    <div class={isPdf ? 'reader-body min-h-0 min-w-0 flex-1 overflow-hidden' : 'min-h-0 min-w-0 flex-1 overflow-auto'} aria-busy={!editing && !preview && !error}>
      {#if editing && draft}
        <textarea bind:this={textarea} aria-label="File content" aria-invalid={tooLarge || undefined}
          aria-describedby={editError || tooLarge ? 'text-edit-error' : undefined} spellcheck={false}
          readonly={!!editBusy || unresolved} value={draft.text} oninput={updateDraft}
          onkeydown={(event) => {
            if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
              event.preventDefault();
              void saveDraft();
            }
          }}
          class="border-input bg-muted text-foreground focus-visible:ring-ring h-full min-h-0 w-full resize-none rounded-md border p-4 font-mono text-sm focus-visible:ring-2 focus-visible:ring-inset focus-visible:outline-none"
        ></textarea>
      {:else if error}
        <div class="grid gap-3"><p role="alert">{error}</p><Button variant="outline" class="justify-self-start" onclick={() => attempt++}>Retry preview</Button></div>
      {:else if preview?.kind === 'text'}
        <pre class="bg-muted max-h-full overflow-auto rounded-md p-4 font-mono text-sm whitespace-pre-wrap break-words">{preview.text || 'This file is empty.'}</pre>
      {:else if preview?.kind === 'image'}
        <img src={imageUrl} alt={entry?.name ?? 'File preview'} class="mx-auto max-h-full max-w-full object-contain" onerror={() => { error = 'This image could not be displayed. Download the file to open it.'; }} />
      {:else if preview?.kind === 'video' || preview?.kind === 'audio'}
        {#key preview}
          <div class="flex h-full min-h-0 items-center justify-center">
            {#if preview.kind === 'video'}
              <!-- svelte-ignore a11y_media_has_caption (preview plays user files; captions may be embedded) -->
              <video use:attachMedia={preview} controls preload="none" playsinline aria-label={entry?.name} class="media-player max-h-full w-full max-w-4xl bg-black object-contain"></video>
            {:else}
              <audio use:attachMedia={preview} controls preload="none" aria-label={entry?.name} class="media-player w-full max-w-xl"></audio>
            {/if}
          </div>
        {/key}
      {:else if preview?.kind === 'pdf'}
        {@const compatibilityError = getPdfPreviewCompatibilityError()}
        {#if compatibilityError}
          <p role="alert">{compatibilityError}</p>
        {:else}
          {#await import('./PdfPreview.svelte')}
            <p role="status">Opening PDF…</p>
          {:then component}
            <component.default bytes={preview.bytes} onretry={() => attempt++} />
          {:catch}
            <div class="grid gap-3"><p role="alert">Could not load the PDF viewer. Retry or download the file.</p><Button variant="outline" class="justify-self-start" onclick={() => attempt++}>Retry preview</Button></div>
          {/await}
        {/if}
      {:else if preview?.kind === 'unsupported'}
        <p role="status">{preview.message}</p>
      {:else}
        <p role="status">Loading preview…</p>
      {/if}
    </div>
    {#if editing}
      <PublicFolderNotice {client} folder={entry?.parent_id ?? null} subject="The saved changes" />
      {#if editError || tooLarge}
        <p id="text-edit-error" role="alert" class="text-destructive shrink-0 text-sm">
          {tooLarge ? `The edit limit is ${TEXT_EDIT_LIMIT / 1024} KiB. Shorten the text or download your draft.` : editError}
        </p>
      {/if}
      <Dialog.Footer class="shrink-0 flex-row flex-wrap justify-end">
        <div class="flex flex-wrap justify-end gap-2">
          <Button variant="ghost" onclick={close}>Close preview</Button>
          {#if dirty}<Button variant="outline" onclick={downloadDraft}>Download draft</Button>{/if}
        </div>
        <div class="flex gap-2">
          <Button variant="outline" disabled={editBusy === 'save'} aria-disabled={!!editBusy}
            aria-busy={editBusy === 'cancel'} onclick={cancelDraft}>{editBusy === 'cancel' ? 'Canceling…' : 'Cancel'}</Button>
          <Button disabled={!dirty || tooLarge || edit.stale || editBusy === 'cancel'}
            aria-disabled={editBusy === 'save' || undefined} aria-busy={editBusy === 'save'} onclick={saveDraft}>
            {editBusy === 'save' ? 'Saving…' : unresolved ? 'Retry save' : 'Save'}
          </Button>
        </div>
      </Dialog.Footer>
    {:else if !isPdf}
      <Dialog.Footer class={mediaKind ? 'shrink-0 flex-row flex-wrap justify-end' : 'shrink-0'}>
        <Button variant="outline" onclick={close}>Close preview</Button>
        <Button bind:ref={downloadButton} variant={!readOnly && preview?.kind === 'text' && preview.edit && !draft ? 'outline' : 'default'} onclick={ondownload}>Download file</Button>
        {#if !readOnly && preview?.kind === 'text' && preview.edit && !draft}
          <Button bind:ref={editButton} onclick={beginEdit}>Edit</Button>
        {/if}
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  .reader-header { display: flex; align-items: center; gap: 0.5rem; padding: 0.375rem 0.75rem; border-bottom: 1px solid var(--border); flex-shrink: 0; }
  .reader-body > :global(p), .reader-body > :global(.grid) { margin: 1rem; }
  :global(.pdf-reader-shell[data-state]), :global(.pdf-reader-shell) { animation: none; }
  .media-player:focus-visible { outline-offset: -2px; }
</style>

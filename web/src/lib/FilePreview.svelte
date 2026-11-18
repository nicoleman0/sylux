<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import type { ApiClient } from './api';
  import type { Entry } from './listing';
  import { loadPreview, mediaPreviewKind, type MediaPreview, type Preview } from './preview';
  import { getPdfPreviewCompatibilityError } from './pdf-compatibility';
  import XIcon from '@lucide/svelte/icons/x';
  import DownloadIcon from '@lucide/svelte/icons/download';

  let { entry, url, client, onclose, ondownload }: {
    entry: Entry | null;
    url: string;
    client: ApiClient;
    onclose: () => void;
    ondownload: () => void;
  } = $props();
  let preview = $state<Preview | null>(null);
  let imageUrl = $state('');
  let error = $state('');
  let attempt = $state(0);
  let downloadButton: HTMLElement | null = $state(null);
  const isPdf = $derived(entry?.name.split('.').at(-1)?.toLowerCase() === 'pdf');
  const mediaKind = $derived(entry?.kind === 'file' ? mediaPreviewKind(entry.name) : null);

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

<Dialog.Root open={!!entry} onOpenChange={(open) => { if (!open) onclose(); }}>
  <Dialog.Content
    class={isPdf
      ? 'pdf-reader-shell !max-w-none !w-full !h-[100dvh] !max-h-none !rounded-none !gap-0 !p-0 !ring-0 overflow-hidden flex flex-col'
      : mediaKind === 'audio'
        ? '!max-w-xl w-[calc(100vw-1rem)] max-h-[calc(100dvh-1rem)] flex flex-col gap-4 p-4 sm:p-6'
        : '!max-w-[calc(100vw-1rem)] w-[calc(100vw-1rem)] h-[calc(100dvh-1rem)] max-h-[calc(100dvh-1rem)] overflow-hidden flex flex-col gap-4 p-4 sm:!max-w-[min(92vw,1400px)] sm:w-[92vw] sm:h-[88dvh] sm:max-h-[88dvh] sm:gap-6 sm:p-6'}
    showCloseButton={false}
    onCloseAutoFocus={(event) => event.preventDefault()}
  >
    {#if isPdf}
      <header class="reader-header">
        <Button variant="ghost" size="icon" aria-label="Close preview" title="Close preview" onclick={onclose}><XIcon /></Button>
        <Dialog.Title class="min-w-0 flex-1 truncate text-sm" title={entry?.name}><span class="sr-only">Preview </span>{entry?.name}</Dialog.Title>
        <Dialog.Description class="sr-only">PDF reader</Dialog.Description>
        <Button variant="ghost" size="icon" aria-label="Download file" title="Download file" onclick={ondownload}><DownloadIcon /></Button>
      </header>
    {:else if mediaKind}
      <Dialog.Header class="shrink-0">
        <Dialog.Title class="truncate" title={entry?.name}><span class="sr-only">Preview </span>{entry?.name}</Dialog.Title>
        <Dialog.Description class="sr-only">Media preview</Dialog.Description>
      </Dialog.Header>
    {:else}
    <Dialog.Header class="shrink-0">
      <Dialog.Title class="break-all">Preview {entry?.name}</Dialog.Title>
      <Dialog.Description>Read-only preview.</Dialog.Description>
    </Dialog.Header>
    {/if}
    <div class={isPdf ? 'reader-body min-h-0 min-w-0 flex-1 overflow-hidden' : 'min-h-0 min-w-0 flex-1 overflow-auto'} aria-busy={!preview && !error}>
      {#if error}
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
    {#if !isPdf}
      <Dialog.Footer class={mediaKind ? 'shrink-0 flex-row flex-wrap justify-end' : 'shrink-0'}>
        <Button variant="outline" onclick={onclose}>Close preview</Button>
        <Button bind:ref={downloadButton} onclick={ondownload}>Download file</Button>
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

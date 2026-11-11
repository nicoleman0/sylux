<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import type { ApiClient } from './api';
  import type { Entry } from './listing';
  import { loadPreview, type Preview } from './preview';

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
  <Dialog.Content class="max-h-[90dvh] overflow-y-auto sm:max-w-3xl" showCloseButton={false} onCloseAutoFocus={(event) => event.preventDefault()}>
    <Dialog.Header>
      <Dialog.Title class="break-all">Preview {entry?.name}</Dialog.Title>
      <Dialog.Description>UTF-8 text, PNG, JPEG and GIF up to 2 MiB. Images must be at most 16 megapixels.</Dialog.Description>
    </Dialog.Header>
    <div class="min-h-24 min-w-0" aria-busy={!preview && !error}>
      {#if error}
        <div class="grid gap-3"><p role="alert">{error}</p><Button variant="outline" class="justify-self-start" onclick={() => attempt++}>Retry preview</Button></div>
      {:else if preview?.kind === 'text'}
        <pre class="bg-muted max-h-[55dvh] overflow-auto rounded-md p-4 font-mono text-sm whitespace-pre-wrap break-words">{preview.text || 'This file is empty.'}</pre>
      {:else if preview?.kind === 'image'}
        <img src={imageUrl} alt={entry?.name ?? 'File preview'} class="mx-auto max-h-[55dvh] max-w-full object-contain" onerror={() => { error = 'This image could not be displayed. Download the file to open it.'; }} />
      {:else if preview?.kind === 'unsupported'}
        <p role="status">{preview.message}</p>
      {:else}
        <p role="status">Loading preview…</p>
      {/if}
    </div>
    <Dialog.Footer><Button variant="outline" onclick={onclose}>Close preview</Button><Button onclick={ondownload}>Download file</Button></Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

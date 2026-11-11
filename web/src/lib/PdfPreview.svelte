<script lang="ts">
  import { getDocument, GlobalWorkerOptions, type PDFDocumentLoadingTask, type PDFDocumentProxy, type RenderTask } from 'pdfjs-dist';
  import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
  import rendererUrl from 'pdfjs-dist/build/pdf.renderer.min.mjs?url';
  import { Button } from '$lib/components/ui/button/index.js';

  let { bytes, onretry }: { bytes: Uint8Array<ArrayBuffer>; onretry: () => void } = $props();
  const assets = import.meta.glob<string>('/node_modules/pdfjs-dist/{cmaps,standard_fonts,wasm}/*.{bcmap,pfb,ttf,wasm,js}', { eager: true, query: '?url&no-inline', import: 'default' });
  const folders: Record<string, string> = { cMapUrl: 'cmaps', standardFontDataUrl: 'standard_fonts', wasmUrl: 'wasm' };
  const MAX_CANVAS_PIXELS = 4_000_000;
  const MAX_CANVAS_SIDE = 4096;
  GlobalWorkerOptions.workerSrc = workerUrl;
  GlobalWorkerOptions.rendererSrc = rendererUrl;

  class LocalBinaryDataFactory {
    signal: AbortSignal;
    constructor(signal: AbortSignal) { this.signal = signal; }
    async fetch({ kind, filename }: { kind: string; filename: string }): Promise<Uint8Array> {
      const url = assets[`/node_modules/pdfjs-dist/${folders[kind]}/${filename}`];
      if (!url) throw new Error('Unavailable PDF resource');
      const response = await fetch(url, { signal: this.signal, credentials: 'same-origin' });
      if (!response.ok) throw new Error('Could not load PDF resource');
      return new Uint8Array(await response.arrayBuffer());
    }
  }

  let pdf = $state.raw<PDFDocumentProxy | null>(null);
  let canvas = $state<HTMLCanvasElement>();
  let containerWidth = $state(0);
  let pageNumber = $state(1);
  let zoom = $state(1);
  let loading = $state(true);
  let error = $state('');
  let limited = $state(false);

  $effect(() => {
    const data = bytes;
    const controller = new AbortController();
    let task: PDFDocumentLoadingTask | undefined;
    let timeout: ReturnType<typeof setTimeout>;
    pdf = null;
    pageNumber = 1;
    zoom = 1;
    loading = true;
    error = '';
    timeout = setTimeout(() => {
      error = 'This PDF took too long to open. Retry or download the file.';
      loading = false;
      controller.abort();
      void task?.destroy();
    }, 20_000);
    try {
      // Copy because PDF.js transfers ownership of the data to its worker.
      task = getDocument({ data: data.slice(), BinaryDataFactory: LocalBinaryDataFactory.bind(null, controller.signal), useWorkerFetch: false, enableXfa: false,
        maxImageSize: 16_000_000, canvasMaxAreaInBytes: MAX_CANVAS_PIXELS * 4, stopAtErrors: true, verbosity: 0 });
      void task.promise.then((document) => {
        if (controller.signal.aborted) return;
        clearTimeout(timeout);
        if (document.numPages > 500) {
          error = 'This PDF exceeds the 500-page preview limit. Download the file to open it.';
          loading = false;
          void task?.destroy();
          return;
        }
        pdf = document;
      }).catch((reason: unknown) => {
        if (controller.signal.aborted) return;
        clearTimeout(timeout);
        loading = false;
        error = reason instanceof Error && reason.name === 'PasswordException'
          ? 'This PDF needs a password. Download the file to open it.'
          : 'Could not open this PDF. Retry or download the file.';
        void task?.destroy();
      });
    } catch {
      clearTimeout(timeout);
      loading = false;
      error = 'Could not open this PDF. Retry or download the file.';
      void task?.destroy();
    }
    return () => {
      clearTimeout(timeout);
      controller.abort();
      void task?.destroy();
    };
  });

  $effect(() => {
    const document = pdf;
    const target = canvas;
    const number = pageNumber;
    const width = containerWidth;
    const magnification = zoom;
    if (!document || !target || width <= 0) return;
    let cancelled = false;
    let render: RenderTask | undefined;
    let page: Awaited<ReturnType<PDFDocumentProxy['getPage']>> | undefined;
    loading = true;
    limited = false;
    error = '';
    const timeout = setTimeout(() => {
      cancelled = true;
      render?.cancel();
      loading = false;
      error = 'This page took too long to display. Retry or download the file.';
    }, 20_000);
    void (async () => {
      page = await document.getPage(number);
      if (cancelled) { page.cleanup(); return; }
      const original = page.getViewport({ scale: 1 });
      if (!(original.width > 0 && original.height > 0 && Number.isFinite(original.width + original.height))) throw new Error('Invalid page dimensions');
      const cssScale = Math.max(1, width - 16) / original.width * magnification;
      const requestedScale = cssScale * Math.min(window.devicePixelRatio || 1, 2);
      const scale = Math.min(requestedScale, Math.sqrt(MAX_CANVAS_PIXELS / (original.width * original.height)), MAX_CANVAS_SIDE / original.width, MAX_CANVAS_SIDE / original.height);
      limited = scale < requestedScale;
      const viewport = page.getViewport({ scale });
      target.width = Math.max(1, Math.floor(viewport.width));
      target.height = Math.max(1, Math.floor(viewport.height));
      target.style.width = `${original.width * cssScale}px`;
      target.style.height = `${original.height * cssScale}px`;
      render = page.render({ canvas: target, viewport });
      await render.promise;
      if (!cancelled) loading = false;
    })().catch(() => {
      if (!cancelled) { loading = false; error = 'Could not display this PDF page. Retry or download the file.'; }
    }).finally(() => {
      clearTimeout(timeout);
      page?.cleanup();
    });
    return () => {
      cancelled = true;
      clearTimeout(timeout);
      render?.cancel();
      target.width = 0;
      target.height = 0;
    };
  });
</script>

<div class="pdf-preview">
  {#if pdf}
    <div class="pdf-controls" aria-label="PDF controls">
      <Button variant="outline" size="sm" disabled={pageNumber <= 1} onclick={() => pageNumber--}>Previous page</Button>
      <span class="page-count" aria-live="polite">Page {pageNumber} of {pdf.numPages}</span>
      <Button variant="outline" size="sm" disabled={pageNumber >= pdf.numPages} onclick={() => pageNumber++}>Next page</Button>
      <div class="zoom-controls">
        <Button variant="outline" size="sm" aria-label="Zoom out" disabled={zoom <= 0.5} onclick={() => zoom = Math.max(0.5, zoom - 0.25)}>−</Button>
        <span class="zoom-value">{Math.round(zoom * 100)}%</span>
        <Button variant="outline" size="sm" aria-label="Zoom in" disabled={zoom >= 3} onclick={() => zoom = Math.min(3, zoom + 0.25)}>+</Button>
        <Button variant="outline" size="sm" onclick={() => zoom = 1}>Fit width</Button>
      </div>
    </div>
  {/if}
  {#if error}
    <div class="pdf-error"><p role="alert">{error}</p><Button variant="outline" onclick={onretry}>Retry preview</Button></div>
  {:else if loading}
    <p role="status">{pdf ? 'Rendering PDF page…' : 'Opening PDF…'}</p>
  {/if}
  <div class="pdf-page" bind:clientWidth={containerWidth} aria-busy={loading} hidden={!!error}>
    <canvas bind:this={canvas} aria-label={`PDF page ${pageNumber}`} hidden={loading}>Download the file to read this PDF.</canvas>
  </div>
  {#if limited && !error}<p class="pdf-note">Preview resolution is reduced for this page. Download for full detail.</p>{/if}
  <p class="pdf-note">One page at a time. PDF text, links and forms are not interactive. Large embedded images may be omitted. Download to read with assistive tools.</p>
</div>

<style>
  .pdf-preview { display: flex; flex-direction: column; gap: 0.75rem; min-width: 0; min-height: 100%; height: 100%; }
  .pdf-controls, .zoom-controls { display: flex; align-items: center; justify-content: center; flex-wrap: wrap; gap: 0.5rem; }
  .page-count, .zoom-value { font-variant-numeric: tabular-nums; font-size: 0.875rem; white-space: nowrap; }
  .zoom-controls { width: 100%; }
  .pdf-page { flex: 1 1 auto; min-height: 0; min-width: 0; overflow: auto; background: var(--muted); border-radius: var(--radius); padding: 0.5rem; }
  .pdf-page canvas { display: block; background: white; margin-inline: auto; }
  .pdf-page canvas[hidden], .pdf-page[hidden] { display: none; }
  .pdf-note { color: var(--muted-foreground); font-size: 0.75rem; line-height: 1.5; }
  .pdf-error { display: grid; gap: 0.75rem; justify-items: start; }
</style>

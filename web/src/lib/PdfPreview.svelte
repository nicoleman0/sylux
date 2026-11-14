<script lang="ts">
  import { AnnotationEditorType, AnnotationMode, getDocument, GlobalWorkerOptions, type PDFDocumentLoadingTask } from 'pdfjs-dist';
  import { EventBus, FindState, PDFFindController, PDFLinkService, PDFViewer, RenderingStates } from 'pdfjs-dist/web/pdf_viewer.mjs';
  import viewerCss from 'pdfjs-dist/web/pdf_viewer.css?inline';
  import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
  import rendererUrl from 'pdfjs-dist/build/pdf.renderer.min.mjs?url';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Minus from '@lucide/svelte/icons/minus';
  import Plus from '@lucide/svelte/icons/plus';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  import { tick } from 'svelte';
  import { Button } from '$lib/components/ui/button/index.js';

  let { bytes, onretry }: { bytes: Uint8Array<ArrayBuffer>; onretry: () => void } = $props();
  const assets = import.meta.glob<string>('/node_modules/pdfjs-dist/{cmaps,standard_fonts,wasm}/*.{bcmap,pfb,ttf,wasm,js}', { eager: true, query: '?url&no-inline', import: 'default' });
  const folders: Record<string, string> = { cMapUrl: 'cmaps', standardFontDataUrl: 'standard_fonts', wasmUrl: 'wasm' };
  const MAX_CANVAS_PIXELS = 4_000_000;
  const DEADLINE_MS = 20_000;
  const scopedViewerCss = `@scope (.pdf-preview) { ${viewerCss.replaceAll(':root', ':scope')} }`;
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

  let container = $state<HTMLDivElement>();
  let pages = $state<HTMLDivElement>();
  let viewer = $state.raw<PDFViewer | null>(null);
  let eventBus = $state.raw<EventBus | null>(null);
  let pageCount = $state(0);
  let pageNumber = $state(1);
  let pageInput = $state<number | undefined>(1);
  let scale = $state(1);
  let scaleValue = $state('auto');
  let opening = $state(true);
  let rendering = $state(false);
  let error = $state('');
  let findOpen = $state(false);
  let findInput = $state<HTMLInputElement>();
  let query = $state('');
  let matches = $state('');
  let navigation = 0;

  $effect(() => {
    const data = bytes;
    const target = container;
    const pageContainer = pages;
    if (!target || !pageContainer) return;
    navigation++;
    const controller = new AbortController();
    const stylesheet = document.createElement('style');
    stylesheet.textContent = scopedViewerCss;
    target.before(stylesheet);
    const timers = new Set<ReturnType<typeof setTimeout>>();
    type RenderSource = { renderingState: number; div?: HTMLDivElement; pageView?: { detailView?: RenderSource | null } };
    type RenderWatch = { source: RenderSource; pageNumber: number; detail: boolean; timer: ReturnType<typeof setTimeout>; checkedAt: number; remaining: number; state: number };
    const renderTimers = new Map<string, RenderWatch>();
    const pendingPages = new Map<number, number>();
    let task: PDFDocumentLoadingTask | undefined;
    let reader: PDFViewer | undefined;
    let stopped = false;
    let resize: ResizeObserver | undefined;
    pageCount = 0;
    pageNumber = pageInput = 1;
    scaleValue = 'auto';
    scale = 1;
    opening = true;
    rendering = false;
    error = '';
    query = matches = '';
    findOpen = false;

    function clear(timer: ReturnType<typeof setTimeout>) {
      clearTimeout(timer);
      timers.delete(timer);
    }
    function deadline(callback: () => void) {
      return schedule(callback, DEADLINE_MS);
    }
    function schedule(callback: () => void, delay: number) {
      const timer = setTimeout(() => { timers.delete(timer); callback(); }, delay);
      timers.add(timer);
      return timer;
    }
    function visible(number: number, source?: RenderSource) {
      if (number === reader?.currentPageNumber) return true;
      const element = reader?.getPageView(number - 1)?.div ?? source?.div;
      if (!element || !target) return false;
      const pageRect = element.getBoundingClientRect();
      const viewport = target.getBoundingClientRect();
      return pageRect.bottom > viewport.top && pageRect.top < viewport.bottom && pageRect.right > viewport.left && pageRect.left < viewport.right;
    }
    function updateBusy() {
      rendering = [...renderTimers.values()].some(watch => watch.source.renderingState === RenderingStates.RUNNING && visible(watch.pageNumber, watch.source))
        || [...pendingPages.keys()].some(number => visible(number));
    }
    function removeWatch(key: string) {
      const watch = renderTimers.get(key);
      if (watch) clear(watch.timer);
      renderTimers.delete(key);
      updateBusy();
    }
    function stop() {
      if (stopped) return;
      stopped = true;
      navigation++;
      resize?.disconnect();
      stylesheet.remove();
      controller.abort();
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
      renderTimers.clear();
      pendingPages.clear();
      reader?.setDocument(null);
      void reader?.l10n?.destroy().catch((reason: unknown) => console.warn('PDF preview localization cleanup failed', reason));
      void task?.destroy().catch((reason: unknown) => console.warn('PDF preview cleanup failed', reason));
    }
    function fail(message: string, reason?: unknown) {
      if (stopped) return;
      if (reason) console.warn('PDF preview failed', reason);
      error = message;
      opening = rendering = false;
      stop();
    }
    const openTimer = deadline(() => fail('This PDF took too long to open. Retry or download the file.'));
    try {
      const bus = new EventBus();
      const links = new PDFLinkService({ eventBus: bus });
      links.externalLinkEnabled = false;
      const find = new PDFFindController({ eventBus: bus, linkService: links });
      // The shipped viewer supports abortSignal; its published option types omit it.
      const options: ConstructorParameters<typeof PDFViewer>[0] & { abortSignal: AbortSignal } = {
        container: target, viewer: pageContainer, eventBus: bus, linkService: links, findController: find,
        abortSignal: controller.signal,
        annotationMode: AnnotationMode.ENABLE, annotationEditorMode: AnnotationEditorType.DISABLE,
        enableAutoLinking: false, maxCanvasPixels: MAX_CANVAS_PIXELS, maxCanvasDim: 4096,
        enableDetailCanvas: true, removePageBorders: true,
      };
      reader = new PDFViewer(options);
      links.setViewer(reader);
      viewer = reader;
      eventBus = bus;
      const listen = (name: string, callback: (...args: any[]) => void) => bus.on(name, callback, { signal: controller.signal });
      listen('pagesinit', () => {
        reader!.currentScaleValue = 'auto';
        pageCount = reader!.pagesCount;
      });
      listen('pagechanging', ({ pageNumber: number }: { pageNumber: number }) => { pageNumber = pageInput = number; updateBusy(); });
      listen('updateviewarea', updateBusy);
      listen('scalechanging', ({ scale: value, presetValue }: { scale: number; presetValue?: string }) => {
        scale = value;
        scaleValue = presetValue || String(value);
      });
      listen('pagerender', ({ source, pageNumber: number }: { source: RenderSource; pageNumber: number }) => {
        const key = `${number}:${source.pageView ? 'detail' : 'page'}`;
        const existing = renderTimers.get(key);
        if (existing) { existing.source = source; updateBusy(); return; }
        const watch = { source, pageNumber: number, detail: !!source.pageView, checkedAt: performance.now(), remaining: DEADLINE_MS, state: RenderingStates.RUNNING } as RenderWatch;
        function checkRender() {
          const current = watch.source;
          const state = current.renderingState;
          if (state === RenderingStates.FINISHED || state === RenderingStates.INITIAL && (!visible(number, current) || watch.detail && !current.pageView?.detailView)) {
            removeWatch(key);
            return;
          }
          const now = performance.now();
          if (watch.state !== RenderingStates.PAUSED) watch.remaining -= now - watch.checkedAt;
          watch.checkedAt = now;
          watch.state = state;
          if (watch.remaining <= 0 && (state === RenderingStates.RUNNING || state === RenderingStates.INITIAL)) {
            fail('This page took too long to display. Retry or download the file.');
            return;
          }
          // A visible page keeps its budget through resize; abandoned views release it.
          updateBusy();
          watch.timer = schedule(checkRender, watch.remaining > 0 ? Math.min(250, watch.remaining) : 250);
        }
        watch.timer = schedule(checkRender, 250);
        renderTimers.set(key, watch);
        updateBusy();
      });
      listen('pagerendered', ({ source, pageNumber: number, cssTransform, error: reason }: { source: RenderSource; pageNumber: number; cssTransform: boolean; error?: unknown }) => {
        if (reason) { fail('Could not display this PDF page. Retry or download the file.', reason); return; }
        if (cssTransform) return;
        const key = `${number}:${source.pageView ? 'detail' : 'page'}`;
        if (renderTimers.get(key)?.source === source) removeWatch(key);
        opening = false;
        clear(openTimer);
      });
      listen('annotationlayerrendered', ({ pageNumber: number }: { pageNumber: number }) => {
        const layer = reader?.getPageView(number - 1)?.annotationLayer?.div;
        if (layer) layer.inert = true;
      });
      listen('textlayerrendered', ({ error: reason }: { error?: unknown }) => {
        if (reason) fail('Could not display this PDF text. Retry or download the file.', reason);
      });
      listen('updatefindmatchescount', ({ matchesCount }: { matchesCount: { current: number; total: number } }) => {
        matches = `${matchesCount.current} of ${matchesCount.total}`;
      });
      listen('updatefindcontrolstate', ({ state, matchesCount }: { state: number; matchesCount: { current: number; total: number } }) => {
        matches = state === FindState.PENDING ? 'Searching…' : state === FindState.NOT_FOUND ? 'No matches' : `${matchesCount.current} of ${matchesCount.total}`;
      });
      // PDF.js transfers the input buffer. Keep the parent's bytes reusable for retries.
      task = getDocument({ data: data.slice(), BinaryDataFactory: LocalBinaryDataFactory.bind(null, controller.signal),
        useWorkerFetch: false, enableXfa: false, disableAutoFetch: true,
        canvasMaxAreaInBytes: MAX_CANVAS_PIXELS * 4, stopAtErrors: true, verbosity: 0 });
      void task.promise.then((document) => {
        if (stopped) return;
        if (document.numPages > 500) { fail('This PDF exceeds the 500-page preview limit. Download the file to open it.'); return; }
        const getPage = document.getPage.bind(document);
        // Page acquisition precedes pagerender, so it needs its own bounded wait.
        document.getPage = async (number) => {
          pendingPages.set(number, (pendingPages.get(number) ?? 0) + 1);
          updateBusy();
          const timer = deadline(() => fail('This page took too long to load. Retry or download the file.'));
          try {
            return await getPage(number);
          } catch (reason) {
            fail('Could not display this PDF page. Retry or download the file.', reason);
            throw reason;
          } finally {
            clear(timer);
            const count = (pendingPages.get(number) ?? 1) - 1;
            if (count) pendingPages.set(number, count); else pendingPages.delete(number);
            if (!stopped) updateBusy();
          }
        };
        links.setDocument(document);
        reader!.setDocument(document);
      }).catch((reason: unknown) => {
        fail(reason instanceof Error && reason.name === 'PasswordException'
          ? 'This PDF needs a password. Download the file to open it.'
          : 'Could not open this PDF. Retry or download the file.', reason);
      });
      resize = new ResizeObserver(() => {
        if (!stopped && reader?.pagesCount) {
          const current = reader.currentScaleValue;
          if (['auto', 'page-width', 'page-fit'].includes(current)) reader.currentScaleValue = current;
          reader.update();
        }
      });
      resize.observe(target);
      for (const event of ['wheel', 'pointerdown', 'keydown']) {
        target.addEventListener(event, () => navigation++, { passive: true, signal: controller.signal });
      }
      window.addEventListener('keydown', keydown, { capture: true, signal: controller.signal });
    } catch (reason) { fail('Could not open this PDF. Retry or download the file.', reason); }
    return () => { stop(); viewer = null; eventBus = null; };
  });

  async function goToPage(number = pageInput) {
    if (!viewer || !number || !Number.isInteger(number) || number < 1 || number > pageCount) {
      pageInput = pageNumber;
      return;
    }
    const reader = viewer;
    const revision = ++navigation;
    const preset = reader.currentScaleValue;
    reader.currentPageNumber = number;
    if (!['auto', 'page-width', 'page-fit'].includes(preset)) return;
    const pageView = reader.getPageView(number - 1);
    if (!pageView.pdfPage) {
      try {
        const page = await reader.pdfDocument?.getPage(number);
        if (!page || revision !== navigation || viewer !== reader) return;
        if (!pageView.pdfPage) pageView.setPdfPage(page);
      } catch (reason) {
        console.warn('PDF navigation failed', reason);
        return;
      }
    }
    if (revision !== navigation || viewer !== reader) return;
    // Preset zoom must use the destination's dimensions in mixed-size documents.
    reader.currentPageNumber = number;
    reader.currentScaleValue = preset;
    reader.currentPageNumber = number;
  }
  function changeZoom(value: string) {
    navigation++;
    if (viewer) viewer.currentScaleValue = value;
  }
  function zoomBy(factor: number) {
    navigation++;
    if (viewer) viewer.currentScale = Math.min(5, Math.max(0.25, viewer.currentScale * factor));
  }
  function search(type = '', backwards = false) {
    navigation++;
    eventBus?.dispatch('find', { source: findInput, type, query, caseSensitive: false, entireWord: false,
      highlightAll: true, findPrevious: backwards, matchDiacritics: false });
    if (!query) matches = '';
  }
  async function showFind() {
    findOpen = true;
    await tick();
    findInput?.focus();
    findInput?.select();
  }
  function closeFind() {
    findOpen = false;
    eventBus?.dispatch('findbarclose', {});
    container?.focus();
  }
  function keydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      event.stopPropagation();
      void showFind();
    } else if (event.key === 'Escape' && findOpen) {
      event.preventDefault();
      event.stopImmediatePropagation();
      closeFind();
    }
  }
</script>

<div class="pdf-preview">
  <div class="pdf-controls" role="toolbar" aria-label="PDF controls">
    <div class="control-group">
      <Button variant="ghost" size="icon-sm" aria-label="Previous page" title="Previous page" disabled={!pageCount || pageNumber <= 1 || !!error} onclick={() => goToPage(pageNumber - 1)}><ChevronUp /></Button>
      <label class="page-label"><span class="sr-only">Page number</span><input aria-label="Page number" type="number" min="1" max={pageCount || 1} bind:value={pageInput} disabled={!pageCount || !!error} onchange={() => goToPage()} onkeydown={(event) => { if (event.key === 'Enter') goToPage(); }} /></label>
      <span class="page-total">of {pageCount || '—'}</span>
      <Button variant="ghost" size="icon-sm" aria-label="Next page" title="Next page" disabled={!pageCount || pageNumber >= pageCount || !!error} onclick={() => goToPage(pageNumber + 1)}><ChevronDown /></Button>
    </div>
    <div class="control-group zoom-controls">
      <Button variant="ghost" size="icon-sm" aria-label="Zoom out" title="Zoom out" disabled={!pageCount || scale <= 0.25 || !!error} onclick={() => zoomBy(1 / 1.25)}><Minus /></Button>
      <select aria-label="PDF zoom" value={scaleValue} disabled={!pageCount || !!error} onchange={(event) => changeZoom(event.currentTarget.value)}>
        {#if !['auto', 'page-width', 'page-fit', '0.5', '0.75', '1', '1.25', '1.5', '2', '3', '4', '5'].includes(scaleValue)}<option value={scaleValue}>{Math.round(scale * 100)}%</option>{/if}
        <option value="auto">Automatic</option><option value="page-width">Fit width</option><option value="page-fit">Fit page</option>
        {#each [0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 5] as value}<option value={String(value)}>{value * 100}%</option>{/each}
      </select>
      <Button variant="ghost" size="icon-sm" aria-label="Zoom in" title="Zoom in" disabled={!pageCount || scale >= 5 || !!error} onclick={() => zoomBy(1.25)}><Plus /></Button>
    </div>
    <Button variant="ghost" size="icon-sm" aria-label="Find in PDF" title="Find in PDF (Ctrl or Command F)" aria-expanded={findOpen} disabled={!pageCount || !!error} onclick={showFind}><Search /></Button>
    <span class="render-status" role="status">{#if rendering && !opening}<LoaderCircle class="motion-safe:animate-spin" aria-hidden="true" /><span class="sr-only">Rendering PDF page…</span>{/if}</span>
  </div>
  {#if findOpen && !error}
    <div class="pdf-find" role="search" aria-label="Find in PDF">
      <input bind:this={findInput} aria-label="Find in PDF" placeholder="Find in document" bind:value={query} oninput={(event) => { query = event.currentTarget.value; search(); }} onkeydown={(event) => { if (event.key === 'Enter') { event.preventDefault(); search('again', event.shiftKey); } }} />
      <span class="find-result" role="status">{matches}</span>
      <Button variant="ghost" size="icon-sm" aria-label="Previous match" title="Previous match" disabled={!query} onclick={() => search('again', true)}><ChevronUp /></Button>
      <Button variant="ghost" size="icon-sm" aria-label="Next match" title="Next match" disabled={!query} onclick={() => search('again')}><ChevronDown /></Button>
      <Button variant="ghost" size="icon-sm" aria-label="Close search" title="Close search" onclick={closeFind}><X /></Button>
    </div>
  {/if}
  <div class="pdf-workspace">
    {#if error}
      <div class="pdf-message pdf-error"><p role="alert">{error}</p><Button variant="outline" onclick={onretry}>Retry preview</Button></div>
    {:else if opening}
      <p class="pdf-message" role="status">Opening PDF…</p>
    {/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to focus the document to scroll.) -->
    <div class="pdf-scroll-container" bind:this={container} tabindex="0" role="region" aria-label="PDF document" aria-busy={opening || rendering} hidden={!!error}>
      <div class="pdfViewer" bind:this={pages}></div>
    </div>
  </div>
</div>

<style>
  .pdf-preview { display: flex; flex-direction: column; min-width: 0; min-height: 0; height: 100%; color: var(--foreground); }
  .pdf-controls, .control-group, .pdf-find { display: flex; align-items: center; gap: 0.25rem; }
  .pdf-controls { flex-wrap: wrap; justify-content: center; gap: 0.5rem; padding: 0.375rem 0.75rem; background: var(--background); border-bottom: 1px solid var(--border); }
  .control-group { min-width: 0; }
  .render-status { display: inline-flex; align-items: center; justify-content: center; width: 1rem; height: 1rem; color: var(--muted-foreground); }
  .render-status :global(svg) { width: 1rem; height: 1rem; }
  .zoom-controls { padding-inline: 0.5rem; border-inline: 1px solid var(--border); }
  input, select { height: var(--control-height); border: 1px solid var(--input); border-radius: var(--radius); background: var(--background); color: var(--foreground); font: inherit; font-size: var(--type-meta); caret-color: var(--primary); }
  input:focus-visible, select:focus-visible, .pdf-scroll-container:focus-visible { outline: 2px solid var(--ring); outline-offset: -2px; }
  input::selection { background: var(--accent); color: var(--accent-foreground); }
  .page-label input { width: 3.25rem; padding: 0 0.375rem; text-align: center; font-variant-numeric: tabular-nums; appearance: textfield; }
  .page-label input::-webkit-inner-spin-button { appearance: none; }
  .page-total { white-space: nowrap; color: var(--muted-foreground); font-size: var(--type-meta); font-variant-numeric: tabular-nums; padding-inline: 0.125rem; }
  select { max-width: 8.5rem; padding-inline: 0.375rem; }
  .pdf-find { flex-wrap: wrap; justify-content: center; padding: 0.375rem 0.75rem; background: var(--background); border-bottom: 1px solid var(--border); }
  .pdf-find input { min-width: 7rem; width: min(18rem, 45vw); padding-inline: 0.625rem; }
  .pdf-find input::placeholder { color: var(--muted-foreground); }
  .find-result { min-width: 5.5rem; font-size: var(--type-meta); color: var(--muted-foreground); font-variant-numeric: tabular-nums; }
  .pdf-workspace { position: relative; flex: 1; min-height: 0; min-width: 0; background: var(--muted); }
  .pdf-scroll-container { position: absolute; inset: 0; overflow: auto; overflow-anchor: none; overscroll-behavior: contain; scrollbar-gutter: stable; scrollbar-color: var(--muted-foreground) var(--muted); padding-top: 1rem; }
  .pdf-scroll-container[hidden] { display: none; }
  .pdf-preview :global(.pdfViewer) { --page-bg-color: #fff; --page-margin: 0 auto 1rem; --pdfViewer-padding-bottom: 1rem; color: #000; color-scheme: only light; }
  .pdf-preview :global(.pdfViewer .page) { background-color: #fff; box-shadow: 0 1px 5px rgb(0 0 0 / 0.16); }
  .pdf-preview :global(.pdfViewer .textLayer .highlight) { --highlight-bg-color: rgb(244 193 49 / 0.45); --highlight-selected-bg-color: rgb(29 155 142 / 0.45); }
  .pdf-preview :global(.pdfViewer .annotationLayer), .pdf-preview :global(.pdfViewer .annotationLayer *) { pointer-events: none !important; }
  .pdf-message { position: absolute; z-index: 1; inset: 1rem auto auto 50%; transform: translateX(-50%); width: max-content; max-width: calc(100% - 2rem); margin: 0; padding: 0.75rem 1rem; border-radius: var(--radius); background: var(--background); color: var(--foreground); font-size: var(--type-ui); }
  .pdf-error { display: grid; justify-items: start; gap: 0.75rem; }
  @media (max-width: 480px) { .pdf-controls { gap: 0.25rem; padding-inline: 0.25rem; } .zoom-controls { padding-inline: 0.25rem; } select { max-width: 6.75rem; } .pdf-find { gap: 0.125rem; padding-inline: 0.25rem; } .find-result { min-width: 4.5rem; } }
  @media (pointer: coarse) { .pdf-controls :global(button), .pdf-find :global(button), input, select { min-height: var(--control-height-touch); } .pdf-controls :global(button), .pdf-find :global(button) { min-width: var(--control-height-touch); } }
</style>

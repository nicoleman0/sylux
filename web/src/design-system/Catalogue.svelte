<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { modifiedTime } from '$lib/modified-time';
  import FileIcon from '@lucide/svelte/icons/file';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import MoreHorizontalIcon from '@lucide/svelte/icons/ellipsis';
  import * as Button from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import SortHeader from '$lib/SortHeader.svelte';
  import RefreshButton from '$lib/RefreshButton.svelte';
  import AccountStatus from '$lib/AccountStatus.svelte';
  import AccountOptions from '$lib/AccountOptions.svelte';
  import { Badge } from '$lib/components/ui/badge/index.js';

  type ColorSample = { label: string; token: string; value: string };
  const colorTokens = [
    { label: 'Background', token: '--background' },
    { label: 'Foreground', token: '--foreground' },
    { label: 'Primary', token: '--primary' },
    { label: 'Primary foreground', token: '--primary-foreground' },
    { label: 'Secondary', token: '--secondary' },
    { label: 'Secondary foreground', token: '--secondary-foreground' },
    { label: 'Muted', token: '--muted' },
    { label: 'Muted foreground', token: '--muted-foreground' },
    { label: 'Accent', token: '--accent' },
    { label: 'Destructive', token: '--destructive' },
    { label: 'Border', token: '--border' },
    { label: 'Input', token: '--input' },
    { label: 'Ring', token: '--ring' },
  ];
  const typography = [
    { label: 'Page title', token: '--type-page', sample: 'Your files' },
    { label: 'Compact page title', token: '--type-page-compact', sample: 'Account' },
    { label: 'Section title', token: '--type-section', sample: 'Recent uploads' },
    { label: 'Brand', token: '--type-brand', sample: 'Sylux' },
    { label: 'Interface', token: '--type-ui', sample: 'Name, date modified, and size' },
    { label: 'Metadata', token: '--type-meta', sample: 'Edited just now' },
    { label: 'Note', token: '--type-note', sample: 'Private to your account' },
  ];
  const sampleModified = modifiedTime({ kind: 'file', modified_at: 1767227400 })!;
  let darkMode = $state(false);
  let samples = $state<ColorSample[]>(colorTokens.map((color) => ({ ...color, value: '' })));
  let sampleNotice = $state('');
  let sampleSelected = $state(false);
  let typeSizes = $state<Record<string, string>>({});

  function readColors(): void {
    const styles = getComputedStyle(document.documentElement);
    typeSizes = Object.fromEntries(typography.map(({ token }) => [token, styles.getPropertyValue(token).trim()]));
    samples = colorTokens.map(({ label, token }) => ({
      label,
      token,
      value: styles.getPropertyValue(token).trim(),
    }));
  }

  async function toggleTheme(): Promise<void> {
    darkMode = !darkMode;
    document.documentElement.classList.toggle('dark', darkMode);
    await tick();
    readColors();
  }

  onMount(() => {
    darkMode = document.documentElement.classList.contains('dark');
    readColors();
  });
</script>

<div class="catalogue" data-testid="design-system">
  <header class="catalogue-header">
    <a class="wordmark" href="/" aria-label="Sylux application"><FolderIcon aria-hidden="true" /><span>Sylux</span></a>
    <div class="header-copy">
      <h1>Design system</h1>
    </div>
    <Button.Root variant="outline" aria-pressed={darkMode} onclick={toggleTheme}>
      {darkMode ? 'Use light theme' : 'Use dark theme'}
    </Button.Root>
  </header>

  <main class="catalogue-main">
    <section class="catalogue-section" aria-labelledby="refresh-title">
      <h2 id="refresh-title">Refresh</h2>
      <div class="flex flex-wrap gap-2"><RefreshButton label="Refresh files" busy={false} onclick={() => {}} /><RefreshButton label="Refresh files" busyLabel="Refreshing…" busy={true} onclick={() => {}} /></div>
    </section>
    <section class="catalogue-section" aria-labelledby="sorting-title">
      <h2 id="sorting-title">Sortable headings</h2>
      <div role="table" aria-label="Sorting examples">
        <div role="row" class="flex flex-wrap gap-3 text-muted-foreground">
          <SortHeader label="Name" column="name" sort="name_asc" pendingSort="modified_asc" onclick={() => {}} />
          <SortHeader label="Modified" column="modified" sort="name_asc" pendingSort="modified_asc" onclick={() => {}} />
          <SortHeader label="Size" column="size" sort="name_asc" pendingSort="modified_asc" disabled onclick={() => {}} />
        </div>
      </div>
    </section>
    <section class="catalogue-section" aria-labelledby="type-title">
      <div class="section-heading"><h2 id="type-title">Typography</h2><p>Interface text and column headings stay sans. File data uses JetBrains Mono.</p></div>
      <div class="type-grid">
        {#each typography as item}
          <div class="type-sample" data-type-sample={item.token}>
            <p class="sample-caption">{item.label}<code>{item.token} · {typeSizes[item.token] ?? ""}</code></p>
            <p class="type-specimen" style={`font-family: var(${item.token === '--type-meta' ? '--font-data' : '--font-ui'}); font-size: var(${item.token}); font-weight: var(${["--type-page", "--type-page-compact", "--type-section", "--type-brand"].includes(item.token) ? "--weight-heading" : "--weight-body"});`}>{item.sample}</p>
          </div>
        {/each}
        <div class="type-sample" data-type-sample="--font-code">
          <p class="sample-caption">Code <code>--font-code</code></p>
          <p class="type-specimen code-specimen" style="font-family: var(--font-code);">folder/report.pdf</p>
        </div>
      </div>
    </section>

    <section class="catalogue-section" aria-labelledby="data-type-title">
      <div class="section-heading"><h2 id="data-type-title">Data roles</h2><p>Column headings use the interface font. JetBrains Mono 400 marks dates, sizes, and account roles.</p></div>
      <div class="data-role-samples">
        <div><span class="sample-caption">Column headings</span><div class="list-head"><span>Name</span><span>Modified</span><span>Size</span></div></div>
        <div><span class="sample-caption">File metadata</span><div class="file-row"><span class="file-modified">Jan 1, 2026</span><span class="file-size">1.15 GB</span></div></div>
        <div><span class="sample-caption">Interface text</span><p>File names, navigation, and descriptions keep the interface sans.</p></div>
      </div>
    </section>

    <section class="catalogue-section" aria-labelledby="colors-title">
      <div class="section-heading"><h2 id="colors-title">Semantic colors</h2><p>Values are read from the active theme.</p></div>
      <div class="color-grid">
        {#each samples as color}
          <div class="color-swatch" data-color-token={color.token}>
            <span class="swatch-chip" style={`background-color: var(${color.token});`} aria-hidden="true"></span>
            <span class="swatch-copy"><span>{color.label}</span><code>{color.value}</code></span>
          </div>
        {/each}
      </div>
    </section>

    <section class="catalogue-section" aria-labelledby="controls-title">
      <div class="section-heading"><h2 id="controls-title">Controls</h2><p>Shared controls in their common states.</p></div>
      <div class="control-layout">
        <div class="control-group" aria-label="Button variants">
          <Button.Root>Save changes</Button.Root>
          <Button.Root variant="outline">Open folder</Button.Root>
          <Button.Root variant="secondary">Cancel</Button.Root>
          <Button.Root variant="ghost">More actions</Button.Root>
          <Button.Root variant="destructive">Delete account</Button.Root>
          <Button.Root disabled>Unavailable</Button.Root>
        </div>
        <form class="field-sample" onsubmit={(event) => event.preventDefault()}>
          <div class="field">
            <Label for="sample-name">Account name</Label>
            <Input id="sample-name" value="alex" autocomplete="off" />
          </div>
          <div class="field">
            <Label for="sample-disabled">Disabled field</Label>
            <Input id="sample-disabled" value="Read only" disabled />
          </div>
          <Button.Root type="submit" variant="outline">Save sample</Button.Root>
        </form>
        <div class="badge-group" aria-label="Badge variants">
          <Badge>Administrator</Badge>
          <Badge variant="secondary">Enabled</Badge>
          <Badge variant="outline">User</Badge>
          <Badge variant="destructive">Disabled</Badge>
        </div>
      </div>
    </section>

    <section class="catalogue-section" aria-labelledby="rows-title">
      <div class="section-heading"><h2 id="rows-title">Rows</h2><p>Everyday file and account details.</p></div>
      <div class="sample-rows">
        {#if sampleSelected}<div class="selection-toolbar" role="group" aria-label="Selected sample items"><span>1 selected</span><Button.Root variant="ghost" size="sm" onclick={() => sampleSelected = false}>Clear selection</Button.Root><Button.Root variant="outline" size="sm" onclick={() => sampleNotice = 'Move selected.'}>Move selected</Button.Root></div>{/if}
        <div class="list-head selectable-head modified-head"><span aria-hidden="true"></span><span>Name</span><span class="file-modified">Modified</span><span class="file-size">Size</span><span class="sr-only">Options</span></div>
        <article class="file-row selectable-row modified-row">
          <label class="entry-selection"><input type="checkbox" aria-label="Select Projects sample" bind:checked={sampleSelected} /></label>
          <div class="name-cell"><button type="button" class="item-name" onclick={() => sampleNotice = 'Folder action selected.'}><span class="folder-icon" aria-hidden="true"><FolderIcon /></span><span class="item-label"><span class="filename">Projects</span><span class="mobile-file-size" aria-hidden="true">0 B · —</span></span></button></div>
          <span class="file-modified" title="Modified date not applicable"><span class="sr-only">Modified date not applicable</span><span aria-hidden="true">—</span></span><span class="file-size">0 B</span>
          <div class="options-cell"><Button.Root variant="ghost" size="icon" class="size-11" aria-label="Project actions" onclick={() => sampleNotice = 'File actions selected.'}><MoreHorizontalIcon /></Button.Root></div>
        </article>
        <article class="file-row selectable-row modified-row">
          <label class="entry-selection"><input type="checkbox" aria-label="Select Report.pdf sample" /></label>
          <div class="name-cell"><button type="button" class="item-name" onclick={() => sampleNotice = 'File action selected.'}><span aria-hidden="true"><FileIcon /></span><span class="item-label"><span class="filename">Report.pdf</span><span class="mobile-file-size" aria-hidden="true">42 KB · <time datetime={sampleModified.datetime} title={sampleModified.full}>{sampleModified.compact}</time></span></span></button></div>
          <span class="file-modified"><time datetime={sampleModified.datetime} title={sampleModified.full}><span aria-hidden="true">{sampleModified.compact}</span><span class="sr-only">Modified {sampleModified.full}</span></time></span><span class="file-size">42 KB</span>
          <div class="options-cell"><Button.Root variant="ghost" size="icon" class="size-11" aria-label="Report actions" onclick={() => sampleNotice = 'File actions selected.'}><MoreHorizontalIcon /></Button.Root></div>
        </article>
        <article class="file-row selectable-row modified-row">
          <label class="entry-selection"><input type="checkbox" aria-label="Select quarterly results sample" /></label>
          <div class="name-cell"><button type="button" class="item-name" title="quarterly-results-export_2026-11-14_final-review-approved.pdf" onclick={() => sampleNotice = 'File action selected.'}><span aria-hidden="true"><FileIcon /></span><span class="item-label"><span class="filename">quarterly-results-export_2026-11-14_final-review-approved.pdf</span><span class="mobile-file-size" aria-hidden="true">1.15 GB · <time datetime={sampleModified.datetime} title={sampleModified.full}>{sampleModified.compact}</time></span></span></button></div>
          <span class="file-modified"><time datetime={sampleModified.datetime} title={sampleModified.full}><span aria-hidden="true">{sampleModified.compact}</span><span class="sr-only">Modified {sampleModified.full}</span></time></span><span class="file-size">1.15 GB</span>
          <div class="options-cell"><Button.Root variant="ghost" size="icon" class="size-11" aria-label="Quarterly results file actions" onclick={() => sampleNotice = 'File actions selected.'}><MoreHorizontalIcon /></Button.Root></div>
        </article>
        <div class="account-row">
          <div class="account-identity"><strong>alex</strong></div><span class="account-role">User</span>
          <span><AccountStatus enabled={true} /></span>
          <div class="account-actions"><AccountOptions account={{ account_id: 'alex', handle: 'alex', role: 'user', enabled: true }} onaction={() => sampleNotice = 'Account action selected.'} /></div>
        </div>
      </div>
      <p class="sample-notice" role="status" aria-live="polite">{sampleNotice}</p>
    </section>
  <section class="catalogue-section"><h2>Account actions and status</h2><div class="flex items-center gap-4"><AccountStatus enabled={true} /><AccountStatus enabled={false} /><AccountOptions account={{ account_id: 'example', handle: 'example', role: 'user', enabled: true }} onaction={() => {}} /><AccountOptions account={{ account_id: 'owner', handle: 'owner', role: 'administrator', enabled: true }} self lastAdministrator onaction={() => {}} /></div></section>
</main>
</div>

<style>
  .catalogue { min-height: 100dvh; background: var(--background); color: var(--foreground); font-family: var(--font-ui); }
  .catalogue-header { min-height: 76px; display: flex; align-items: center; gap: 20px; padding: 12px clamp(16px, 4vw, 48px); border-bottom: 1px solid var(--border); }
  .wordmark { display: inline-flex; align-items: center; gap: 8px; color: var(--foreground); text-decoration: none; font-size: var(--type-brand); font-weight: var(--weight-heading); }
  .wordmark :global(svg) { color: var(--primary); width: var(--icon-file); height: var(--icon-file); }
  .header-copy { flex: 1; min-width: 0; }
  .sample-caption { color: var(--muted-foreground); font-size: var(--type-note); margin: 0 0 3px; }
  .data-role-samples { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 18px; }
  .data-role-samples > div { min-width: 0; }
  .data-role-samples .list-head, .data-role-samples .file-row { display: flex; justify-content: space-between; gap: 12px; min-height: 36px; padding: 8px 0; border-bottom: 1px solid var(--border); color: var(--muted-foreground); font-size: var(--type-meta); }
  .data-role-samples p { margin: 8px 0 0; font-size: var(--type-ui); }
  .header-copy h1 { font-size: var(--type-brand); letter-spacing: 0; margin: 0; }
  .catalogue-main { width: min(100%, 1120px); margin: 0 auto; padding: 8px clamp(16px, 4vw, 48px) 40px; }
  .catalogue-section { padding: 24px 0; border-bottom: 1px solid var(--border); }
  .section-heading { display: flex; align-items: baseline; flex-wrap: wrap; gap: 8px 14px; margin-bottom: 14px; }
  .section-heading h2 { font-size: var(--type-section); margin: 0; }
  .section-heading p { color: var(--muted-foreground); font-size: var(--type-meta); margin: 0; }
  .type-grid { display: grid; }
  .type-sample { display: grid; grid-template-columns: 230px minmax(0, 1fr); align-items: center; min-width: 0; min-height: 60px; padding: 10px 0; border-bottom: 1px solid var(--border); }
  .sample-caption { display: grid; gap: 3px; font-size: var(--type-ui); }
  .sample-caption code, .swatch-copy code { color: var(--muted-foreground); font-family: var(--font-code); font-size: var(--type-note); }
  .type-specimen { margin: 0; overflow-wrap: anywhere; }
  .code-specimen { font-size: var(--type-ui); }
  .color-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 210px), 1fr)); gap: 8px; }
  .color-swatch { display: flex; align-items: center; gap: 10px; min-width: 0; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius); }
  .swatch-chip { width: 36px; height: 36px; flex: 0 0 auto; border: 1px solid var(--border); border-radius: calc(var(--radius) / 2); }
  .swatch-copy { display: grid; gap: 2px; min-width: 0; font-size: var(--type-ui); }
  .swatch-copy code { overflow-wrap: anywhere; }
  .control-layout { display: grid; grid-template-columns: minmax(0, 1.2fr) minmax(220px, 1fr); gap: 16px; align-items: start; }
  .control-group, .badge-group { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .field-sample { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; align-items: end; }
  .field { display: grid; gap: 7px; min-width: 0; }
  .field-sample :global(button) { grid-column: 1 / -1; justify-self: start; }
  .badge-group { grid-column: 1 / -1; padding-top: 4px; }
  .sample-rows { max-width: 640px; border-top: 1px solid var(--border); }
  .sample-notice { min-height: 1.5em; margin: 8px 0 0; color: var(--muted-foreground); font-size: var(--type-meta); }
  @media (max-width: 680px) {
    .type-sample { grid-template-columns: minmax(0, 1fr); gap: 8px; }
    .data-role-samples { grid-template-columns: 1fr; gap: 12px; }
    .catalogue-header { gap: 12px; padding-inline: 16px; }
    .wordmark span { display: none; }
    .catalogue-main { padding-inline: 16px; }
    .control-layout { grid-template-columns: 1fr; }
    .field-sample { grid-template-columns: 1fr; }
    .field-sample :global(button) { grid-column: auto; }
    .badge-group { grid-column: auto; }
  }
</style>

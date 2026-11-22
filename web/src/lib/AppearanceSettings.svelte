<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import BrandLogo from './BrandLogo.svelte';
  import { ApiClient, HttpError } from './api';
  import { appearanceStyle, defaultAppearanceSurfaces, defaultAppearance, imageUpload, readAppearance, type Appearance, type ImageChange } from './appearance';

  let { client, instanceName, dark, busy = $bindable(false), error = $bindable(''), onsaved }: {
    client: ApiClient; instanceName: string; dark: boolean; busy?: boolean; error?: string; onsaved: (appearance: Appearance) => void;
  } = $props();
  let saved = $state<Appearance>({ ...defaultAppearance });
  let draft = $state<Appearance>({ ...defaultAppearance });
  let logo = $state<ImageChange>({ keep: true });
  let loginImage = $state<ImageChange>({ keep: true });
  let loading = $state(true);
  let reading = $state(false);
  let loaded = $state(false);
  let notice = $state('');
  let uncertain = $state(false);
  let fileVersion = $state(0);
  let surfaces = $state<ReturnType<typeof defaultAppearanceSurfaces> | null>(null);
  let baseAccent = $state('');
  let baseBackground = $state('');
  $effect(() => {
    dark;
    surfaces = defaultAppearanceSurfaces();
    const sample = document.createElement('div');
    sample.className = 'appearance-defaults';
    sample.hidden = true;
    document.body.append(sample);
    const styles = getComputedStyle(sample);
    baseAccent = styles.getPropertyValue('--primary').trim();
    baseBackground = styles.getPropertyValue('--background').trim();
    sample.remove();
  });
  let alive = true;
  let imageRead = 0;
  const objectUrls = new Set<string>();
  const changed = $derived(JSON.stringify(draft) !== JSON.stringify(saved) || logo === null || loginImage === null);
  const disabled = $derived(busy || loading || reading || !loaded || uncertain);

  function releaseImages(): void { for (const url of objectUrls) URL.revokeObjectURL(url); objectUrls.clear(); }
  function cancel(): void {
    imageRead++; reading = false; releaseImages(); draft = { ...saved }; logo = { keep: true }; loginImage = { keep: true }; notice = ''; error = ''; fileVersion++;
  }
  async function load(): Promise<void> {
    if (busy) return;
    loading = true; error = ''; notice = '';
    try {
      const appearance = readAppearance(await client.request('/instance/appearance'));
      if (!alive) return;
      saved = appearance; onsaved(appearance); cancel(); uncertain = false; loaded = true;
    } catch { if (alive) error = 'Could not load appearance. Reload to try again.'; }
    finally { if (alive) loading = false; }
  }
  function reset(): void {
    releaseImages(); draft = { ...defaultAppearance }; logo = null; loginImage = null; notice = ''; error = ''; fileVersion++;
  }
  async function choose(event: Event, kind: 'logo' | 'loginImage'): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    const request = ++imageRead; reading = true; error = ''; notice = '';
    try {
      const upload = await imageUpload(file, kind);
      if (!alive || request !== imageRead) return;
      const url = URL.createObjectURL(file); objectUrls.add(url);
      if (kind === 'logo') { logo = upload; draft.logoUrl = url; }
      else { loginImage = upload; draft.loginImageUrl = url; }
    } catch (cause) { if (alive && request === imageRead) error = cause instanceof Error ? cause.message : 'Could not read the image.'; }
    finally { if (alive && request === imageRead) { reading = false; input.value = ''; } }
  }
  async function save(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (disabled || !changed) return;
    busy = true; error = ''; notice = '';
    try {
      const appearance = readAppearance(await client.request('/instance/appearance', { method: 'PUT', body: { accentColor: draft.accentColor, loginBackgroundColor: draft.loginBackgroundColor, logo, loginImage } }));
      if (!alive) return;
      saved = appearance; onsaved(appearance); cancel(); notice = 'Appearance saved.';
    } catch (cause) {
      if (!alive) return;
      if (cause instanceof HttpError && cause.status >= 400 && cause.status < 500) {
        error = cause.status === 403 ? 'You no longer have permission to change appearance.' : 'Appearance was rejected. Check the image type, size and colors, then try again.';
      } else {
        uncertain = true; error = 'Save could not be confirmed. Reload appearance to check what was saved before making another change.';
      }
    } finally { if (alive) busy = false; }
  }
  onMount(() => { void load(); });
  onDestroy(() => { alive = false; imageRead++; releaseImages(); });
</script>

<section class="settings-section" aria-labelledby="appearance-heading">
  <div class="section-heading"><div><h2 id="appearance-heading">Appearance</h2><p class="section-description">Applies to everyone. Images are public on the sign-in page.</p></div></div>
  {#if loading}<p role="status">Loading appearance…</p>{/if}
  <form onsubmit={save} class="appearance-layout" aria-busy={busy}>
    <div class="appearance-fields">
      <fieldset disabled={disabled}>
        <div class="appearance-field"><Label for="accent-color">Accent color</Label><div class="color-control"><Input id="accent-color" type="color" value={draft.accentColor ?? baseAccent} oninput={event => { draft.accentColor = event.currentTarget.value; notice = ''; }} /><span>{draft.accentColor ?? 'Default'}</span><Button type="button" variant="ghost" size="sm" disabled={!draft.accentColor} onclick={() => draft.accentColor = null}>Use default</Button></div><p class="section-description">Adjusted for readable text in each theme.</p></div>
        <div class="appearance-field"><Label for="login-background-color">Login background color</Label><div class="color-control"><Input id="login-background-color" type="color" value={draft.loginBackgroundColor ?? baseBackground} oninput={event => { draft.loginBackgroundColor = event.currentTarget.value; notice = ''; }} /><span>{draft.loginBackgroundColor ?? 'Default'}</span><Button type="button" variant="ghost" size="sm" disabled={!draft.loginBackgroundColor} onclick={() => draft.loginBackgroundColor = null}>Use default</Button></div></div>
        {#key fileVersion}
          <div class="appearance-field"><Label for="appearance-logo">Logo</Label><Input id="appearance-logo" type="file" accept="image/png,image/jpeg,image/webp" onchange={event => choose(event, 'logo')} aria-describedby="logo-help" /><p id="logo-help" class="section-description">PNG, JPEG or WebP. Up to 512 KiB.</p>{#if draft.logoUrl}<Button type="button" variant="ghost" size="sm" class="justify-self-start" onclick={() => { draft.logoUrl = null; logo = null; }}>Remove logo</Button>{/if}</div>
          <div class="appearance-field"><Label for="appearance-login-image">Login image</Label><Input id="appearance-login-image" type="file" accept="image/png,image/jpeg,image/webp" onchange={event => choose(event, 'loginImage')} aria-describedby="image-help" /><p id="image-help" class="section-description">PNG, JPEG or WebP. Up to 2 MiB. No animation.</p>{#if draft.loginImageUrl}<Button type="button" variant="ghost" size="sm" class="justify-self-start" onclick={() => { draft.loginImageUrl = null; loginImage = null; }}>Remove login image</Button>{/if}</div>
        {/key}
      </fieldset>
      {#if reading}<p role="status">Reading image…</p>{/if}
      <div class="appearance-actions"><Button type="submit" disabled={disabled || !changed} aria-busy={busy}>{busy ? 'Saving…' : 'Save appearance'}</Button><Button type="button" variant="outline" disabled={disabled || !changed} onclick={cancel}>Cancel</Button><Button type="button" variant="ghost" disabled={disabled} onclick={reset}>Reset defaults</Button></div>
    </div>
    <section aria-label="Appearance preview" class="appearance-preview-column">
      <h3>Preview</h3><p class="section-description">Only you see these changes until you save.</p>
      <div class="appearance-preview appearance-defaults" style={surfaces ? appearanceStyle(draft, dark, surfaces) : ''}>
        <div class="preview-header"><BrandLogo url={draft.logoUrl} /><span>{instanceName}</span></div>
        <div class="preview-background" style:background-color={draft.loginBackgroundColor ?? 'var(--background)'} style:background-image={draft.loginImageUrl ? `url("${draft.loginImageUrl}")` : 'none'}>
          <div class="preview-form"><h4>Sign in to {instanceName}</h4><span>Account name</span><div class="preview-input"></div><span>Password</span><div class="preview-input"></div><span class="preview-submit">Sign in</span></div>
        </div>
      </div>
    </section>
  </form>
  {#if error}<p role="alert" class="form-notice">{error}</p>{/if}
  {#if !loaded || uncertain}<Button variant="outline" class="mt-3" disabled={loading || busy} onclick={load}>Reload appearance</Button>{/if}
  {#if notice}<p role="status" class="form-notice">{notice}</p>{/if}
</section>

<style>
  .appearance-layout { display: grid; grid-template-columns: minmax(0, 1fr) minmax(240px, 1fr); gap: 32px; max-width: 960px; margin-top: 24px; }
  fieldset { display: grid; gap: 24px; min-width: 0; border: 0; padding: 0; }
  .appearance-field { display: grid; gap: 8px; min-width: 0; }
  .color-control { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  .color-control :global(input) { width: 44px; padding: 3px; cursor: pointer; }
  .appearance-actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 24px; }
  h3 { font-weight: var(--weight-heading); margin-bottom: 4px; }
  .appearance-preview { border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; margin-top: 12px; color: var(--foreground); }
  .preview-header { display: flex; align-items: center; gap: 10px; padding: 12px; background: var(--background); font-weight: var(--weight-heading); }
  .preview-header span { overflow-wrap: anywhere; }
  .preview-header :global(svg) { color: var(--primary); width: 25px; height: 25px; flex-shrink: 0; }
  .preview-background { padding: 30px 16px; background-size: cover; background-position: center; }
  .preview-form { display: grid; gap: 10px; background: var(--background); color: var(--foreground); padding: 20px; border-radius: var(--radius); }
  h4 { font-size: var(--type-brand); font-weight: var(--weight-heading); margin-bottom: 8px; overflow-wrap: anywhere; }
  .preview-input { height: var(--input-height); border: 1px solid var(--input); border-radius: calc(var(--radius) - 2px); }
  .preview-submit { background: var(--primary); color: var(--primary-foreground); border-radius: calc(var(--radius) - 2px); padding: 8px; text-align: center; margin-top: 8px; font-weight: var(--weight-control); }
  @media (max-width: 900px) { .appearance-layout { grid-template-columns: minmax(0, 1fr); } }
</style>

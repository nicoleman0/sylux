<script lang="ts">
  import { onMount, tick } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { HttpError, type ApiClient } from './api';
  import type { Entry } from './listing';

  type Link = { kind: 'file' | 'folder'; id: string; entry_id: string; name: string; created_at: number; expires_at: number };
  type CreatedLink = Link & { url: string };

  let { client, entry, onclose, busy = $bindable(false) }: { client: ApiClient; entry: Entry | null; onclose: () => void; busy?: boolean } = $props();
  let days = $state(7);
  let links = $state<Link[] | null>(null);
  let created = $state<CreatedLink | null>(null);
  let loading = $state(true);
  let revoking = $state<string | null>(null);
  let listError = $state(false);
  let error = $state('');
  let status = $state('');
  let urlField = $state<HTMLInputElement | null>(null);
  let done = $state<HTMLButtonElement | null>(null);
  const visibleLinks = $derived(links?.filter(link => !entry || link.entry_id === entry.id));
  const dates = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'medium' });
  const controller = new AbortController();

  onMount(() => {
    void load();
    return () => controller.abort();
  });

  async function load(): Promise<void> {
    loading = true;
    listError = false;
    try {
      links = await client.request<Link[]>('/share-links', { signal: controller.signal });
    } catch {
      if (!controller.signal.aborted) listError = true;
    } finally {
      loading = false;
    }
  }

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!entry || busy || loading) return;
    busy = true;
    error = '';
    status = '';
    created = null;
    try {
      const result = await client.request<CreatedLink>('/share-links', {
        method: 'POST',
        body: { entry_id: entry.id, expiry_days: days },
        signal: controller.signal,
      });
      if (controller.signal.aborted) return;
      created = result;
      if (links) links = [result, ...links];
      else await load();
      await tick();
      urlField?.focus();
    } catch (failure) {
      if (controller.signal.aborted) return;
      if (failure instanceof HttpError && failure.code === 'share_link_limit') {
        error = 'You have 50 active links. Revoke one in Account to create another.';
      } else if (failure instanceof HttpError && failure.code === 'entry_not_found') {
        error = 'This item is no longer available to share.';
      } else if (failure instanceof HttpError && failure.status >= 400 && failure.status < 500) {
        error = 'Could not create the link. Check your access and try again.';
      } else {
        error = 'Could not confirm the new link. Check Active links before creating another. An unconfirmed link cannot be copied; revoke it if it appears.';
        await load();
      }
    } finally {
      busy = false;
    }
  }

  async function copyLink(): Promise<void> {
    if (!created || busy) return;
    error = '';
    try {
      await navigator.clipboard.writeText(created.url);
      status = 'Link copied.';
    } catch {
      urlField?.focus();
      urlField?.select();
      status = 'Copy the selected link.';
    }
  }

  async function revoke(link: Link, button: HTMLElement): Promise<void> {
    if (busy || loading) return;
    busy = true;
    revoking = link.id;
    error = '';
    status = '';
    try {
      await client.request<void>(`/share-links/${link.id}`, { method: 'DELETE', signal: controller.signal });
      if (controller.signal.aborted) return;
      const focused = document.activeElement === button;
      if (created?.id === link.id) created = null;
      links = links?.filter(item => item.id !== link.id) ?? null;
      status = 'Link revoked.';
      await tick();
      if (focused) done?.focus();
    } catch {
      if (!controller.signal.aborted) error = 'Could not confirm revocation. Select Revoke again to make sure the link is removed.';
    } finally {
      busy = false;
      revoking = null;
    }
  }
</script>

<Dialog.Root open onOpenChange={(open) => { if (!open && !busy) onclose(); }}>
  <Dialog.Content
    class="max-h-[calc(100dvh-2rem)] min-w-0 gap-4 overflow-y-auto sm:max-w-lg"
    showCloseButton={false}
    onEscapeKeydown={(event) => { if (busy) event.preventDefault(); }}
    onInteractOutside={(event) => { if (busy) event.preventDefault(); }}
    onCloseAutoFocus={(event) => event.preventDefault()}
  >
    <Dialog.Header class="min-w-0 text-left">
      <Dialog.Title class="break-all">{entry ? `Share ${entry.name}` : 'Shared links'}</Dialog.Title>
      <Dialog.Description>
        {#if entry?.kind === 'folder'}
          Anyone with the link can browse this folder and download individual files. Files you add, restore or replace become available until the link expires or you revoke it.
        {:else if entry}
          Anyone with a link can download the file until the link expires or you revoke it.
        {:else}
          Review public links to files and folders.
        {/if}
      </Dialog.Description>
    </Dialog.Header>

    {#if entry}
      <form class="grid min-w-0 gap-3" aria-busy={busy && !revoking} onsubmit={create}>
        <div class="flex flex-wrap items-end gap-2">
          <div class="grid gap-2">
            <Label for="share-expiry">Expires after</Label>
            <select id="share-expiry" class="border-input bg-background text-foreground focus-visible:ring-ring h-[var(--input-height)] rounded-md border px-3 focus-visible:ring-2" bind:value={days} disabled={busy}>
              <option value={1}>1 day</option>
              <option value={7}>7 days</option>
              <option value={30}>30 days</option>
            </select>
          </div>
          <Button type="submit" aria-disabled={busy || loading} aria-busy={busy && !revoking}>
            {busy && !revoking ? 'Creating…' : 'Create link'}
          </Button>
        </div>
        {#if created}
          <div class="grid min-w-0 gap-2">
            <Label for="share-url">{entry.kind === 'folder' ? 'Folder link' : 'Download link'}</Label>
            <div class="flex min-w-0 flex-wrap gap-2">
              <Input id="share-url" class="min-w-0 flex-1" readonly value={created.url} bind:ref={urlField} />
              <Button type="button" variant="outline" disabled={busy} onclick={copyLink}>Copy link</Button>
            </div>
            <p class="text-xs text-muted-foreground">Copy this address before closing. You can revoke it later from Active links.</p>
          </div>
        {/if}
      </form>
    {/if}

    <section aria-labelledby="share-active-heading" aria-busy={loading}>
      <h3 id="share-active-heading" class="font-medium">Active links</h3>
      {#if loading}
        <p role="status" class="mt-2 text-muted-foreground">Loading links…</p>
      {:else if listError}
        <p role="alert" class="mt-2">Could not load active links.</p>
        <Button type="button" variant="outline" class="mt-2" disabled={busy} onclick={load}>Try again</Button>
      {:else if !visibleLinks?.length}
        <p class="mt-2 text-muted-foreground">No active links.</p>
      {:else}
        <ul class="mt-1 divide-y divide-border">
          {#each visibleLinks as link, index (link.id)}
            <li class="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 py-2">
              <div class="min-w-0 break-words">
                {#if !entry}<p class="break-all font-medium">{link.name}</p>{#if link.kind === 'folder'}<p class="text-xs text-muted-foreground">Folder</p>{/if}{/if}
                <p class="text-xs text-muted-foreground">Created {dates.format(link.created_at * 1000)}</p>
                <p class="text-xs text-muted-foreground">Expires {dates.format(link.expires_at * 1000)}</p>
              </div>
              <Button type="button" variant="outline" aria-disabled={busy} aria-busy={revoking === link.id}
                aria-label={`Revoke link ${index + 1} for ${link.name}`}
                onclick={(event) => void revoke(link, event.currentTarget)}>
                {revoking === link.id ? 'Revoking…' : 'Revoke'}
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
    {#if error}<p role="alert" class="text-destructive">{error}</p>{/if}
    <p role="status" class:sr-only={!status}>{status}</p>
    <Dialog.Footer>
      <Button type="button" variant="outline" bind:ref={done} aria-disabled={busy} onclick={() => { if (!busy) onclose(); }}>Done</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

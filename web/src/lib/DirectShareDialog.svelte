<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { HttpError, type ApiClient } from './api';
  import { DirectSharingApi, type DirectGrant } from './direct-grants';
  import type { Folder } from './listing';

  let { client, folder, busy = $bindable(''), onclose }: {
    client: ApiClient; folder: Folder; busy?: string; onclose: () => void;
  } = $props();
  const sharing = $derived(new DirectSharingApi(client));
  let grants = $state<DirectGrant[]>([]);
  let handle = $state('');
  let error = $state('');
  let notice = $state('');
  let loaded = $state(false);
  let publicationId = $state<string | null>(null);
  let confirmingPublication = $state(false);
  const controller = new AbortController();

  onMount(() => {
    void load();
    return () => controller.abort();
  });

  async function load(): Promise<void> {
    busy = 'load'; error = ''; notice = '';
    try {
      const response = await sharing.grants(folder.id, controller.signal);
      if (!controller.signal.aborted) {
        grants = response.items;
        publicationId = response.publication_id;
        loaded = true;
      }
    } catch {
      if (!controller.signal.aborted) { loaded = false; error = 'Could not load access. Try again.'; }
    } finally { busy = ''; }
  }

  async function add(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy || !loaded) return;
    const target = handle.trim();
    if (!target) return;
    busy = 'add'; error = ''; notice = '';
    try {
      const saved = await sharing.grant(folder.id, target, controller.signal);
      if (controller.signal.aborted) return;
      grants = [...grants.filter(grant => grant.id !== saved.id), saved].sort((a, b) => a.handle.localeCompare(b.handle));
      handle = ''; notice = `Shared with ${saved.handle}.`;
    } catch (failure) {
      if (controller.signal.aborted) return;
      if (failure instanceof HttpError && failure.status === 404) {
        error = 'Could not share. Check the account name and make sure the folder is available.';
      } else {
        loaded = false; error = 'Could not confirm the share. Refresh access before trying again.';
      }
    } finally { busy = ''; }
  }

  async function revoke(grant: DirectGrant): Promise<void> {
    if (busy || !loaded) return;
    const trigger = document.activeElement;
    const restoreFocus = trigger?.getAttribute('aria-label') === `Revoke access for ${grant.handle}`;
    busy = grant.id; error = ''; notice = '';
    try {
      await sharing.revoke(grant.id, controller.signal);
      if (controller.signal.aborted) return;
      grants = grants.filter(item => item.id !== grant.id);
      notice = `Revoked access for ${grant.handle}.`;
    } catch (failure) {
      if (controller.signal.aborted) return;
      if (!(failure instanceof HttpError) || failure.status === 404 || failure.status >= 500) {
        try {
          const response = await sharing.grants(folder.id, controller.signal);
          if (controller.signal.aborted) return;
          grants = response.items;
          publicationId = response.publication_id;
          notice = grants.some(item => item.handle === grant.handle)
            ? `${grant.handle} still has direct access to this folder.`
            : `${grant.handle} has no direct access to this folder.`;
        } catch {
          if (controller.signal.aborted) return;
          loaded = false;
        }
      } else {
        loaded = false;
      }
      if (!loaded) error = 'Could not confirm revocation. Refresh access before trying again.';
    } finally { busy = ''; }
    if (restoreFocus) {
      await tick();
      if (!controller.signal.aborted && (document.activeElement === document.body || document.activeElement === trigger)) {
        const retry = loaded ? grants.find(item => item.id === grant.id) : undefined;
        if (retry && trigger instanceof HTMLElement) trigger.focus();
        else document.getElementById(loaded ? 'direct-share-handle' : 'direct-share-refresh')?.focus();
      }
    }
  }

  async function reviewPublication(): Promise<void> {
    if (busy || !loaded) return;
    error = ''; notice = '';
    confirmingPublication = true;
    await tick();
    document.getElementById('cancel-publication')?.focus();
  }

  async function cancelPublication(): Promise<void> {
    confirmingPublication = false;
    await focusAccessAction();
  }

  async function focusAccessAction(): Promise<void> {
    await tick();
    if (!controller.signal.aborted) {
      const target = !loaded ? 'direct-share-refresh' : folder.parent_id === null ? 'review-publication' : 'direct-share-handle';
      document.getElementById(target)?.focus();
    }
  }

  async function changePublication(): Promise<void> {
    if (!confirmingPublication || busy || !loaded) return;
    busy = 'publication';
    const wasPublished = publicationId;
    try {
      if (wasPublished) {
        await sharing.withdraw(wasPublished, controller.signal);
        publicationId = null;
        notice = 'Publication withdrawn.';
      } else {
        const result = await sharing.publish(folder.id, controller.signal);
        publicationId = result.id;
        notice = 'Library published.';
      }
    } catch {
      if (!controller.signal.aborted) {
        loaded = false;
        error = 'Could not confirm the change. Refresh access before trying again.';
      }
    } finally {
      confirmingPublication = false;
      busy = '';
    }
    await focusAccessAction();
  }
</script>

<Dialog.Root open onOpenChange={(open) => { if (!open && !busy) onclose(); }}>
  <Dialog.Content class="max-h-[85dvh] overflow-y-auto" aria-busy={!!busy} showCloseButton={!busy && !confirmingPublication}
    escapeKeydownBehavior={busy ? 'ignore' : 'close'} interactOutsideBehavior={busy || confirmingPublication ? 'ignore' : 'close'}
    onEscapeKeydown={(event) => {
      if (confirmingPublication && !busy) {
        event.preventDefault();
        void cancelPublication();
      }
    }}
    onCloseAutoFocus={(event) => event.preventDefault()}>
    <Dialog.Header>
      <Dialog.Title class="[overflow-wrap:anywhere]">
        {confirmingPublication ? publicationId ? `Withdraw publication of ${folder.name}?` : `Publish ${folder.name}?` : `Share ${folder.name}`}
      </Dialog.Title>
      <Dialog.Description>
        {#if confirmingPublication}
          {#if publicationId}
            This library will disappear from Shared with all. Any separate direct shares stay in place.
          {:else}
            Everyone signed in now or in the future can browse, search, preview PDFs and download individual files in this library, including files added later.
          {/if}
        {:else}
          Accounts you add can browse, search and download files in this {folder.parent_id === null ? 'library' : 'folder'}. They cannot change anything.
        {/if}
      </Dialog.Description>
    </Dialog.Header>
    {#if confirmingPublication}
      <Dialog.Footer>
        <Button id="cancel-publication" variant="outline" disabled={!!busy} onclick={cancelPublication}>Cancel</Button>
        <Button aria-disabled={!!busy} aria-busy={busy === 'publication'} onclick={changePublication}>
          {publicationId ? (busy ? 'Withdrawing…' : 'Withdraw publication') : (busy ? 'Publishing…' : 'Publish library')}
        </Button>
      </Dialog.Footer>
    {:else if loaded}
      {#if folder.parent_id === null}
        <section class="grid gap-2 border-b pb-4" aria-labelledby="library-publication-heading">
          <h2 id="library-publication-heading" class="font-medium">Shared with all</h2>
          <p class="text-muted-foreground">{publicationId ? 'Read-only access for everyone signed in.' : 'This library is unpublished.'}</p>
          <Button id="review-publication" variant="outline" class="justify-self-start" disabled={!!busy} onclick={reviewPublication}>
            {publicationId ? 'Withdraw publication…' : 'Publish library…'}
          </Button>
        </section>
      {/if}
      <form class="grid gap-2" onsubmit={add}>
        <Label for="direct-share-handle">Account name</Label>
        <div class="flex flex-wrap gap-2">
          <Input id="direct-share-handle" class="min-w-36 flex-1" bind:value={handle} maxlength={64} disabled={!!busy} required />
          <Button type="submit" disabled={!!busy || !handle.trim()} aria-busy={busy === 'add'}>{busy === 'add' ? 'Sharing…' : 'Share'}</Button>
        </div>
      </form>
      {#if grants.length}
        <ul aria-label="Accounts with direct access" class="max-h-48 divide-y overflow-auto">
          {#each grants as item (item.id)}
            <li class="flex min-h-11 items-center justify-between gap-3 py-1">
              <span class="min-w-0 [overflow-wrap:anywhere]">{item.handle}<span class="block text-xs text-muted-foreground">Read only</span></span>
              <Button variant="ghost" size="sm" aria-disabled={!!busy} aria-busy={busy === item.id} aria-label={`Revoke access for ${item.handle}`} onclick={() => revoke(item)}>{busy === item.id ? 'Revoking…' : 'Revoke'}</Button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if busy === 'load'}
      <p role="status">Loading access…</p>
    {/if}
    {#if !confirmingPublication}
      {#if error}<p role="alert">{error}</p>{/if}
      {#if notice}<p role="status" class="[overflow-wrap:anywhere]">{notice}</p>{/if}
      <Dialog.Footer>
        {#if !loaded && error}<Button id="direct-share-refresh" variant="outline" disabled={!!busy} onclick={async () => { await load(); await focusAccessAction(); }}>Refresh access</Button>{/if}
        <Button variant="outline" disabled={!!busy} onclick={onclose}>Close</Button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>

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
  const controller = new AbortController();

  onMount(() => {
    void load();
    return () => controller.abort();
  });

  async function load(): Promise<void> {
    busy = 'load'; error = ''; notice = '';
    try {
      const response = await sharing.grants(folder.id, controller.signal);
      if (!controller.signal.aborted) { grants = response.items; loaded = true; }
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
    busy = grant.id; error = ''; notice = '';
    let focusLost = false;
    try {
      await sharing.revoke(grant.id, controller.signal);
      if (controller.signal.aborted) return;
      focusLost = document.activeElement?.getAttribute('aria-label') === `Revoke access for ${grant.handle}`;
      grants = grants.filter(item => item.id !== grant.id);
      notice = `Revoked access for ${grant.handle}.`;
    } catch {
      if (controller.signal.aborted) return;
      loaded = false; error = 'Could not confirm revocation. Refresh access before trying again.';
    } finally { busy = ''; }
    if (focusLost) {
      await tick();
      document.getElementById('direct-share-handle')?.focus();
    }
  }
</script>

<Dialog.Root open onOpenChange={(open) => { if (!open && !busy) onclose(); }}>
  <Dialog.Content aria-busy={!!busy} showCloseButton={!busy}
    escapeKeydownBehavior={busy ? 'ignore' : 'close'} interactOutsideBehavior={busy ? 'ignore' : 'close'}
    onCloseAutoFocus={(event) => event.preventDefault()}>
    <Dialog.Header>
      <Dialog.Title class="[overflow-wrap:anywhere]">Share {folder.name}</Dialog.Title>
      <Dialog.Description>Accounts you add can browse, search and download files in this folder. They cannot change anything.</Dialog.Description>
    </Dialog.Header>
    {#if loaded}
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
              <Button variant="ghost" size="sm" disabled={!!busy} aria-busy={busy === item.id} aria-label={`Revoke access for ${item.handle}`} onclick={() => revoke(item)}>{busy === item.id ? 'Revoking…' : 'Revoke'}</Button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if busy === 'load'}
      <p role="status">Loading access…</p>
    {/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if notice}<p role="status" class="[overflow-wrap:anywhere]">{notice}</p>{/if}
    <Dialog.Footer>
      {#if !loaded && error}<Button variant="outline" disabled={!!busy} onclick={load}>Refresh access</Button>{/if}
      <Button variant="outline" disabled={!!busy} onclick={onclose}>Close</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

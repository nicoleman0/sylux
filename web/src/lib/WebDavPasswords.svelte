<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { HttpError, type ApiClient } from './api';

  type AppPassword = { id: string; name: string; read_only: boolean };

  let { client, handle, active, available = $bindable(false), open = $bindable(false), onrestorefocus }:
    { client: ApiClient; handle: string; active: boolean; available?: boolean; open?: boolean; onrestorefocus: () => void } = $props();

  let passwords = $state<AppPassword[]>([]);
  let name = $state('');
  let readOnly = $state(false);
  let created = $state<string | null>(null);
  let pending = $state<'create' | string | null>(null);
  let status = $state('');
  let failure = $state('');
  let nameField = $state<HTMLInputElement | null>(null);
  let passwordField = $state<HTMLInputElement | null>(null);
  const lifetime = new AbortController();

  onDestroy(() => lifetime.abort());

  $effect(() => {
    if (active) void load();
    else status = '';
  });

  async function load(): Promise<void> {
    try {
      const list = await client.request<AppPassword[]>('/me/webdav', { signal: lifetime.signal });
      if (lifetime.signal.aborted) return;
      passwords = list;
      available = true;
    } catch (error) {
      if (lifetime.signal.aborted) return;
      if (error instanceof HttpError && error.status === 404) available = false;
      else failure = 'Could not load app passwords.';
    }
  }

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (pending || !name.trim()) return;
    pending = 'create';
    status = '';
    failure = '';
    try {
      const result = await client.request<{ credential: AppPassword; password: string }>('/me/webdav', {
        method: 'POST',
        body: { name: name.trim(), read_only: readOnly },
        signal: lifetime.signal,
      });
      if (lifetime.signal.aborted) return;
      passwords = [...passwords, result.credential];
      name = '';
      readOnly = false;
      created = result.password;
      await tick();
      passwordField?.focus();
    } catch (error) {
      if (lifetime.signal.aborted) return;
      if (error instanceof HttpError && error.status === 409) {
        failure = 'Use a different name or revoke an unused app password.';
      } else {
        await load();
        failure = 'Could not confirm creation. If the name appears in the list, revoke it before trying again.';
      }
    } finally {
      pending = null;
    }
  }

  async function revoke(password: AppPassword): Promise<void> {
    if (pending) return;
    pending = password.id;
    status = '';
    failure = '';
    try {
      await client.request(`/me/webdav/${password.id}`, { method: 'DELETE', signal: lifetime.signal });
      if (lifetime.signal.aborted) return;
      passwords = passwords.filter(item => item.id !== password.id);
      status = 'App password revoked.';
    } catch {
      if (lifetime.signal.aborted) return;
      await load();
      failure = 'Could not confirm revocation. Check the list before trying again.';
    } finally {
      pending = null;
      await tick();
      nameField?.focus();
    }
  }

  async function forget(): Promise<void> {
    created = null;
    await tick();
    nameField?.focus();
  }

  function close(): void {
    if (pending || created) return;
    open = false;
    status = '';
    failure = '';
    name = '';
    readOnly = false;
  }
</script>

<Dialog.Root {open} onOpenChange={(show) => { if (!show) close(); }}>
  <Dialog.Content showCloseButton={!pending && !created}
    onEscapeKeydown={(event) => { if (pending || created) event.preventDefault(); }}
    onInteractOutside={(event) => { if (pending || created) event.preventDefault(); }}
    onCloseAutoFocus={(event) => { event.preventDefault(); onrestorefocus(); }}>
    <Dialog.Header>
      <Dialog.Title>WebDAV app passwords</Dialog.Title>
      <Dialog.Description>Use a separate app password in each client.</Dialog.Description>
    </Dialog.Header>

    {#if created}
      <div class="grid gap-3">
        <dl class="account-details">
          <dt>Server URL</dt><dd class="break-all">{window.location.origin}/dav/</dd>
          <dt>Username</dt><dd class="break-all">{handle}</dd>
        </dl>
        <Label for="webdav-password">App password</Label>
        <Input id="webdav-password" bind:ref={passwordField} value={created} readonly autocomplete="off" />
        <p class="section-description">Copy the password now. You can only see it once.</p>
      </div>
      <Dialog.Footer><Button onclick={() => void forget()}>Done</Button></Dialog.Footer>
    {:else}
      {#if passwords.length}
        <ul aria-label="Active app passwords" class="grid gap-2 max-h-52 overflow-auto">
          {#each passwords as password (password.id)}
            <li class="flex items-center justify-between gap-3 border-b pb-2">
              <div class="min-w-0"><div class="break-words">{password.name}</div>
                <div class="text-xs text-muted-foreground">{password.read_only ? 'Read only' : 'Read and write'}</div></div>
              <Button type="button" variant="outline" size="sm" disabled={!!pending}
                aria-busy={pending === password.id} aria-label={`Revoke ${password.name}`}
                onclick={() => void revoke(password)}>{pending === password.id ? 'Revoking…' : 'Revoke'}</Button>
            </li>
          {/each}
        </ul>
      {/if}
      <form class="grid gap-3" aria-busy={pending === 'create'} onsubmit={create}>
        <div class="grid gap-2">
          <Label for="webdav-app-name">App name</Label>
          <Input id="webdav-app-name" bind:ref={nameField} bind:value={name} maxlength={80}
            autocomplete="off" required disabled={!!pending} />
        </div>
        <label class="flex min-h-10 items-center gap-2 text-sm">
          <input type="checkbox" bind:checked={readOnly} disabled={!!pending} />
          Read only
        </label>
        {#if failure}<p role="alert">{failure}</p>{/if}
        {#if status}<p role="status">{status}</p>{/if}
        <Dialog.Footer>
          <Button type="button" variant="outline" disabled={!!pending} onclick={close}>Close</Button>
          <Button type="submit" disabled={!!pending || !name.trim()} aria-busy={pending === 'create'}>
            {pending === 'create' ? 'Creating…' : 'Create app password'}
          </Button>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>

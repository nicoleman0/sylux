<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { HttpError } from './api';
  import type { Account, OidcLink, SessionApi } from './session';
  import RefreshButton from './RefreshButton.svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';

  let { session, accounts, onrevoked }: {
    session: SessionApi;
    accounts: Account[];
    onrevoked: () => Promise<void>;
  } = $props();

  let links = $state<OidcLink[]>([]);
  let loading = $state(false);
  let busy = $state(false);
  let loadError = $state(false);
  let error = $state('');
  let notice = $state('');
  let approving = $state(false);
  let revoking = $state<OidcLink | null>(null);
  let subject = $state('');
  let handle = $state('');
  let selectedId = $state('');
  let verified = $state(false);
  let heading: HTMLHeadingElement | null = null;
  let trigger: HTMLElement | null = null;
  const eligible = $derived(accounts.filter((account) => account.role === 'user' &&
    (account.credential_mode === 'external' ? !account.enabled : account.enabled)));

  onMount(() => { void load(); });

  async function load(): Promise<void> {
    loading = true;
    loadError = false;
    try { links = await session.oidcLinks(); }
    catch { loadError = true; }
    finally { loading = false; }
  }

  function openApproval(event: MouseEvent & { currentTarget: HTMLElement }): void {
    trigger = event.currentTarget;
    subject = ''; handle = ''; selectedId = ''; verified = false;
    error = ''; notice = '';
    approving = true;
  }

  function openRevoke(event: MouseEvent & { currentTarget: HTMLElement }, link: OidcLink): void {
    trigger = event.currentTarget;
    error = ''; notice = '';
    revoking = link;
  }

  function close(): void {
    if (busy) return;
    approving = false; revoking = null;
    subject = ''; error = '';
  }

  function restoreFocus(event: Event): void {
    event.preventDefault();
    if (trigger?.isConnected) trigger.focus();
    else heading?.focus();
  }

  async function approve(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy || !verified) return;
    busy = true; error = '';
    const account = eligible.find((item) => item.account_id === selectedId);
    try {
      await session.approveOidc(subject.trim(), account?.handle ?? handle, account?.account_id);
      approving = false;
      subject = '';
      notice = 'Keycloak sign-in approved. The approval expires in 24 hours.';
      await load();
    } catch (failure) {
      error = failure instanceof HttpError && failure.status === 409
        ? 'This subject or account cannot be linked. Check existing links and the account’s status.'
        : 'Approval could not be saved. Check the details and try again.';
    } finally { busy = false; }
  }

  async function revoke(): Promise<void> {
    if (busy || !revoking) return;
    const link = revoking;
    busy = true; error = '';
    try {
      await session.revokeOidc(link);
      links = links.filter((item) => item.id !== link.id);
      revoking = null;
      notice = link.pending ? 'Approval canceled.' : 'Keycloak access revoked.';
      await Promise.all([load(), onrevoked()]);
      await tick();
      heading?.focus();
    } catch { error = 'Could not revoke Keycloak access. Refresh the list before trying again.'; }
    finally { busy = false; }
  }

  function preventWhileBusy(event: Event): void { if (busy) event.preventDefault(); }
</script>

<section class="settings-section" aria-labelledby="oidc-approvals-heading">
  <div class="flex flex-wrap items-start justify-between gap-2">
    <h2 id="oidc-approvals-heading" tabindex="-1" bind:this={heading}>Keycloak sign-in</h2>
    <div class="flex flex-wrap gap-2">
      <RefreshButton label="Refresh Keycloak links" busyLabel="Refreshing…" keepFocus busy={loading} disabled={busy} onclick={() => { if (!loading && !busy) { notice = ''; void load(); } }} />
      <Button size="sm" disabled={busy} onclick={openApproval}>Approve sign-in</Button>
    </div>
  </div>
  {#if notice}<p class="form-notice" role="status">{notice}</p>{/if}
  <div aria-busy={loading}>
    {#if loadError}<p class="form-notice" role="alert">Could not load Keycloak links. Refresh to try again.</p>
    {:else if loading && !links.length}<p role="status">Loading Keycloak links…</p>
    {:else if !links.length}<p class="text-muted-foreground text-sm mt-2">No pending or active links.</p>{/if}
    {#if links.length}
      <ul aria-label="Keycloak links" class="mt-2">
        {#each links as link (link.id)}
          <li class="flex flex-wrap justify-between items-center gap-2 border-b py-2">
            <div class="grid gap-1 break-all"><strong>{link.handle}</strong>
              <span class="text-xs text-muted-foreground">
                {link.pending && link.expires_at ? `Pending · expires ${new Date(link.expires_at * 1000).toLocaleString()}` : 'Linked'}
              </span>
            </div>
            <Button size="sm" variant="outline" disabled={busy || loading} aria-label={link.pending ? `Cancel approval for ${link.handle}` : `Revoke link for ${link.handle}`} onclick={(event) => openRevoke(event, link)}>{link.pending ? 'Cancel approval' : 'Revoke link'}</Button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</section>

<Dialog.Root open={approving} onOpenChange={(open) => { if (!open) close(); }}>
  <Dialog.Content onEscapeKeydown={preventWhileBusy} onInteractOutside={preventWhileBusy} showCloseButton={!busy} onCloseAutoFocus={restoreFocus}>
    <Dialog.Header>
      <Dialog.Title>Approve Keycloak sign-in</Dialog.Title>
      <Dialog.Description>Verify the person and copy their subject from Keycloak administration.</Dialog.Description>
    </Dialog.Header>
    <form class="grid gap-4" onsubmit={approve}>
      <div class="grid gap-2"><Label for="oidc-subject">Keycloak subject</Label><Input id="oidc-subject" bind:value={subject} maxlength={255} disabled={busy} autocomplete="off" required /></div>
      <div class="grid gap-2">
        <Label for="oidc-account">Sylux account</Label>
        <select id="oidc-account" class="min-h-10 rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-2 focus-visible:outline-ring" bind:value={selectedId} disabled={busy}>
          <option value="">New account</option>
          {#each eligible as account (account.account_id)}<option value={account.account_id}>{account.handle}</option>{/each}
        </select>
      </div>
      {#if !selectedId}<div class="grid gap-2"><Label for="oidc-handle">New account name</Label><Input id="oidc-handle" bind:value={handle} pattern={'[a-z0-9][a-z0-9._\\-]*'} title="Start with a lowercase letter or digit. Use lowercase letters, digits, dots, underscores or hyphens." maxlength={64} disabled={busy} required /></div>{/if}
      <label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={verified} disabled={busy} required />I verified the person and their Keycloak subject.</label>
      {#if error}<p role="alert" class="form-notice">{error}</p>{/if}
      <Dialog.Footer><Button type="button" variant="outline" disabled={busy} onclick={close}>Cancel</Button><Button type="submit" disabled={busy || !verified} aria-busy={busy}>{busy ? 'Approving…' : 'Approve sign-in'}</Button></Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root open={!!revoking} onOpenChange={(open) => { if (!open) close(); }}>
  <Dialog.Content onEscapeKeydown={preventWhileBusy} onInteractOutside={preventWhileBusy} showCloseButton={!busy} onCloseAutoFocus={restoreFocus}>
    <Dialog.Header>
      <Dialog.Title class="break-words">{revoking?.pending ? 'Cancel approval' : 'Revoke Keycloak access'} for {revoking?.handle}?</Dialog.Title>
      <Dialog.Description>{revoking?.pending ? 'They will need a new approval before they can finish linking.' : 'This disables the account, signs out its sessions, ends direct shares to and from it, and withdraws its published libraries. Its files stay stored.'}</Dialog.Description>
    </Dialog.Header>
    {#if error}<p role="alert" class="form-notice">{error}</p>{/if}
    <Dialog.Footer><Button variant="outline" disabled={busy} onclick={close}>Keep {revoking?.pending ? 'approval' : 'access'}</Button><Button variant="destructive" disabled={busy} aria-busy={busy} onclick={revoke}>{busy ? 'Revoking…' : revoking?.pending ? 'Cancel approval' : 'Revoke access'}</Button></Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

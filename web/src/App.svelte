<script lang="ts">
  import './app.css';
  import { onMount, tick } from 'svelte';
  import MoonIcon from '@lucide/svelte/icons/moon';
  import SunIcon from '@lucide/svelte/icons/sun';
  import UserRoundIcon from '@lucide/svelte/icons/user-round';
  import LogOutIcon from '@lucide/svelte/icons/log-out';
  import EntryOptions from './lib/EntryOptions.svelte';
  import HeaderIconButton from './lib/HeaderIconButton.svelte';
  import FilePreview from './lib/FilePreview.svelte';
  import FileIcon from '@lucide/svelte/icons/file';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Progress } from '$lib/components/ui/progress/index.js';
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import { ApiClient, HttpError, HttpTransport, MutationApi } from './lib/api';
  import { ListingApi, type Entry, type Folder, type DeletedEntry } from './lib/listing';
  import { SessionApi, type User, type Account } from './lib/session';
  import { TransferApi, type Replacement } from './lib/transfers';

  import { MockTransport } from './lib/mock-transport';

  // Dev-only: VITE_MOCK=1 renders the UI without a backend.
  const client = new ApiClient(import.meta.env.DEV && import.meta.env.VITE_MOCK ? new MockTransport() : new HttpTransport());
  const mutations = new MutationApi(client);
  const session = new SessionApi(client);
  const listing = new ListingApi(client);
  let transfers = new TransferApi(client, mutations);
  let view = $state<'login' | 'setup'>('login');
  let authBusy = $state(false);
  let darkMode = $state(false);
  let followsSystem = true;

  function applyTheme(dark: boolean): void {
    darkMode = dark;
    document.documentElement.classList.toggle('dark', dark);
    document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
  }

  function toggleTheme(): void {
    followsSystem = false;
    applyTheme(!darkMode);
    try { localStorage.setItem('sylux-theme', darkMode ? 'dark' : 'light'); }
    catch { /* The switch still works when storage is unavailable. */ }
  }

  onMount(() => {
    const preference = window.matchMedia('(prefers-color-scheme: dark)');
    let saved: string | null = null;
    try { saved = localStorage.getItem('sylux-theme'); }
    catch { /* Use the system preference when storage is unavailable. */ }
    followsSystem = saved !== 'dark' && saved !== 'light';
    applyTheme(followsSystem ? preference.matches : saved === 'dark');
    const onChange = (event: MediaQueryListEvent) => {
      if (followsSystem) applyTheme(event.matches);
    };
    preference.addEventListener('change', onChange);
    return () => preference.removeEventListener('change', onChange);
  });
  let user = $state<User | null>(null);
  let accountView = $state(false);
  let sessionGeneration = 0;
  let loggingOut = $state(false);
  let accounts = $state<Account[]>([]);
  let accountHandle = $state('');
  let accountPassword = $state('');
  let currentPassword = $state('');
  let newPassword = $state('');
  let resetId = $state('');
  let resetPassword = $state('');
  let accountNotice = $state('');
  let accountBusy = $state(false);
  let accountsLoading = $state(false);
  let accountsError = $state('');
  let accountsRequest = 0;
  let handle = $state('');
  let password = $state('');
  let setupAuthorization = $state('');
  const emptyRoot: Folder = { id: '', parent_id: null, name: 'Files', kind: 'folder', etag: '"g1"' };
  let currentFolder = $state<Folder>(emptyRoot);
  let folders = $state<Folder[]>([]);
  let entries = $state<Entry[]>([]);
  let pageCursors = $state<(string | null)[]>([null]);
  let pageIndex = $state(0);
  let nextCursor = $state<string | null>(null);
  let hasMore = $state(false);
  let folderPath = $state<Folder[]>([emptyRoot]);
  let loading = $state(false);
  let listingError = $state('');
  let listingRequest = 0;
  let notice = $state('');
  let folderName = $state('');
  let folderBusy = $state(false);
  let createOpen = $state(false);
  let folderError = $state('');
  let folderInput = $state<HTMLInputElement | null>(null);
  let createTrigger: HTMLElement | null = null;
  let previewEntry = $state<Entry | null>(null);
  let previewTrigger: HTMLElement | null = null;
  let draggedEntry = $state<Entry | null>(null);
  let dropTarget = $state('');
  let dropPending = $state<{ entry: Entry; destination: Folder } | null>(null);
  let dropError = $state('');
  let moveNeedsRefresh = $state(false);
  const dragType = 'application/x-sylux-entry';
  const knownParents = new Map<string, string | null>();
  let trashTarget = $state<Entry | null>(null);
  let trashTrigger: HTMLElement | null = null;
  let deletedItems = $state<DeletedEntry[]>([]);
  let trashRequest = 0;
  let showTrash = $state(false);
  let trashError = $state('');
  let trashBusy = $state(false);
  let trashLoading = $state(false);
  let renameId = $state('');
  let renameName = $state('');
  let renameError = $state('');
  let renameBusy = $state(false);
  let renameTrigger: HTMLElement | null = null;
  let renameInput = $state<HTMLInputElement | null>(null);
  let moveTarget = $state<Entry | null>(null);
  let movePath = $state<Folder[]>([]);
  let moveFolders = $state<Folder[]>([]);
  let moveCursor = $state<string | null>(null);
  let moveError = $state('');
  let moveLoading = $state(false);
  let moveDestinationReady = $state(false);
  let moveBusy = $state(false);
  let moveRequest = 0;
  let moveTrigger: HTMLElement | null = null;
  let replacementPrompt = $state<{ file: File; folder: Folder; target: { id: string; name: string; etag: string } } | null>(null);
  let replacementTrigger: HTMLElement | null = null;
  let uploadReplacement = $state(false);
  let transferName = $state('');
  let progress = $state(0);
  let transferPhase = $state<'uploading' | 'saving' | 'complete' | ''>('');
  let transferBusy = $state(false);
  let uploadCancelable = $state(false);
  let uploadId = $state('');
  let unresolvedUploadId = $state('');
  let unresolvedUploadName = $state('');
  let unresolvedCancel = $state(false);
  let controller: AbortController | null = null;
  let selectedFileName = $state('');

  function restoreFocus(trigger: HTMLElement | null, fallback = 'folder-title'): void {
    if (trigger?.isConnected) trigger.focus();
    else document.getElementById(fallback)?.focus();
  }

  function expireSession(generation: number): void {
    if (generation !== sessionGeneration || !user) return;
    sessionGeneration++; clearAccountForms(); clearWorkspace();
    client.setSessionExpiredHandler(null); client.setCsrfToken(null);
    user = null; accountView = false; view = 'login';
    notice = 'Your session ended. Sign in again to continue.';
    void tick().then(() => document.getElementById('handle')?.focus());
  }

  async function refreshRoot(): Promise<boolean> {
    const generation = sessionGeneration;
    const listingData = await listing.roots();
    if (generation !== sessionGeneration) return false;
    const root = { ...listingData.root, name: 'Files' };
    const descriptors = [root, ...listingData.items.filter((entry): entry is Folder => entry.kind === 'folder')];
    folders = descriptors;
    for (const folder of descriptors) knownParents.set(folder.id, folder.parent_id);
    const current = descriptors.find((folder) => folder.id === currentFolder.id);
    if (current) currentFolder = current;
    return true;
  }

  async function refresh(): Promise<boolean> {
    if (!user) return false;
    const generation = sessionGeneration;
    let folderId = currentFolder.id;
    const requestId = ++listingRequest;
    loading = true;
    listingError = '';
    try {
      if (!folderId) {
        if (!await refreshRoot() || requestId !== listingRequest) return false;
        currentFolder = folders[0]; folderPath = [currentFolder]; folderId = currentFolder.id;
      }
      const page = await listing.children(folderId);
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return false;
      entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      moveNeedsRefresh = false;
      pageCursors = [null]; pageIndex = 0; nextCursor = page.next_cursor; hasMore = page.has_more;
      return true;
    }
    catch { if (generation === sessionGeneration && currentFolder.id === folderId && requestId === listingRequest) listingError = 'Could not load this folder. Try again.'; return false; }
    finally { if (generation === sessionGeneration && requestId === listingRequest) loading = false; }
  }

  async function loadMore(): Promise<void> {
    if (!hasMore || !nextCursor || loading) return;
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const cursor = nextCursor;
    const requestId = ++listingRequest;
    loading = true; listingError = '';
    try {
      const page = await listing.children(folderId, cursor);
      if (generation !== sessionGeneration || currentFolder.id !== folderId || nextCursor !== cursor || requestId !== listingRequest) return;
      pageCursors = [...pageCursors.slice(0, pageIndex + 1), nextCursor];
      pageIndex += 1; entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      nextCursor = page.next_cursor; hasMore = page.has_more;
    } catch (error) {
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return;
      if (error instanceof HttpError && error.status === 409) {
        await refresh();
        if (generation !== sessionGeneration) return;
        notice = 'This folder changed. The listing has been restarted.';
      } else listingError = 'Could not load the next page. Try again.';
    } finally { if (generation === sessionGeneration && requestId === listingRequest) loading = false; }
  }

  async function loadPrevious(): Promise<void> {
    if (pageIndex === 0 || loading) return;
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const requestId = ++listingRequest;
    loading = true; listingError = '';
    const target = pageIndex - 1;
    try {
      const page = await listing.children(folderId, pageCursors[target]);
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return;
      pageIndex = target; entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      nextCursor = page.next_cursor; hasMore = page.has_more;
    } catch (error) {
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return;
      if (error instanceof HttpError && error.status === 409) {
        await refresh();
        if (generation !== sessionGeneration) return;
        notice = 'This folder changed. The listing has been restarted.';
      } else listingError = 'Could not load the previous page. Try again.';
    } finally { if (generation === sessionGeneration && requestId === listingRequest) loading = false; }
  }

  async function signIn(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (authBusy) return;
    authBusy = true;
    const authView = view;
    notice = '';
    let generation = sessionGeneration;
    try {
      const bootstrap = await session.csrf();
      client.setCsrfToken(bootstrap.csrfToken);
      user = authView === 'setup' ? await session.setup(handle, password, setupAuthorization) : await session.login(handle, password);
      generation = ++sessionGeneration;
      authBusy = false;
      handle = '';
      password = '';
      setupAuthorization = '';
      if (authView === 'setup') {
        user = null;
        view = 'login';
        notice = 'Setup complete. Sign in with the new account.';
        return;
      }
      client.setSessionExpiredHandler(() => expireSession(generation));
      await refresh();
    } catch {
      if (generation !== sessionGeneration) return;
      notice = authView === 'setup' ? 'Setup could not be completed. Check the authorization and account details, then try again.' : 'Sign-in could not be completed. Check the details and try again.';
      password = '';
    } finally { if (generation === sessionGeneration) authBusy = false; }
  }

  async function openAccounts(): Promise<void> {
    accountView = true; accountNotice = '';
    if (user?.role === 'administrator') await loadAccounts();
  }
  async function loadAccounts(): Promise<void> {
    const generation = sessionGeneration;
    const request = ++accountsRequest;
    accountsLoading = true; accountsError = '';
    try {
      const result = await session.accounts();
      if (generation === sessionGeneration && request === accountsRequest) accounts = result;
    } catch {
      if (generation === sessionGeneration && request === accountsRequest) accountsError = 'Could not load accounts. Try again.';
    } finally { if (generation === sessionGeneration && request === accountsRequest) accountsLoading = false; }
  }
  async function createAccount(event: SubmitEvent): Promise<void> {
    event.preventDefault(); const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.createAccount(accountHandle, accountPassword);
      if (generation !== sessionGeneration) return;
      accountHandle = ''; accountPassword = ''; accountNotice = 'Account created. Share the password through a private channel.';
      await loadAccounts();
    } catch { if (generation === sessionGeneration) accountNotice = 'Account could not be created. Check the name and password, then try again.'; }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }
  async function disableAccount(account: Account): Promise<void> {
    if (!window.confirm(`Disable ${account.handle}? All their sessions will be signed out.`)) return;
    const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.disableAccount(account.account_id);
      if (generation !== sessionGeneration) return;
      accountNotice = 'Account disabled and sessions signed out.';
      await loadAccounts();
      if (generation === sessionGeneration) { await tick(); document.getElementById('accounts-heading')?.focus(); }
    } catch { if (generation === sessionGeneration) accountNotice = 'Account could not be disabled. Try again.'; }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }
  async function resetAccount(event: SubmitEvent): Promise<void> {
    event.preventDefault(); const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.resetAccount(resetId, resetPassword);
      if (generation !== sessionGeneration) return;
      resetId = ''; resetPassword = ''; accountNotice = 'Password reset and sessions signed out. Share the new password through a private channel.';
      await tick(); if (generation === sessionGeneration) document.getElementById('accounts-heading')?.focus();
    } catch { if (generation === sessionGeneration) accountNotice = 'Password could not be reset. Check its length and try again.'; }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }
  function clearWorkspace(): void {
    controller?.abort(); controller = null;
    replacementPrompt = null; uploadReplacement = false; selectedFileName = '';
    transfers.invalidate(); transfers = new TransferApi(client, mutations);
    transferName = ''; progress = 0; transferPhase = ''; transferBusy = false; uploadCancelable = false;
    uploadId = ''; unresolvedUploadId = ''; unresolvedUploadName = ''; unresolvedCancel = false; notice = '';
    listingRequest++; currentFolder = emptyRoot; folders = []; entries = []; folderPath = [emptyRoot];
    pageCursors = [null]; pageIndex = 0; nextCursor = null; hasMore = false; loading = false; listingError = '';
    folderName = ''; folderBusy = false; createOpen = false; folderError = ''; previewEntry = null; previewTrigger = null;
    draggedEntry = null; dropTarget = ''; dropPending = null; dropError = ''; moveNeedsRefresh = false; knownParents.clear();
    renameId = ''; renameName = ''; renameError = ''; renameBusy = false;
    closeMove(); moveBusy = false;
    trashRequest++; deletedItems = []; showTrash = false; trashError = ''; trashTarget = null; trashBusy = false; trashLoading = false;
  }
  function clearAccountForms(): void {
    accountHandle = ''; accountPassword = ''; currentPassword = ''; newPassword = '';
    resetId = ''; resetPassword = ''; accounts = []; accountNotice = ''; accountBusy = false; accountsLoading = false; accountsError = ''; accountsRequest++;
  }
  async function changePassword(event: SubmitEvent): Promise<void> {
    event.preventDefault(); const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.changePassword(currentPassword, newPassword);
      if (generation !== sessionGeneration) return;
      sessionGeneration++; clearAccountForms(); clearWorkspace(); client.setSessionExpiredHandler(null);
      user = null; accountView = false; client.setCsrfToken(null); notice = 'Password changed. Sign in again.';
    } catch (error) {
      if (generation !== sessionGeneration) return;
      // The password route also uses 401 for an incorrect current password.
      if (error instanceof HttpError && error.status === 401) {
        try { await listing.roots(); }
        catch {
          if (generation === sessionGeneration) accountNotice = 'Could not check your session. Check your connection and try again.';
          return;
        }
      }
      if (generation === sessionGeneration) accountNotice = 'Password could not be changed. Check the current password and the new password length.';
    }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }

  async function signOut(): Promise<void> {
    if (loggingOut) return;
    loggingOut = true; client.setSessionExpiredHandler(null); sessionGeneration++; clearAccountForms(); clearWorkspace(); accountView = false; user = null;
    let logoutFailed = false;
    try { await session.logout(); } catch { logoutFailed = true; }
    sessionGeneration++; clearAccountForms(); clearWorkspace();
    if (logoutFailed) notice = 'Sign-out could not be confirmed. Check your connection.';
    client.setCsrfToken(null);
    loggingOut = false;
  }

  function beginCreate(trigger: HTMLElement): void {
    createTrigger = trigger; folderName = ''; folderError = ''; createOpen = true;
  }

  async function createFolder(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (folderBusy || !currentFolder.id) return;
    const name = folderName.trim();
    if (!name || name === '.' || name === '..' || /[\/\\\x00-\x1f\x7f]/.test(name)) {
      folderError = 'Enter a folder name without slashes or control characters.';
      await tick(); folderInput?.focus(); return;
    }
    const generation = sessionGeneration;
    const parent = currentFolder;
    folderBusy = true; folderError = ''; notice = '';
    try {
      await listing.createFolder(parent.id, name, parent.etag);
    } catch (error) {
      if (generation === sessionGeneration) {
        folderError = error instanceof HttpError && error.status === 409 ? 'An item already has this name. Choose another name.' : 'Folder could not be created. Check its name and try again.';
        folderBusy = false; await tick(); folderInput?.focus();
      }
      return;
    }
    if (generation !== sessionGeneration) return;
    folderName = ''; notice = 'Folder created.';
    try {
      await refreshRoot();
      if (generation !== sessionGeneration) return;
      if (!await refresh()) notice = 'Folder created. Refresh your files to see it.';
    } catch { if (generation === sessionGeneration) notice = 'Folder created. Refresh your files to see it.'; }
    finally { if (generation === sessionGeneration) { folderBusy = false; createOpen = false; } }
  }

  // The dialog moves focus itself: to the name field on open, back to the trigger on close.
  function beginRename(entry: Entry, trigger: HTMLElement): void {
    renameError = ''; renameId = entry.id;
    renameName = entry.name;
    renameTrigger = trigger;
  }
  function closeRename(): void {
    renameId = ''; renameError = '';
  }
  async function renameEntry(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const target = entries.find((entry) => entry.id === renameId);
    if (!target || !renameName.trim() || renameBusy) return;
    const generation = sessionGeneration;
    renameBusy = true; renameError = '';
    try { await listing.rename(target.id, renameName.trim(), target.etag); }
    catch (error) {
      if (generation !== sessionGeneration) return;
      renameError = error instanceof HttpError && error.status === 409 ? 'An item already has this name. Choose another name.' : 'Rename could not be completed. Check the name and try again. If the item changed, cancel and refresh the folder.';
      renameBusy = false; await tick(); renameInput?.focus();
      return;
    }
    if (generation !== sessionGeneration) return;
    const refreshed = await refresh();
    if (generation !== sessionGeneration) return;
    notice = refreshed ? 'Name updated.' : 'Name updated. Refresh your files to see it.';
    renameBusy = false; closeRename();
  }

  function closeMove(): void {
    moveRequest++; moveTarget = null; movePath = []; moveFolders = []; moveCursor = null; moveLoading = false; moveDestinationReady = false; moveError = '';
  }
  async function loadMoveFolder(path: Folder[], cursor: string | null = null): Promise<void> {
    const destination = path.at(-1);
    if (!destination || !moveTarget) return;
    const request = ++moveRequest;
    const generation = sessionGeneration;
    moveLoading = true; moveDestinationReady = false; moveError = ''; movePath = path;
    if (!cursor) { moveFolders = []; moveCursor = null; }
    try {
      const page = await listing.children(destination.id, cursor);
      if (request !== moveRequest || generation !== sessionGeneration || !moveTarget) return;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      movePath = path;
      const children = page.items.filter((entry): entry is Folder => entry.kind === 'folder');
      moveFolders = cursor ? [...moveFolders, ...children] : children;
      moveCursor = page.next_cursor;
      moveDestinationReady = true;
    } catch {
      if (request === moveRequest && generation === sessionGeneration) moveError = 'Could not open this destination. Choose another folder or try again.';
    } finally { if (request === moveRequest) moveLoading = false; }
  }
  async function beginMove(entry: Entry, trigger: HTMLElement): Promise<void> {
    if (dropPending || moveNeedsRefresh) return;
    closeMove(); moveTarget = entry; moveTrigger = trigger;
    const root = folders[0];
    if (root) await loadMoveFolder([root]);
    else moveError = 'Your storage is unavailable. Close this dialog and refresh your files.';
  }
  async function moveEntry(): Promise<void> {
    const target = moveTarget;
    const destination = movePath.at(-1);
    if (!target || !destination || moveBusy || moveLoading || !moveDestinationReady || destination.id === target.parent_id) return;
    const generation = sessionGeneration;
    moveBusy = true; moveError = '';
    try {
      await listing.move(target.id, destination.id, target.etag);
      if (generation === sessionGeneration) knownParents.set(target.id, destination.id);
    } catch (error) {
      if (generation !== sessionGeneration) return;
      moveError = moveFailure(error);
      if (moveOutcomeUncertain(error)) moveNeedsRefresh = true;
      moveBusy = false;
      return;
    }
    if (generation !== sessionGeneration) return;
    closeMove(); moveBusy = false;
    notice = `${target.name} moved to ${destination.name}.`;
    try {
      await refreshRoot();
      if (generation === sessionGeneration) await refresh();
    } catch { if (generation === sessionGeneration) notice = `${target.name} moved to ${destination.name}. Refresh your files to update the folder list.`; }
  }

  function moveFailure(error: unknown): string {
    if (error instanceof HttpError) {
      if (error.code === 'name_conflict') return 'An item with this name is already there. Choose another folder, or rename your item.';
      if (error.code === 'invalid_destination') return 'Choose a different folder. A folder cannot be moved into itself or its children.';
      if (error.code === 'depth_limit') return 'This destination would nest folders too deeply. Choose a folder closer to Files.';
      if (error.status === 404) return 'The item or destination is no longer available. Refresh your files.';
      if (error.status === 412) return 'This item changed. Refresh your files and try again.';
    }
    return 'Move could not be confirmed. Close any open dialog and refresh your files before trying again.';
  }

  function moveOutcomeUncertain(error: unknown): boolean {
    return !(error instanceof HttpError) || error.status >= 500;
  }

  function canDrop(entry: Entry | null, destination: Folder): boolean {
    if (!entry || dropPending || moveBusy || moveNeedsRefresh || destination.id === entry.id || destination.id === entry.parent_id) return false;
    const seen = new Set<string>();
    let ancestor: string | null | undefined = destination.id;
    while (ancestor) {
      if (ancestor === entry.id || seen.has(ancestor)) return false;
      seen.add(ancestor); ancestor = knownParents.get(ancestor);
    }
    return true;
  }

  function startDrag(event: DragEvent, entry: Entry): void {
    if (!event.dataTransfer || dropPending || moveBusy || moveNeedsRefresh) { event.preventDefault(); return; }
    draggedEntry = entry; dropError = '';
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData(dragType, entry.id);
  }

  function endDrag(): void { draggedEntry = null; dropTarget = ''; }

  function dragOver(event: DragEvent, destination: Folder): void {
    if (!event.dataTransfer?.types.includes(dragType) || !canDrop(draggedEntry, destination)) return;
    event.preventDefault(); event.dataTransfer.dropEffect = 'move'; dropTarget = destination.id;
  }

  function dragLeave(event: DragEvent, destination: Folder): void {
    if (event.currentTarget instanceof HTMLElement && event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget)) return;
    if (dropTarget === destination.id) dropTarget = '';
  }

  async function dropEntry(event: DragEvent, destination: Folder): Promise<void> {
    const target = draggedEntry;
    if (!event.dataTransfer?.types.includes(dragType) || !canDrop(target, destination) || !target) return;
    event.preventDefault(); event.stopPropagation(); endDrag();
    const generation = sessionGeneration;
    dropPending = { entry: target, destination }; dropError = ''; notice = '';
    try {
      await listing.move(target.id, destination.id, target.etag);
      if (generation === sessionGeneration) knownParents.set(target.id, destination.id);
      if (generation !== sessionGeneration) return;
      notice = `${target.name} moved to ${destination.name}.`;
      try { await refreshRoot(); if (generation === sessionGeneration) await refresh(); }
      catch { if (generation === sessionGeneration) notice += ' Refresh your files to update the folder list.'; }
    } catch (error) { if (generation === sessionGeneration) { dropError = moveFailure(error); if (moveOutcomeUncertain(error)) moveNeedsRefresh = true; } }
    finally { if (generation === sessionGeneration) dropPending = null; }
  }

  function externalDrag(event: DragEvent): void {
    if (!event.dataTransfer?.types.includes('Files')) return;
    event.preventDefault();
    if (event.type === 'dragover') event.dataTransfer.dropEffect = 'none';
    else if (user) notice = 'Use Choose file and Upload to add a file. Dragging here moves files already in Sylux.';
  }

  function openPreview(entry: Entry, trigger: HTMLElement): void { previewTrigger = trigger; previewEntry = entry; }
  function closePreview(): void {
    previewEntry = null;
    void tick().then(() => { restoreFocus(previewTrigger); previewTrigger = null; });
  }
  function entryAction(action: 'download' | 'rename' | 'move' | 'delete', entry: Entry, trigger: HTMLElement): void {
    if (action === 'download') void download(entry);
    else if (action === 'rename') beginRename(entry, trigger);
    else if (action === 'move') void beginMove(entry, trigger);
    else { trashError = ''; trashTarget = entry; trashTrigger = trigger; }
  }

  async function loadTrash(): Promise<void> {
    const accountId = user?.id;
    if (!accountId) return;
    const request = ++trashRequest;
    trashError = ''; trashLoading = true;
    try {
      const result = await listing.deleted();
      if (request === trashRequest && user?.id === accountId && showTrash) deletedItems = result.items;
    } catch {
      if (request === trashRequest && user?.id === accountId && showTrash) trashError = 'Could not load trash. Try again.';
    } finally { if (request === trashRequest && user?.id === accountId) trashLoading = false; }
  }
  async function openTrash(): Promise<void> {
    showTrash = !showTrash;
    if (showTrash) await loadTrash();
    else { trashRequest++; deletedItems = []; trashError = ''; trashLoading = false; }
  }
  async function deleteEntry(): Promise<void> {
    const target = trashTarget;
    if (!target || trashBusy) return;
    trashBusy = true;
    const sessionVersion = sessionGeneration;
    try {
      await listing.trash(target.id, target.etag);
      if (sessionVersion !== sessionGeneration) return;
      notice = `${target.name} moved to trash.`;
      await refresh();
      if (sessionVersion !== sessionGeneration) return;
      trashTarget = null;
      try { await refreshRoot(); } catch { /* The deletion was confirmed. */ }
      if (sessionVersion === sessionGeneration && showTrash) await loadTrash();
    } catch (error) {
      if (sessionVersion !== sessionGeneration) return;
      trashError = error instanceof HttpError && error.status === 409 ? 'Empty this folder before deleting it.' : 'Could not delete this item. Refresh the folder and try again.';
    } finally { if (sessionVersion === sessionGeneration) trashBusy = false; }
  }
  function needsDeletedReference(item: DeletedEntry): boolean {
    return deletedItems.some((other) => other.id !== item.id && other.name === item.name &&
      other.original_path.length === item.original_path.length &&
      other.original_path.every((part, index) => part === item.original_path[index]));
  }
  async function restoreEntry(item: Entry): Promise<void> {
    if (trashBusy) return;
    trashBusy = true; trashError = '';
    const sessionVersion = sessionGeneration;
    try {
      await listing.restore(item.id, item.etag);
      if (sessionVersion !== sessionGeneration) return;
      notice = `${item.name} restored.`;
      await loadTrash();
      if (sessionVersion === sessionGeneration) { await refresh(); await tick(); if (sessionVersion === sessionGeneration) document.getElementById('trash-heading')?.focus(); }
    } catch (error) {
      if (sessionVersion !== sessionGeneration) return;
      trashError = error instanceof HttpError && error.status === 409 ? 'Restore its parent folder first, or rename the item occupying its original name.' : 'Could not restore this item. Refresh trash and try again.';
    } finally { if (sessionVersion === sessionGeneration) trashBusy = false; }
  }

  async function uploadOutcome(id: string): Promise<'committed' | 'canceled' | 'rejected' | 'unknown'> {
    try {
      const status = await transfers.status(id);
      if (status.state === 'committed') return 'committed';
      if (status.state === 'rejected' && status.status === 'canceled') return 'canceled';
      if (status.state === 'rejected') return 'rejected';
      return 'unknown';
    } catch { return 'unknown'; }
  }

  async function showCommittedUpload(): Promise<void> {
    const generation = sessionGeneration;
    progress = 100; transferPhase = 'complete';
    notice = uploadReplacement ? 'Replacement complete. The previous file is in trash. Rename the current file before restoring the previous one.' : 'Upload complete.';
    try { if (!await refreshRoot()) return; } catch { /* Keep the confirmed commit outcome visible. */ }
    if (generation !== sessionGeneration) return;
    await refresh();
  }

  async function upload(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const input = document.querySelector<HTMLInputElement>('#upload-file');
    const file = input?.files?.[0];
    if (!file || transferBusy || unresolvedUploadId) return;
    replacementTrigger = event.submitter as HTMLElement | null;
    await startUpload(file, input);
  }

  async function confirmReplacement(): Promise<void> {
    const prompt = replacementPrompt;
    if (!prompt || transferBusy || unresolvedUploadId) return;
    replacementPrompt = null;
    await startUpload(prompt.file, null, { entry_id: prompt.target.id, etag: prompt.target.etag }, prompt.folder);
  }

  function resetTransferActivity(): void {
    if (transferPhase !== 'complete') { transferPhase = ''; progress = 0; }
  }

  async function startUpload(file: File, input: HTMLInputElement | null, replacement?: Replacement, folder = currentFolder): Promise<void> {
    const generation = sessionGeneration;
    const activeTransfers = transfers;
    transferBusy = true;
    uploadReplacement = !!replacement;
    progress = 0;
    transferPhase = 'uploading';
    transferName = file.name;
    const uploadController = new AbortController();
    controller = uploadController;
    try {
      if (!await refreshRoot()) return;
      const targetFolder = currentFolder.id === folder.id ? currentFolder : folder;
      const admission = await activeTransfers.admit(targetFolder.id, file.name, file.size, targetFolder.etag, (id) => { if (generation === sessionGeneration) uploadId = id; }, replacement);
      if (generation !== sessionGeneration) return;
      uploadId = admission.id;
      progress = 0;
      uploadCancelable = true;
      await activeTransfers.send(admission.id, file, uploadController.signal, (sent, total) => { if (generation === sessionGeneration) progress = total ? Math.floor((sent / total) * 100) : 0; });
      if (generation !== sessionGeneration) return;
      uploadCancelable = false;
      progress = 100;
      transferPhase = 'saving';
      try { await activeTransfers.commit(admission.id); }
      catch (error) {
        if (generation !== sessionGeneration) return;
        if (replacement && error instanceof HttpError && (error.status === 409 || error.status === 412)) {
          try {
            await activeTransfers.cancel(admission.id);
            if (generation !== sessionGeneration) return;
            notice = 'Replacement was not saved because the file or folder changed. This upload did not replace the existing file. Refresh and try again.';
            return;
          } catch { /* Check durable status when cancellation cannot be confirmed. */ }
        }
        const outcome = await uploadOutcome(admission.id);
        if (generation !== sessionGeneration) return;
        if (outcome === 'rejected') {
          notice = 'Upload failed. No saved file is implied.';
          return;
        }
        if (outcome !== 'committed') {
          unresolvedUploadId = admission.id;
          unresolvedUploadName = file.name;
          notice = 'Upload outcome could not be confirmed. Check your files before retrying.';
          return;
        }
      }
      if (generation !== sessionGeneration) return;
      await showCommittedUpload();
    } catch (error) {
      if (generation !== sessionGeneration) return;
      if (error instanceof HttpError && error.status === 409 && error.details?.code === 'name_collision') {
        if (uploadId) activeTransfers.forget(uploadId);
        uploadId = '';
        const target = error.details.existing;
        if (target?.kind === 'file') {
          replacementPrompt = { file, folder, target };
          notice = 'A file already has this name. Confirm replacement to continue.';
        } else notice = 'A folder already has this name. Rename the upload or the folder and try again.';
      } else if (error instanceof HttpError && error.status >= 400 && error.status < 500 && !uploadCancelable && transferPhase === 'uploading') {
        if (uploadId) activeTransfers.forget(uploadId);
        uploadId = '';
        notice = replacement ? 'Replacement was not admitted. The file or folder changed. Refresh and try again.' : 'Upload was not admitted. Refresh the folder and try again.';
      } else if (error instanceof DOMException && error.name === 'AbortError') {
        try {
          if (!uploadId) throw new Error('Upload was not admitted');
          await activeTransfers.cancel(uploadId);
          if (generation !== sessionGeneration) return;
          notice = uploadReplacement ? 'Replacement canceled. This upload did not replace the existing file.' : 'Upload canceled. No saved file is implied.';
        } catch {
          if (generation !== sessionGeneration) return;
          const outcome = uploadId ? await uploadOutcome(uploadId) : 'unknown';
          if (generation !== sessionGeneration) return;
          if (outcome === 'committed') {
            await showCommittedUpload();
          } else if (outcome === 'canceled') notice = uploadReplacement ? 'Replacement canceled. This upload did not replace the existing file.' : 'Upload canceled. No saved file is implied.';
          else { unresolvedUploadId = uploadId; unresolvedUploadName = file.name; unresolvedCancel = true; notice = 'Upload outcome could not be confirmed. Check your files before retrying.'; }
        }
      } else if (uploadId) {
        const outcome = await uploadOutcome(uploadId);
        if (generation !== sessionGeneration) return;
        if (outcome === 'committed') await showCommittedUpload();
        else if (outcome === 'canceled' || outcome === 'rejected') notice = uploadReplacement ? 'Replacement failed. This upload did not replace the existing file.' : 'Upload failed. No saved file was created.';
        else { unresolvedUploadId = uploadId; unresolvedUploadName = file.name; notice = 'Upload outcome could not be confirmed. Check status before retrying.'; }
      } else notice = 'Upload failed before it was admitted. No saved file is implied.';
    } finally {
      if (generation !== sessionGeneration) return;
      uploadCancelable = false;
      transferBusy = false;
      resetTransferActivity();
      controller = null;
      if (!unresolvedUploadId) uploadId = '';
      if (input) { input.value = ''; selectedFileName = ''; }
    }
  }

  async function reconcileUpload(): Promise<void> {
    if (!unresolvedUploadId || transferBusy) return;
    const generation = sessionGeneration;
    const activeTransfers = transfers;
    transferBusy = true;
    let status;
    try { status = await activeTransfers.status(unresolvedUploadId); }
    catch (error) {
      if (generation !== sessionGeneration) return;
      if (error instanceof HttpError && error.status === 404) {
        try {
          await activeTransfers.retryAdmission(unresolvedUploadId);
          if (generation !== sessionGeneration) return;
          status = await activeTransfers.status(unresolvedUploadId);
        } catch (retryError) {
          if (generation !== sessionGeneration) return;
          if (retryError instanceof HttpError && retryError.status === 410) {
            // An expired ticket can no longer be admitted, so a second 404 is final.
            let admitted = true;
            try { status = await activeTransfers.status(unresolvedUploadId); }
            catch (finalError) { admitted = !(finalError instanceof HttpError && finalError.status === 404); }
            if (generation !== sessionGeneration) return;
            if (!admitted) {
              activeTransfers.forget(unresolvedUploadId);
              unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false;
              notice = 'The upload was never admitted and its ticket expired. No saved file was created.';
              transferBusy = false;
              return;
            }
          }
        }
        if (!status) {
          notice = 'Admission could not be confirmed. The original request is retained; retry checking before starting another upload.';
          transferBusy = false;
          return;
        }
      } else {
        notice = 'Upload status is unavailable. Try checking again before starting another upload.';
        transferBusy = false;
        return;
      }
    }
    if (generation !== sessionGeneration) return;
    const outcome = status.state === 'committed' ? 'committed' : status.state === 'rejected' && status.status === 'canceled' ? 'canceled' : status.state === 'rejected' ? 'rejected' : 'unknown';
    if (outcome === 'committed') {
      uploadId = unresolvedUploadId;
      unresolvedUploadId = '';
      unresolvedCancel = false;
      await showCommittedUpload();
      if (generation !== sessionGeneration) return;
      if (!uploadReplacement) notice = 'Upload complete. The saved file is listed below.';
    } else if (outcome === 'canceled' || outcome === 'rejected') {
      unresolvedUploadId = '';
      uploadId = '';
      unresolvedCancel = false;
      notice = outcome === 'canceled' ? 'Upload canceled. No saved file was created.' : 'Upload was rejected. No saved file was created.';
    } else {
      unresolvedCancel = true;
      notice = 'Upload is admitted or still processing. You can retry cancellation or check status again.';
    }
    transferBusy = false;
  }

  async function retryCancel(): Promise<void> {
    if (!unresolvedCancel || !unresolvedUploadId || transferBusy) return;
    const generation = sessionGeneration;
    const activeTransfers = transfers;
    transferBusy = true;
    try {
      await activeTransfers.cancel(unresolvedUploadId);
      if (generation !== sessionGeneration) return;
      unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false;
      notice = 'Upload canceled. No saved file was created.';
    } catch {
      if (generation !== sessionGeneration) return;
      const outcome = await uploadOutcome(unresolvedUploadId);
      if (generation !== sessionGeneration) return;
      if (outcome === 'canceled' || outcome === 'rejected') {
        unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false;
        notice = outcome === 'canceled' ? 'Upload canceled. No saved file was created.' : 'Upload was rejected. No saved file was created.';
      } else if (outcome === 'committed') {
        unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false;
        await showCommittedUpload();
      } else notice = 'Cancellation is still unconfirmed. Retry cancellation or check status.';
    } finally { if (generation === sessionGeneration) transferBusy = false; }
  }

  async function download(entry: Entry): Promise<void> {
    try {
      const anchor = document.createElement('a');
      anchor.href = listing.downloadUrl(entry.id);
      anchor.target = '_blank';
      anchor.rel = 'noopener';
      anchor.download = entry.name;
      anchor.click();
      notice = 'Download requested. Check your browser for progress.';
    } catch { notice = 'Download could not be completed.'; }
  }

  async function openFolder(folder: Folder): Promise<void> {
    const index = folderPath.findIndex((item) => item.id === folder.id);
    folderPath = index >= 0 ? folderPath.slice(0, index + 1) : [...folderPath, folder];
    currentFolder = folder; await refresh();
  }
</script>

<svelte:window ondragover={externalDrag} ondrop={externalDrag} />

<svelte:head>
  <title>Sylux — private files</title>
  <meta name="description" content="Private file storage proof of concept" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</svelte:head>

<div class="flex min-h-screen flex-col">
  <header class="border-b">
    <div class="mx-auto flex w-full max-w-6xl flex-wrap items-center justify-between gap-3 px-4 py-3">
      <a class="rounded-sm text-lg font-semibold tracking-tight" href="#home" aria-label="Sylux home">Sylux</a>
      <div class="flex flex-wrap items-center justify-end gap-3">
        <HeaderIconButton label={darkMode ? 'Switch to light mode' : 'Switch to dark mode'} pressed={darkMode} onclick={toggleTheme}>{#if darkMode}<SunIcon />{:else}<MoonIcon />{/if}</HeaderIconButton>
        {#if user && !loggingOut}<span class="text-muted-foreground text-sm break-all">{user.handle}</span><HeaderIconButton label="Account" disabled={loggingOut} onclick={openAccounts}><UserRoundIcon /></HeaderIconButton><HeaderIconButton label="Log out" disabled={loggingOut} onclick={signOut}><LogOutIcon /></HeaderIconButton>{/if}
      </div>
    </div>
  </header>
  {#if loggingOut}
    <main class="mx-auto flex w-full max-w-6xl flex-1 items-center px-4 py-8"><p role="status">Signing out…</p></main>
  {:else if !user}
    <main class="mx-auto grid w-full max-w-6xl flex-1 items-center gap-8 px-4 py-8 md:grid-cols-[minmax(0,1fr)_minmax(0,26rem)]">
      <section class="flex flex-col gap-3">
        <p class="text-muted-foreground text-sm font-medium">Your private space</p>
        <h1 class="text-4xl font-semibold tracking-tight">Files, in their<br />own place.</h1>
        <p>A focused home for the things you want to keep close.</p>
        <p class="text-muted-foreground text-sm">Private by default. Your files stay yours.</p>
      </section>
      <Card.Root>
        <section class="flex flex-col gap-6 px-6" aria-labelledby="auth-heading">
          <Tabs.Root bind:value={view}>
            <Tabs.List class="w-full" aria-label="Account access"><Tabs.Trigger value="login" disabled={authBusy}>Sign in</Tabs.Trigger><Tabs.Trigger value="setup" disabled={authBusy}>First-time setup</Tabs.Trigger></Tabs.List>
          <Tabs.Content value={view} class="flex flex-col gap-6 pt-4">
          <div class="flex flex-col gap-1"><h2 class="text-base font-medium" id="auth-heading">{view === 'setup' ? 'Create your account' : 'Welcome back'}</h2><p class="text-muted-foreground text-sm">{view === 'setup' ? 'Set up the first administrator account. Other users should ask their administrator for an account.' : 'Sign in to continue to your files.'}</p></div>
          <form class="flex flex-col gap-4" onsubmit={signIn}>
            {#if view === 'setup'}<div class="grid gap-2"><Label for="setup-auth">One-use setup authorization</Label><p id="setup-help" class="text-muted-foreground text-sm">The server operator reads this from <code class="break-all">secrets/setup.authorization</code> inside the initialized Sylux state directory. Ask your operator if you do not have it. Keep it private.</p><Input aria-describedby="setup-help" id="setup-auth" type="password" bind:value={setupAuthorization} autocomplete="off" required /></div>{/if}
            <div class="grid gap-2"><Label for="handle">Account name</Label><Input id="handle" bind:value={handle} autocomplete="username" pattern={view === 'setup' ? '[a-z0-9][a-z0-9._-]*' : undefined} maxlength={64} required />{#if view === 'setup'}<p class="text-muted-foreground text-xs">Use lowercase letters, numbers, dots, underscores or hyphens. Start with a letter or number.</p>{/if}</div>
            <div class="grid gap-2"><Label for="password">Password</Label><Input id="password" type="password" bind:value={password} autocomplete={view === 'setup' ? 'new-password' : 'current-password'} minlength={view === 'setup' ? 15 : undefined} maxlength={view === 'setup' ? 128 : undefined} required />{#if view === 'setup'}<p class="text-muted-foreground text-xs">Use 15 to 128 characters.</p>{/if}</div>
            <Button type="submit" disabled={authBusy}>{authBusy ? (view === 'setup' ? 'Setting up…' : 'Sign in…') : (view === 'setup' ? 'Set up account' : 'Sign in')}</Button>
          </form>
          </Tabs.Content>
          </Tabs.Root>
          {#if notice}<Alert.Root role="status"><Alert.Description>{notice}</Alert.Description></Alert.Root>{/if}
          <p class="text-muted-foreground text-xs">Credentials are sent only through this session.</p>
        </section>
      </Card.Root>
    </main>
  {:else if accountView}
    <main class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-6">
      <div><Button variant="link" class="h-auto p-0" onclick={() => { accountView = false; }}>Back to files</Button><h1 class="mt-3 text-2xl font-semibold">Account</h1></div>
      {#if accountNotice}<Alert.Root role="status"><Alert.Description>{accountNotice}</Alert.Description></Alert.Root>{/if}
      <Card.Root><section class="grid gap-4 px-6"><h2 class="text-lg font-semibold">Change password</h2><p class="text-muted-foreground text-sm">Changing your password signs out all your sessions. Use 15 to 128 characters.</p><form class="grid gap-3" onsubmit={changePassword}><Label for="current-password">Current password</Label><Input id="current-password" type="password" autocomplete="current-password" bind:value={currentPassword} required /><Label for="new-password">New password</Label><Input id="new-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={newPassword} required /><Button type="submit" disabled={accountBusy}>Change password</Button></form></section></Card.Root>
      {#if user.role === 'administrator'}
      <Card.Root><section class="grid gap-4 px-6"><div class="flex flex-wrap items-center justify-between gap-2"><h2 class="text-lg font-semibold" id="accounts-heading" tabindex="-1">Accounts</h2><Button variant="outline" size="sm" disabled={accountsLoading || accountBusy} onclick={loadAccounts}>Refresh accounts</Button></div><p class="text-muted-foreground text-sm">Each account has private files. Disabling an account signs out its sessions.</p>
        {#if accountsLoading}<p role="status">Loading accounts…</p>{:else if accountsError}<p role="alert">{accountsError}</p>{:else if accounts.length === 0}<p>No accounts to show.</p>{/if}
        {#each accounts as account (account.account_id)}<div class="flex flex-wrap items-center justify-between gap-2 border-b py-2"><div><strong>{account.handle}</strong><p class="text-muted-foreground text-sm">{account.role} · {account.enabled ? 'Active' : 'Disabled'}</p></div>{#if account.role === 'user' && account.enabled}<div class="flex gap-2"><Button variant="outline" size="sm" disabled={accountBusy} onclick={() => { resetId = account.account_id; resetPassword = ''; }}>Reset password</Button><Button variant="outline" size="sm" disabled={accountBusy} onclick={() => disableAccount(account)}>Disable</Button></div>{/if}</div>{/each}
        {#if resetId}<form class="grid gap-3 rounded border p-4" onsubmit={resetAccount}><h3 class="font-medium">Reset password for {accounts.find((a) => a.account_id === resetId)?.handle}</h3><p class="text-muted-foreground text-sm">This signs out all their sessions. Share the new password through a private channel.</p><Label for="reset-password">New password</Label><Input id="reset-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={resetPassword} required /><div class="flex gap-2"><Button type="submit" disabled={accountBusy}>Reset password</Button><Button type="button" variant="outline" onclick={() => { resetId = ''; resetPassword = ''; }}>Cancel</Button></div></form>{/if}
        <form class="grid gap-3" onsubmit={createAccount}><h3 class="font-medium">Create account</h3><p class="text-muted-foreground text-sm">Use 15 to 128 characters for the password. Share it through a private channel.</p><Label for="account-handle">Account name</Label><Input id="account-handle" bind:value={accountHandle} pattern="[a-z0-9][a-z0-9._-]*" maxlength={64} required /><Label for="account-password">Password</Label><Input id="account-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={accountPassword} required /><Button type="submit" disabled={accountBusy}>Create account</Button></form>
      </section></Card.Root>{/if}
    </main>
  {:else}
    <main class="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-6 px-4 py-6">
      <div class="grid items-start gap-4 md:grid-cols-[11rem_minmax(0,1fr)] md:gap-6">
        <aside class="flex flex-wrap gap-2 md:flex-col" aria-label="Workspace navigation">
          <Button variant={showTrash ? 'ghost' : 'secondary'} class={`justify-start ${dropTarget === folders[0]?.id ? 'drop-active' : ''}`} data-drop-target={folders[0]?.id} data-drop-active={dropTarget === folders[0]?.id} ondragover={(event) => folders[0] && dragOver(event, folders[0])} ondragleave={(event) => folders[0] && dragLeave(event, folders[0])} ondrop={(event) => folders[0] && dropEntry(event, folders[0])} onclick={() => { showTrash = false; folders[0] && openFolder(folders[0]); }}>Files</Button>
          <Button variant={showTrash ? 'secondary' : 'outline'} class="justify-start" aria-expanded={showTrash} onclick={openTrash}>Trash</Button>
          {#if entries.some((entry) => entry.kind === 'folder')}<nav class="hidden flex-col gap-1 md:flex" aria-label="Folders"><p class="text-muted-foreground px-2 text-xs">Folders</p>{#each entries.filter((entry): entry is Folder => entry.kind === 'folder') as folder (folder.id)}<Button variant="ghost" class={`h-auto min-h-11 justify-start py-1.5 text-left break-all whitespace-normal ${dropTarget === folder.id ? 'drop-active' : ''}`} data-drop-target={folder.id} data-drop-active={dropTarget === folder.id} ondragover={(event) => dragOver(event, folder)} ondragleave={(event) => dragLeave(event, folder)} ondrop={(event) => dropEntry(event, folder)} onclick={() => openFolder(folder)}>{folder.name}</Button>{/each}</nav>{/if}
        </aside>
        <Card.Root class="min-w-0">
          <section class="flex flex-col gap-4 px-6" aria-labelledby="folder-title">
            {#if showTrash}<section class="grid gap-2 rounded-lg border p-4" aria-labelledby="trash-heading" aria-busy={trashLoading}><h2 id="trash-heading" tabindex="-1" class="font-semibold">Trash</h2><p class="text-sm">Items stay here until restored. Permanent removal is not available yet. Restore a parent folder before its children.</p><Button variant="outline" size="sm" disabled={trashLoading || trashBusy} onclick={loadTrash}>Refresh trash</Button>{#if trashError}<p role="alert">{trashError}</p>{/if}{#if trashLoading}<p role="status">Loading trash…</p>{:else if !trashError && deletedItems.length === 0}<p>Trash is empty.</p>{/if}{#if deletedItems.length > 0}<ul>{#each deletedItems as item (item.id)}<li class="flex flex-wrap items-center justify-between gap-2 border-t py-2"><span class="break-all">{item.name} ({item.kind})<br /><span class="text-muted-foreground text-sm">Original folder: Files{item.original_path.length ? ` / ${item.original_path.join(' / ')}` : ''}{#if needsDeletedReference(item)}<br />Item reference: {item.id}{/if}</span></span><Button variant="outline" size="sm" disabled={trashBusy} aria-label={deletedItems.some((other) => other.id !== item.id && other.name === item.name) ? `Restore ${item.name} from Files / ${item.original_path.join(' / ')}${needsDeletedReference(item) ? `, item reference ${item.id}` : ''}` : `Restore ${item.name}`} onclick={() => restoreEntry(item)}>Restore {item.name}</Button></li>{/each}</ul>{/if}</section>{/if}
            {#if unresolvedUploadId}<Alert.Root variant="destructive"><Alert.Title>Upload status needs checking</Alert.Title><Alert.Description><p>{unresolvedUploadName} may have been saved. Check its status before uploading it again.</p><div class="mt-2 flex flex-wrap gap-2"><Button variant="outline" size="sm" onclick={reconcileUpload} disabled={transferBusy}>Check upload status</Button>{#if unresolvedCancel}<Button variant="outline" size="sm" onclick={retryCancel} disabled={transferBusy}>Retry cancellation</Button>{/if}</div></Alert.Description></Alert.Root>{/if}
            <div class="flex flex-wrap items-end justify-between gap-3"><div class="min-w-0">{#if folderPath.length > 1}<nav class="text-muted-foreground flex flex-wrap items-center gap-1 text-sm" aria-label="Folder path">{#each folderPath.slice(0, -1) as folder (folder.id)}<Button variant="link" class="h-auto p-0 break-all whitespace-normal" onclick={() => openFolder(folder)}>{folder.name}</Button><span aria-hidden="true">/</span>{/each}</nav>{/if}<h1 class="text-2xl font-semibold break-all" id="folder-title" tabindex="-1">{currentFolder.name}</h1></div><div class="flex gap-2"><Button variant="outline" disabled={folderBusy || !currentFolder.id} onclick={(event) => beginCreate(event.currentTarget as HTMLElement)}><PlusIcon /><span>Create</span></Button><Button variant="outline" disabled={loading} onclick={refresh} aria-label="Refresh folder" aria-busy={loading}><RefreshCwIcon class={loading ? 'refreshing' : ''} data-refreshing={loading} /><span>Refresh</span></Button></div></div>
            <section class="bg-muted/50 flex flex-wrap items-center justify-between gap-4 rounded-lg border border-dashed p-4" aria-label="Upload files"><div class="flex flex-col"><strong class="font-medium">Bring a file in</strong><span class="text-muted-foreground text-sm">Choose a file to add to this folder</span></div><form class="flex min-w-0 flex-1 basis-full flex-wrap items-center gap-2 sm:basis-72" onsubmit={upload}><Input class="sr-only size-px border-0 p-0" id="upload-file" type="file" tabindex={-1} aria-label="Choose a file to upload" disabled={transferBusy || !!unresolvedUploadId} onchange={(event) => { selectedFileName = event.currentTarget.files?.[0]?.name ?? ''; }} /><Button variant="outline" type="button" disabled={transferBusy || !!unresolvedUploadId} onclick={() => document.getElementById('upload-file')?.click()}>Choose file</Button><Button type="submit" disabled={transferBusy || !!unresolvedUploadId || !!replacementPrompt || !currentFolder.id}>Upload</Button><span class="text-muted-foreground basis-full break-all text-sm" aria-live="polite">{selectedFileName || 'No file chosen'}</span></form>
              {#if transferBusy || progress > 0}<div class="grid basis-full justify-items-start gap-2" role="status"><div class="grid w-full gap-1 text-sm"><span class="break-all">{transferName}</span><div class="flex justify-between gap-3"><span>{transferPhase === 'complete' ? 'Saved' : transferPhase === 'saving' ? 'Saving file' : 'Uploading'}</span><span>{progress}% transferred</span></div></div><Progress value={progress} max={100} aria-label="Upload progress" />{#if transferBusy}<Button variant="link" class="h-auto p-0" onclick={() => { if (uploadCancelable) controller?.abort(); }} disabled={!uploadCancelable}>Cancel upload</Button>{/if}</div>{/if}
            </section>
            <div class="flex items-end justify-between gap-3"><h3 class="text-sm font-medium">Items</h3><span class="text-muted-foreground text-xs">{loading ? 'Loading…' : `${entries.length} shown`}</span></div>
            {#if loading}<div class="flex flex-col items-center gap-1 rounded-lg border border-dashed px-4 py-10 text-center" role="status"><strong class="font-medium">Loading your files…</strong><p class="text-muted-foreground text-sm">This should only take a moment.</p></div>
            {:else if listingError}<Alert.Root variant="destructive" role="alert"><Alert.Title>We couldn’t open this folder</Alert.Title><Alert.Description><p>{listingError}</p><Button variant="outline" size="sm" class="mt-2" onclick={refresh}>Try again</Button></Alert.Description></Alert.Root>
            {:else if entries.length === 0}<div class="flex flex-col items-center gap-1 rounded-lg border border-dashed px-4 py-10 text-center"><strong class="font-medium">Nothing here just yet</strong><p class="text-muted-foreground text-sm">Upload a file or create a folder to get started.</p></div>
            {:else}<div class="divide-y border-y">{#each entries as entry (entry.id)}
              <article aria-label={entry.name} draggable={!dropPending && !moveBusy && !moveNeedsRefresh} ondragstart={(event) => startDrag(event, entry)} ondragend={endDrag} ondragover={(event) => entry.kind === 'folder' && dragOver(event, entry)} ondragleave={(event) => entry.kind === 'folder' && dragLeave(event, entry)} ondrop={(event) => entry.kind === 'folder' && dropEntry(event, entry)} data-drop-target={entry.kind === 'folder' ? entry.id : undefined} data-drop-active={dropTarget === entry.id} class={`flex items-center gap-3 rounded-sm py-2 ${dropTarget === entry.id ? 'drop-active' : ''}`}>
                <span class="text-muted-foreground shrink-0" aria-hidden="true">{#if entry.kind === 'folder'}<FolderIcon class="size-4" />{:else}<FileIcon class="size-4" />{/if}</span>
                <div class="flex min-w-0 flex-1 flex-col"><button type="button" class="item-name min-h-9 text-left font-medium break-all" aria-label={`${entry.kind === 'folder' ? 'Open' : 'Preview'} ${entry.name}`} onclick={(event) => entry.kind === 'folder' ? openFolder(entry) : openPreview(entry, event.currentTarget)}>{entry.name}</button><span class="text-muted-foreground text-xs">{entry.kind === 'folder' ? 'Folder' : `${entry.size} bytes`}</span></div>
                <EntryOptions {entry} disabled={!!dropPending || moveBusy} moveDisabled={moveNeedsRefresh} onaction={(action, trigger) => entryAction(action, entry, trigger)} />
              </article>
            {/each}</div>{/if}
            {#if dropPending}<p role="status">Moving {dropPending.entry.name} to {dropPending.destination.name}…</p>{/if}
            {#if dropError}<p role="alert">{dropError}</p>{/if}
            {#if pageIndex > 0 || hasMore}<div class="flex flex-wrap gap-2">{#if pageIndex > 0}<Button variant="outline" size="sm" onclick={loadPrevious} disabled={loading}>Previous page</Button>{/if}{#if hasMore}<Button variant="outline" size="sm" onclick={loadMore} disabled={loading}>Next page</Button>{/if}</div>{/if}
            {#if notice}<Alert.Root role="status"><Alert.Description>{notice}</Alert.Description></Alert.Root>{/if}
          </section>
        </Card.Root>
      </div>
    </main>
    <FilePreview entry={previewEntry} url={previewEntry ? listing.downloadUrl(previewEntry.id) : ''} {client} onclose={closePreview} ondownload={() => { if (previewEntry) void download(previewEntry); }} />
    <Dialog.Root open={createOpen} onOpenChange={(open) => { if (!folderBusy) createOpen = open; }}>
      <Dialog.Content showCloseButton={!folderBusy} onOpenAutoFocus={(event) => { event.preventDefault(); folderInput?.focus(); }} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(createTrigger); createTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Create folder</Dialog.Title><Dialog.Description>Add a folder to {currentFolder.name}.</Dialog.Description></Dialog.Header>
        <form class="grid gap-6" aria-busy={folderBusy} onsubmit={createFolder}><div class="grid gap-2"><Label for="new-folder">Folder name</Label><Input id="new-folder" bind:ref={folderInput} bind:value={folderName} disabled={folderBusy} maxlength={255} aria-invalid={!!folderError} aria-describedby={folderError ? 'folder-error' : undefined} required /></div>{#if folderError}<p id="folder-error" role="alert">{folderError}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={folderBusy} onclick={() => createOpen = false}>Cancel</Button><Button type="submit" disabled={folderBusy}>{folderBusy ? 'Creating…' : 'Create folder'}</Button></Dialog.Footer></form>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!moveTarget} onOpenChange={(open) => { if (!open && !moveBusy) closeMove(); }}>
      <Dialog.Content class="max-h-[85dvh] overflow-y-auto" onCloseAutoFocus={(event) => { event.preventDefault(); if (moveTrigger?.isConnected) moveTrigger.focus(); else document.getElementById('folder-title')?.focus(); moveTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Move {moveTarget?.name}</Dialog.Title><Dialog.Description>Current folder: {folderPath.map((folder) => folder.name).join(' / ')}. Choose a destination in your private storage.</Dialog.Description></Dialog.Header>
        <nav aria-label="Move destination path" class="flex flex-wrap gap-2">{#each movePath as folder, index (folder.id)}<Button variant="link" class="h-auto p-0 break-all whitespace-normal" disabled={moveLoading || moveBusy} aria-current={index === movePath.length - 1 ? 'page' : undefined} onclick={() => loadMoveFolder(movePath.slice(0, index + 1))}>{folder.name}</Button>{/each}</nav>
        <div class="grid gap-2 rounded-lg border p-3" aria-label="Destination folders" aria-busy={moveLoading}>
          {#if moveLoading}<p role="status">Loading destinations...</p>{:else if moveFolders.length === 0}<p class="text-muted-foreground text-sm">No folders here.</p>{/if}
          {#each moveFolders as folder (folder.id)}<Button variant="ghost" class="h-auto min-h-9 justify-start text-left break-all whitespace-normal" disabled={moveLoading || moveBusy || folder.id === moveTarget?.id} onclick={() => loadMoveFolder([...movePath, folder])}><FolderIcon class="size-4 shrink-0" />{folder.name}{folder.id === moveTarget?.id ? ' (cannot move into itself)' : ''}</Button>{/each}
          {#if moveCursor}<Button variant="outline" disabled={moveLoading || moveBusy} onclick={() => loadMoveFolder(movePath, moveCursor)}>More destination folders</Button>{/if}
        </div>
        <p class="text-sm break-all">Destination: {movePath.map((folder) => folder.name).join(' / ') || 'Choose a folder'}</p>
        {#if movePath.at(-1)?.id === moveTarget?.parent_id}<p class="text-muted-foreground text-sm">This item is already here. Open another folder or choose an ancestor above.</p>{/if}
        {#if moveError}<p role="alert">{moveError}</p>{#if moveNeedsRefresh}<Button variant="outline" onclick={async () => { closeMove(); await refresh(); }}>Close and refresh files</Button>{:else}<Button variant="outline" disabled={moveBusy || moveLoading} onclick={() => loadMoveFolder(movePath.length ? movePath : folders.slice(0, 1))}>Refresh destinations</Button>{/if}{/if}
        <Dialog.Footer><Button variant="outline" disabled={moveBusy} onclick={closeMove}>Cancel</Button><Button disabled={moveBusy || moveNeedsRefresh || moveLoading || !moveDestinationReady || !movePath.length || movePath.at(-1)?.id === moveTarget?.parent_id} onclick={moveEntry}>{moveBusy ? 'Moving...' : 'Move here'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!replacementPrompt} onOpenChange={(open) => { if (!open) replacementPrompt = null; }}>
      <Dialog.Content onCloseAutoFocus={(event) => { event.preventDefault(); replacementTrigger?.focus(); replacementTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Replace {replacementPrompt?.target.name}?</Dialog.Title><Dialog.Description>A file with this name already exists in {replacementPrompt?.folder.name}. The previous file will move to trash and stay there until restored. Rename the current file before restoring it. If this upload fails or is canceled before saving, the previous file stays unchanged.</Dialog.Description></Dialog.Header>
        <Dialog.Footer><Button variant="outline" onclick={() => replacementPrompt = null}>Keep existing file</Button><Button onclick={confirmReplacement}>Replace file</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!trashTarget} onOpenChange={(open) => { if (!open && !trashBusy) trashTarget = null; }}>
      <Dialog.Content onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(trashTrigger); trashTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Delete {trashTarget?.name}?</Dialog.Title><Dialog.Description>Move this item to trash. You can restore it later. Folders must be empty. Permanent removal is not available yet.</Dialog.Description></Dialog.Header>
        {#if trashError}<p role="alert">{trashError}</p>{/if}
        <Dialog.Footer><Button variant="outline" disabled={trashBusy} onclick={() => trashTarget = null}>Cancel</Button><Button disabled={trashBusy} onclick={deleteEntry}>Move to trash</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!renameId} onOpenChange={(open) => { if (!open && !renameBusy) closeRename(); }}>
      <Dialog.Content onOpenAutoFocus={(event) => { event.preventDefault(); renameInput?.focus(); }} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(renameTrigger); renameTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Rename item</Dialog.Title><Dialog.Description>Choose a name that is not already used in this folder.</Dialog.Description></Dialog.Header>
        <form class="grid gap-6" onsubmit={renameEntry}><div class="grid gap-2"><Label for="rename-name">New name</Label><Input id="rename-name" bind:ref={renameInput} bind:value={renameName} disabled={renameBusy} maxlength={255} aria-invalid={!!renameError} aria-describedby={renameError ? 'rename-error' : undefined} required /></div>{#if renameError}<p id="rename-error" role="alert">{renameError}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={renameBusy} onclick={closeRename}>Cancel</Button><Button type="submit" disabled={renameBusy}>{renameBusy ? 'Saving…' : 'Save name'}</Button></Dialog.Footer></form>
      </Dialog.Content>
    </Dialog.Root>
  {/if}
  <footer class="border-t"><div class="text-muted-foreground mx-auto flex w-full max-w-6xl flex-wrap items-center gap-3 px-4 py-4 text-xs"><span>Sylux · private file storage</span><Badge variant="outline">PoC</Badge><span>Durability not qualified. Keep another copy of important files.</span></div></footer>
</div>

<style>
  .item-name { border-radius: .2rem; text-underline-offset: .2em; }
  .item-name:hover { color: var(--primary); text-decoration: underline; }
  .item-name:focus-visible { outline: 2px solid var(--ring); outline-offset: 2px; }
  :global(.drop-active) { background: var(--accent); outline: 2px solid var(--primary); outline-offset: 2px; }
  :global(.refreshing) { animation: refresh-turn 1s linear infinite; }
  @keyframes refresh-turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { :global(.refreshing) { animation: none; } }
</style>

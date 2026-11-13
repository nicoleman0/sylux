<script lang="ts">
  import './app.css';
  import * as Sidebar from '$lib/components/ui/sidebar/index.js';
  import AppSidebar from '$lib/components/app-sidebar.svelte';
  import { onMount, tick } from 'svelte';
  import MoonIcon from '@lucide/svelte/icons/moon';
  import SunIcon from '@lucide/svelte/icons/sun';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import UserRoundIcon from '@lucide/svelte/icons/user-round';
  import LogOutIcon from '@lucide/svelte/icons/log-out';
  import EntryOptions from './lib/EntryOptions.svelte';
  import HeaderIconButton from './lib/HeaderIconButton.svelte';
  import FilePreview from './lib/FilePreview.svelte';
  import FileIcon from '@lucide/svelte/icons/file';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import CheckIcon from '@lucide/svelte/icons/check';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import XIcon from '@lucide/svelte/icons/x';
  import { ApiClient, HttpError, HttpTransport, MutationApi } from './lib/api';
  import { ListingApi, type Entry, type Folder, type DeletedEntry, type SearchHit, type FolderSort } from './lib/listing';
  import { SessionApi, type User, type Account } from './lib/session';
  import { TransferApi, type Replacement } from './lib/transfers';
  import { filesToUpload, snapshotDesktopDrop, type UploadItem } from './lib/desktop-drop';

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
    void loadInstance();
    window.addEventListener('popstate', restoreNavigation);
    return () => { preference.removeEventListener('change', onChange); window.removeEventListener('popstate', restoreNavigation); };
  });
  let user = $state<User | null>(null);
  let accountView = $state(false);
  let settingsView = $state(false);
  let settingsTab = $state<'accounts' | 'instance'>('accounts');
  let instanceName = $state('Sylux');
  let instanceDraft = $state('Sylux');
  let instanceBusy = $state(false);
  let instanceLoading = $state(false);
  let instanceError = $state('');
  let instanceNotice = $state('');
  let instanceRequest = 0;
  let createAccountOpen = $state(false);
  let changePasswordOpen = $state(false);
  let passwordDialogTrigger: HTMLElement | null = null;
  let disableTarget = $state<Account | null>(null);
  let accountDialogTrigger: HTMLElement | null = null;
  const navigationPaths = new Map<string, Folder[]>();

  async function loadInstance(): Promise<void> {
    const request = ++instanceRequest;
    instanceLoading = true; instanceError = '';
    try {
      const result = await client.request<{ name: string }>('/instance');
      if (request !== instanceRequest) return;
      if (!result || typeof result.name !== 'string' || !result.name.trim()) throw new Error('Invalid instance name response');
      instanceName = result.name; instanceDraft = result.name;
    } catch { if (request === instanceRequest) instanceError = 'Could not load the instance name. Try again.'; }
    finally { if (request === instanceRequest) instanceLoading = false; }
  }

  async function saveInstance(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (instanceBusy || !instanceDraft.trim()) return;
    const generation = sessionGeneration;
    instanceBusy = true; instanceError = ''; instanceNotice = '';
    try {
      const result = await client.request<{ name: string }>('/instance', { method: 'PATCH', body: { name: instanceDraft.trim() } });
      if (generation !== sessionGeneration) return;
      if (!result || typeof result.name !== 'string' || !result.name.trim()) throw new Error('Invalid instance name response');
      instanceName = result.name; instanceDraft = result.name; instanceNotice = 'Instance name saved.';
    } catch { if (generation === sessionGeneration) instanceError = 'Could not save the instance name. Try again.'; }
    finally { if (generation === sessionGeneration) instanceBusy = false; }
  }

  function recordNavigation(): void {
    const destination = settingsView ? 'settings' : accountView ? 'account' : showTrash ? 'trash' : 'files';
    const hash = `#${destination}${destination === 'files' && currentFolder.id ? '/' + encodeURIComponent(currentFolder.id) : ''}`;
    if (window.location.hash !== hash) {
      const key = crypto.randomUUID();
      navigationPaths.set(key, JSON.parse(JSON.stringify(folderPath)));
      window.history.pushState({ sylux: key, destination }, '', hash);
    }
  }

  async function navigate(destination: 'files' | 'trash' | 'account' | 'settings', record = true): Promise<void> {
    if (!user) { view = 'login'; return; }
    if (bulkBusy) return;
    clearSelection(); closeBulk(); hideUploadReport();
    if (destination === 'settings' && user.role !== 'administrator') destination = 'files';
    if (destination !== 'files') clearSearch();
    accountView = destination === 'account'; settingsView = destination === 'settings'; showTrash = destination === 'trash';
    accountNotice = '';
    if (record) recordNavigation();
    if (showTrash) await loadTrash();
    if (settingsView) { await loadAccounts(); await loadInstance(); }
    await tick();
    document.getElementById(accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : 'folder-title')?.focus();
  }

  async function restoreNavigation(event: PopStateEvent): Promise<void> {
    if (!user) return;
    if (bulkBusy) { recordNavigation(); return; }
    closeBulk();
    hideUploadReport();
    clearSearch();
    const generation = sessionGeneration;
    const state = event.state;
    const destination = window.location.hash.slice(1).split('/')[0];
    const target = ['trash', 'account', 'settings'].includes(destination) ? destination as 'trash' | 'account' | 'settings' : 'files';
    const savedPath = typeof state?.sylux === 'string' ? navigationPaths.get(state.sylux) : undefined;
    if (target === 'files' && savedPath?.length) {
      folderPath = savedPath; currentFolder = folderPath[folderPath.length - 1]; await refresh();
    } else if (target === 'files' && folders[0]) {
      folderPath = [folders[0]]; currentFolder = folders[0]; await refresh();
    }
    if (generation !== sessionGeneration) return;
    await navigate(target, false);
  }

  async function home(event: MouseEvent): Promise<void> {
    event.preventDefault();
    if (!user) { view = 'login'; return; }
    if (folders[0]) await openFolder(folders[0]);
    await navigate('files');
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    const units = ['KB', 'MB', 'GB', 'TB']; let value = bytes / 1024; let index = 0;
    while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
    return `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })} ${units[index]}`;
  }
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
  let selectedIds = $state<string[]>([]);
  const selectedEntries = $derived(entries.filter((entry) => selectedIds.includes(entry.id)));

  function clearSelection(): void { selectedIds = []; }
  async function clearSelectionFromToolbar(): Promise<void> {
    const target = document.querySelector<HTMLInputElement>('.selectable-row input:checked');
    clearSelection();
    await tick();
    (target ?? document.getElementById('folder-title'))?.focus();
  }
  function selectEntry(id: string, checked: boolean): void {
    selectedIds = checked ? [...selectedIds.filter((item) => item !== id), id] : selectedIds.filter((item) => item !== id);
  }
  type BulkAction = 'move' | 'trash';
  type BulkResult = { entry: Entry; status: 'pending' | 'succeeded' | 'failed' | 'uncertain' | 'changed'; message: string };
  type BulkReport = { action: BulkAction; sourceId: string; destination: Folder | null; results: BulkResult[] };
  let bulkAction = $state<BulkAction | null>(null);
  let bulkTargets = $state<Entry[]>([]);
  let bulkReport = $state<BulkReport | null>(null);
  let bulkBusy = $state(false);
  let bulkNeedsRefresh = $state(false);
  let bulkError = $state('');
  let bulkRetry = false;
  let bulkTrigger: HTMLElement | null = null;

  let pageCursors = $state<(string | null)[]>([null]);
  let pageIndex = $state(0);
  let nextCursor = $state<string | null>(null);
  let hasMore = $state(false);
  let folderPath = $state<Folder[]>([emptyRoot]);
  let loading = $state(false);
  let listingError = $state('');
  let listingRequest = 0;
  let folderSort = $state<FolderSort>('name_asc');
  let searchInput = $state('');
  let searchQuery = $state('');
  let searchItems = $state<SearchHit[]>([]);
  let searchLoading = $state(false);
  let searchError = $state('');
  let searchInputError = $state('');
  let searchRetryCursor = $state<string | null>(null);
  let searchRetryPage = $state(0);
  let searchPageCursors = $state<(string | null)[]>([null]);
  let searchPageIndex = $state(0);
  let searchNextCursor = $state<string | null>(null);
  let searchHasMore = $state(false);
  let searchRequest = 0;
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let searchController: AbortController | null = null;
  let browseDirty = false;
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
  type ReplacementPrompt = { file: File; folder: Folder; target: { id: string; name: string; etag: string } };
  let replacementPrompt = $state<ReplacementPrompt | null>(null);
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
  let uploadBatchBusy = $state(false);
  let uploadBatchStatus = $state('');
  type UploadResult = { path: string; kind: 'file' | 'folder'; status: 'waiting' | 'active' | 'saved' | 'created' | 'failed' | 'unknown' | 'canceled' };
  let uploadResults = $state<UploadResult[]>([]);
  let uploadResultsExpanded = $state(false);
  let uploadReportVisible = $state(false);
  const uploadComplete = $derived(!uploadBatchBusy && uploadResults.length > 0 && uploadResults.every((item) => item.status === 'saved' || item.status === 'created'));

  function hideUploadReport(): void { uploadReportVisible = false; uploadResultsExpanded = false; }
  let unresolvedUploadResult: number | null = null;
  let uploadBatchCanceled = false;
  let replacementDecision: ((prompt: ReplacementPrompt | null) => void) | null = null;
  const fileMutationBusy = $derived(bulkBusy || bulkNeedsRefresh || moveNeedsRefresh || !!dropPending || moveBusy || renameBusy || trashBusy || folderBusy || transferBusy || uploadBatchBusy);

  function restoreFocus(trigger: HTMLElement | null, fallback = 'folder-title'): void {
    if (trigger?.isConnected && !trigger.matches(':disabled')) trigger.focus();
    else document.getElementById(fallback)?.focus();
  }

  function expireSession(generation: number): void {
    if (generation !== sessionGeneration || !user) return;
    sessionGeneration++; clearAccountForms(); clearWorkspace();
    client.setSessionExpiredHandler(null); client.setCsrfToken(null);
    user = null; accountView = false; settingsView = false; view = 'login';
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
    if (searchQuery) { browseDirty = true; return runSearch(); }
    const generation = sessionGeneration;
    let folderId = currentFolder.id;
    const requestId = ++listingRequest;
    clearSelection();
    loading = true;
    listingError = '';
    try {
      if (!folderId) {
        if (!await refreshRoot() || requestId !== listingRequest) return false;
        currentFolder = folders[0]; folderPath = [currentFolder]; folderId = currentFolder.id;
      }
      const page = await listing.children(folderId, null, folderSort);
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

  async function changeFolderSort(event: Event): Promise<void> {
    await setFolderSort((event.currentTarget as HTMLSelectElement).value as FolderSort);
  }

  async function setFolderSort(sort: FolderSort): Promise<void> {
    hideUploadReport();
    folderSort = sort;
    clearSelection();
    pageCursors = [null]; pageIndex = 0; nextCursor = null; hasMore = false;
    await refresh();
  }

  function clearSearch(restoreBrowse = false): void {
    clearSelection();
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    searchController?.abort(); searchController = null;
    searchRequest++;
    searchInput = ''; searchQuery = ''; searchItems = [];
    searchLoading = false; searchError = ''; searchInputError = ''; searchRetryCursor = null; searchRetryPage = 0;
    searchPageCursors = [null]; searchPageIndex = 0;
    searchNextCursor = null; searchHasMore = false;
    if (restoreBrowse && browseDirty && user) void restoreBrowsingPage();
    else browseDirty = false;
  }

  async function clearSearchFromButton(): Promise<void> {
    clearSearch(true);
    await tick();
    document.getElementById('file-search')?.focus();
  }

  async function restoreBrowsingPage(): Promise<void> {
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const target = pageIndex;
    const cursor = pageCursors[target];
    const request = ++listingRequest;
    loading = true; listingError = '';
    try {
      const page = await listing.children(folderId, cursor, folderSort);
      if (generation !== sessionGeneration || request !== listingRequest || searchQuery || currentFolder.id !== folderId) return;
      entries = page.items; nextCursor = page.next_cursor; hasMore = page.has_more; browseDirty = false;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
    } catch (error) {
      if (generation !== sessionGeneration || request !== listingRequest || searchQuery || currentFolder.id !== folderId) return;
      if (error instanceof HttpError && error.status === 409) {
        await refresh();
        notice = 'This folder changed. The listing has been restarted.';
      } else listingError = 'Could not load this folder. Try again.';
    } finally { if (generation === sessionGeneration && request === listingRequest) loading = false; }
  }

  function searchChanged(): void {
    if (bulkBusy) return;
    hideUploadReport();
    clearSelection();
    if (!searchInput) { clearSearch(true); return; }
    if (searchTimer) clearTimeout(searchTimer);
    searchController?.abort(); searchController = null;
    searchRequest++;
    searchQuery = searchInput;
    searchItems = []; searchError = ''; searchInputError = ''; searchRetryCursor = null; searchRetryPage = 0;
    searchPageCursors = [null]; searchPageIndex = 0;
    searchNextCursor = null; searchHasMore = false;
    if (new TextEncoder().encode(searchQuery).length > 255) {
      searchInputError = 'Search is too long. Try a shorter name.';
      searchLoading = false;
      return;
    }
    if ([...searchQuery].some((char) => /\p{Cc}/u.test(char))) {
      searchInputError = 'Remove control characters from your search.';
      searchLoading = false;
      return;
    }
    searchLoading = true;
    searchTimer = setTimeout(() => { searchTimer = null; void runSearch(); }, 250);
  }

  async function runSearch(cursor: string | null = null, targetPage = 0): Promise<boolean> {
    if (!user || !searchQuery || searchInputError) return false;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    searchController?.abort();
    const controller = new AbortController(); searchController = controller;
    const request = ++searchRequest;
    const generation = sessionGeneration;
    const query = searchQuery;
    searchLoading = true; searchError = '';
    try {
      const page = await listing.search(query, cursor, controller.signal);
      if (request !== searchRequest || generation !== sessionGeneration || query !== searchQuery || accountView || settingsView || showTrash) return false;
      searchItems = page.items;
      moveNeedsRefresh = false;
      searchRetryCursor = null; searchRetryPage = 0;
      searchPageIndex = targetPage;
      searchPageCursors = targetPage === 0 ? [null] : [...searchPageCursors.slice(0, targetPage), cursor];
      searchNextCursor = page.next_cursor; searchHasMore = page.has_more;
      for (const hit of page.items) {
        knownParents.set(hit.entry.id, hit.entry.parent_id);
        for (const folder of hit.location) knownParents.set(folder.id, folder.parent_id);
      }
      return true;
    } catch (error) {
      if (request !== searchRequest || generation !== sessionGeneration || query !== searchQuery) return false;
      if (error instanceof HttpError && error.status === 409) {
        if (cursor) { await runSearch(); notice = 'Files changed. Search results have been restarted.'; }
      } else {
        searchRetryCursor = cursor; searchRetryPage = targetPage;
        searchError = 'Could not search files. Try again.';
      }
      return false;
    } finally {
      if (request === searchRequest && generation === sessionGeneration) { searchLoading = false; searchController = null; }
    }
  }

  async function searchNext(): Promise<void> {
    if (searchHasMore && searchNextCursor && !searchLoading) await runSearch(searchNextCursor, searchPageIndex + 1);
  }

  async function searchPrevious(): Promise<void> {
    if (searchPageIndex > 0 && !searchLoading) await runSearch(searchPageCursors[searchPageIndex - 1], searchPageIndex - 1);
  }

  async function openSearchFolder(hit: SearchHit, containing = false): Promise<void> {
    const path = containing ? hit.location : [...hit.location, hit.entry as Folder];
    const folder = path.at(-1);
    if (!folder) return;
    hideUploadReport();
    clearSearch();
    folderPath = path.map((item, index) => index === 0 ? { ...item, name: 'Files' } : item);
    currentFolder = folderPath.at(-1)!;
    accountView = false; settingsView = false; showTrash = false;
    recordNavigation();
    await refresh();
    await tick(); document.getElementById('folder-title')?.focus();
  }

  async function loadMore(): Promise<void> {
    if (!hasMore || !nextCursor || loading || bulkBusy) return;
    hideUploadReport();
    clearSelection();
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const cursor = nextCursor;
    const requestId = ++listingRequest;
    loading = true; listingError = '';
    try {
      const page = await listing.children(folderId, cursor, folderSort);
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
    if (pageIndex === 0 || loading || bulkBusy) return;
    hideUploadReport();
    clearSelection();
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const requestId = ++listingRequest;
    loading = true; listingError = '';
    const target = pageIndex - 1;
    try {
      const page = await listing.children(folderId, pageCursors[target], folderSort);
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
      await navigate('files');
    } catch {
      if (generation !== sessionGeneration) return;
      notice = authView === 'setup' ? 'Setup could not be completed. Check the authorization and account details, then try again.' : 'Sign-in could not be completed. Check the details and try again.';
      password = '';
    } finally { if (generation === sessionGeneration) authBusy = false; }
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
      createAccountOpen = false; accountHandle = ''; accountPassword = ''; accountNotice = 'Account created. Share the password through a private channel.';
      await loadAccounts();
    } catch { if (generation === sessionGeneration) accountNotice = 'Account could not be created. Check the name and password, then try again.'; }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }
  async function disableAccount(account: Account): Promise<void> {
    const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.disableAccount(account.account_id);
      if (generation !== sessionGeneration) return;
      disableTarget = null; accountNotice = 'Account disabled and sessions signed out.';
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
    clearSearch();
    bulkAction = null; bulkTargets = []; bulkReport = null; bulkBusy = false; bulkNeedsRefresh = false; bulkError = '';
    navigationPaths.clear();
    controller?.abort(); controller = null;
    dismissReplacement(); uploadReplacement = false;
    uploadBatchCanceled = true; uploadBatchBusy = false; uploadBatchStatus = '';
    uploadResults = []; uploadResultsExpanded = false; uploadReportVisible = false; unresolvedUploadResult = null;
    transfers.invalidate(); transfers = new TransferApi(client, mutations);
    transferName = ''; progress = 0; transferPhase = ''; transferBusy = false; uploadCancelable = false;
    uploadId = ''; unresolvedUploadId = ''; unresolvedUploadName = ''; unresolvedCancel = false; notice = '';
    folderSort = 'name_asc';
    listingRequest++; currentFolder = emptyRoot; folders = []; entries = []; folderPath = [emptyRoot];
    pageCursors = [null]; pageIndex = 0; nextCursor = null; hasMore = false; loading = false; listingError = '';
    folderName = ''; folderBusy = false; createOpen = false; folderError = ''; previewEntry = null; previewTrigger = null;
    draggedEntry = null; dropTarget = ''; dropPending = null; dropError = ''; moveNeedsRefresh = false; knownParents.clear();
    renameId = ''; renameName = ''; renameError = ''; renameBusy = false;
    closeMove(); moveBusy = false;
    trashRequest++; deletedItems = []; showTrash = false; trashError = ''; trashTarget = null; trashBusy = false; trashLoading = false;
  }
  function clearAccountForms(): void {
    changePasswordOpen = false; createAccountOpen = false; disableTarget = null; instanceBusy = false; instanceNotice = '';
    accountHandle = ''; accountPassword = ''; currentPassword = ''; newPassword = '';
    resetId = ''; resetPassword = ''; accounts = []; accountNotice = ''; accountBusy = false; accountsLoading = false; accountsError = ''; accountsRequest++;
  }
  async function changePassword(event: SubmitEvent): Promise<void> {
    event.preventDefault(); const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.changePassword(currentPassword, newPassword);
      if (generation !== sessionGeneration) return;
      sessionGeneration++; clearAccountForms(); clearWorkspace(); client.setSessionExpiredHandler(null);
      user = null; accountView = false; settingsView = false; client.setCsrfToken(null); notice = 'Password changed. Sign in again.';
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
    loggingOut = true; client.setSessionExpiredHandler(null); sessionGeneration++; clearAccountForms(); clearWorkspace(); accountView = false; settingsView = false; user = null;
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
    const target = searchQuery ? searchItems.find((hit) => hit.entry.id === renameId)?.entry : entries.find((entry) => entry.id === renameId);
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
    if (bulkAction === 'move') { bulkAction = null; bulkTargets = []; }
    moveRequest++; moveTarget = null; movePath = []; moveFolders = []; moveCursor = null; moveLoading = false; moveDestinationReady = false; moveError = '';
  }
  async function loadMoveFolder(path: Folder[], cursor: string | null = null): Promise<void> {
    const destination = path.at(-1);
    if (!destination || (!moveTarget && bulkAction !== 'move')) return;
    const request = ++moveRequest;
    const generation = sessionGeneration;
    moveLoading = true; moveDestinationReady = false; moveError = ''; movePath = path;
    if (!cursor) { moveFolders = []; moveCursor = null; }
    try {
      const page = await listing.children(destination.id, cursor);
      if (request !== moveRequest || generation !== sessionGeneration || (!moveTarget && bulkAction !== 'move')) return;
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
    if (bulkBusy || bulkNeedsRefresh || dropPending || moveNeedsRefresh) return;
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

  async function beginBulk(action: BulkAction, trigger: HTMLElement, targets = selectedEntries, retry = false): Promise<void> {
    if (!targets.length || fileMutationBusy || loading || searchQuery) return;
    closeMove();
    bulkTargets = targets.map((entry) => ({ ...entry }));
    bulkAction = action; bulkRetry = retry; bulkTrigger = trigger; bulkError = '';
    if (action === 'move') {
      const root = folders[0];
      if (root) await loadMoveFolder([root]);
      else moveError = 'Your storage is unavailable. Close this dialog and refresh your files.';
    }
  }

  function closeBulk(): void {
    if (bulkBusy) return;
    if (bulkAction === 'move') closeMove();
    bulkAction = null; bulkTargets = [];
  }

  function bulkDestinationBlocked(folder: Folder): boolean {
    return bulkTargets.some((entry) => entry.id === folder.id);
  }

  async function runBulk(): Promise<void> {
    const action = bulkAction;
    const destination = action === 'move' ? movePath.at(-1) ?? null : null;
    if (!action || !bulkTargets.length || fileMutationBusy || loading ||
      (action === 'move' && (!destination || moveLoading || !moveDestinationReady ||
        bulkDestinationBlocked(destination) || bulkTargets.every((entry) => entry.parent_id === destination.id)))) return;
    const generation = sessionGeneration;
    const targets = bulkTargets.map((entry) => ({ ...entry }));
    const report: BulkReport = bulkRetry && bulkReport ? bulkReport : {
      action, sourceId: currentFolder.id, destination, results: targets.map((entry) => ({ entry, status: 'pending', message: 'Waiting.' })),
    };
    report.destination = destination;
    bulkReport = report;
    bulkBusy = true; bulkError = ''; clearSelection();
    for (const target of targets) {
      if (generation !== sessionGeneration) return;
      const result = bulkReport!.results.find((item) => item.entry.id === target.id)!;
      result.status = 'pending'; result.message = action === 'move' ? 'Moving…' : 'Moving to trash…';
      try {
        if (action === 'move') await listing.move(target.id, destination!.id, target.etag);
        else await listing.trash(target.id, target.etag);
        if (generation !== sessionGeneration) return;
        result.status = 'succeeded'; result.message = action === 'move' ? `Moved to ${destination!.name}.` : 'Moved to trash.';
      } catch (error) {
        if (generation !== sessionGeneration) return;
        result.status = moveOutcomeUncertain(error) ? 'uncertain' : 'failed';
        result.message = result.status === 'uncertain' ? 'Outcome unconfirmed. Refresh before retrying.' :
          action === 'move' ? moveFailure(error) : error instanceof HttpError && error.status === 403 ? 'You do not have permission to move this item to trash.' : error instanceof HttpError && error.status === 409 ? 'Folder is not empty. Empty it before retrying.' :
          error instanceof HttpError && error.status === 412 ? 'This item changed. Refresh before retrying.' : 'Could not move to trash. Refresh before retrying.';
        if (result.status === 'uncertain') bulkNeedsRefresh = true;
        if (error instanceof HttpError && error.status === 401) break;
      }
    }
    if (generation !== sessionGeneration) return;
    if (action === 'move') closeMove();
    bulkAction = null; bulkTargets = [];
    const succeeded = bulkReport!.results.filter((result) => result.status === 'succeeded').length;
    notice = `${succeeded} of ${bulkReport!.results.length} items ${action === 'move' ? 'moved' : 'moved to trash'}.`;
    try {
      await refreshRoot();
      if (generation === sessionGeneration && !await refresh()) bulkError = 'Could not refresh your files. The item results are kept below.';
    } catch { if (generation === sessionGeneration) bulkError = 'Could not refresh your files. The item results are kept below.'; }
    finally { if (generation === sessionGeneration) bulkBusy = false; }
  }

  async function folderEntries(folderId: string, generation: number): Promise<Entry[]> {
    const items: Entry[] = [];
    const cursors = new Set<string>();
    let cursor: string | null = null;
    do {
      if (generation !== sessionGeneration) throw new Error('Session changed');
      const page = await listing.children(folderId, cursor);
      if (generation !== sessionGeneration) throw new Error('Session changed');
      items.push(...page.items);
      cursor = page.next_cursor;
      if (cursor && cursors.has(cursor)) throw new Error('Repeated listing cursor');
      if (cursor) cursors.add(cursor);
    } while (cursor);
    return items;
  }

  async function reconcileBulk(): Promise<boolean> {
    const report = bulkReport;
    if (!report || bulkBusy) return false;
    const generation = sessionGeneration;
    bulkBusy = true; bulkError = ''; clearSelection();
    try {
      const source = await folderEntries(report.sourceId, generation);
      if (generation !== sessionGeneration) return false;
      const destination = report.action === 'move' && report.destination ? await folderEntries(report.destination.id, generation) : [];
      if (generation !== sessionGeneration) return false;
      const deleted = report.action === 'trash' ? (await listing.deleted()).items : [];
      if (generation !== sessionGeneration) return false;
      for (const result of report.results) {
        if (result.status === 'succeeded') continue;
        const live = source.find((entry) => entry.id === result.entry.id);
        const completed = destination.some((entry) => entry.id === result.entry.id) || deleted.some((entry) => entry.id === result.entry.id);
        if (completed) { result.status = 'succeeded'; result.message = report.action === 'move' ? `Moved to ${report.destination!.name}.` : 'Moved to trash.'; }
        else if (live && live.etag === result.entry.etag) { result.status = 'failed'; result.message = 'Still in the original folder. Ready to retry.'; }
        else if (live) { result.status = 'changed'; result.message = 'This item changed. Select it again to act on its current version.'; }
        else { result.status = 'changed'; result.message = 'Outcome could not be confirmed. Item is no longer in the original folder. Check its location before selecting it again.'; }
      }
      bulkNeedsRefresh = report.results.some((result) => result.status === 'uncertain');
      if (!await refresh()) {
        if (generation === sessionGeneration) bulkError = 'Could not refresh your files. The item results are kept below. Refresh before retrying.';
        return false;
      }
      return generation === sessionGeneration;
    } catch {
      if (generation === sessionGeneration) bulkError = 'Could not check the item results. Check your connection and refresh before retrying.';
      return false;
    } finally { if (generation === sessionGeneration) bulkBusy = false; }
  }

  async function retryBulk(trigger: HTMLElement): Promise<void> {
    if (!bulkReport || !await reconcileBulk()) return;
    const targets = bulkReport.results.filter((result) => result.status === 'failed').map((result) => result.entry);
    if (!targets.length) return;
    if (bulkNeedsRefresh) return;
    await beginBulk(bulkReport.action, trigger, targets, true);
  }

  function moveFailure(error: unknown): string {
    if (error instanceof HttpError) {
      if (error.status === 403) return 'You do not have permission to move this item.';
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
    if (!entry || fileMutationBusy || destination.id === entry.id || destination.id === entry.parent_id) return false;
    const seen = new Set<string>();
    let ancestor: string | null | undefined = destination.id;
    while (ancestor) {
      if (ancestor === entry.id || seen.has(ancestor)) return false;
      seen.add(ancestor); ancestor = knownParents.get(ancestor);
    }
    return true;
  }

  function startDrag(event: DragEvent, entry: Entry): void {
    if (!event.dataTransfer || fileMutationBusy) { event.preventDefault(); return; }
    draggedEntry = entry; dropError = '';
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData(dragType, entry.id);
  }

  function endDrag(): void { draggedEntry = null; dropTarget = ''; }

  function dragOver(event: DragEvent, destination: Folder): void {
    if (isDesktopDrag(event)) {
      event.preventDefault(); event.stopPropagation();
      event.dataTransfer!.dropEffect = canUploadDrop() ? 'copy' : 'none';
      if (canUploadDrop()) dropTarget = destination.id;
      return;
    }
    if (!event.dataTransfer?.types.includes(dragType) || !canDrop(draggedEntry, destination)) return;
    event.preventDefault(); event.stopPropagation(); event.dataTransfer.dropEffect = 'move'; dropTarget = destination.id;
  }

  function dragLeave(event: DragEvent, destination: Folder): void {
    if (event.currentTarget instanceof HTMLElement && event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget)) return;
    if (dropTarget === destination.id) dropTarget = '';
  }

  async function dropEntry(event: DragEvent, destination: Folder): Promise<void> {
    if (isDesktopDrag(event)) { acceptDesktopDrop(event, destination); return; }
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

  function isDesktopDrag(event: DragEvent): boolean {
    return !!event.dataTransfer?.types.includes('Files') && !event.dataTransfer.types.includes(dragType);
  }

  function canUploadDrop(): boolean {
    return !!user && !showTrash && !accountView && !settingsView && !searchQuery && !!currentFolder.id &&
      !fileMutationBusy && !unresolvedUploadId && !replacementPrompt && !createOpen && !renameId && !moveTarget && !trashTarget;
  }

  function acceptDesktopDrop(event: DragEvent, destination: Folder): void {
    event.preventDefault();
    event.stopPropagation(); dropTarget = '';
    if (!canUploadDrop() || !event.dataTransfer) return;
    try {
      const read = snapshotDesktopDrop(event.dataTransfer);
      void runUploadBatch(read, destination);
    } catch (error) {
      uploadResults = []; uploadResultsExpanded = false; uploadReportVisible = true; unresolvedUploadResult = null; notice = '';
      uploadBatchStatus = error instanceof Error ? error.message : 'The dropped items could not be read. Nothing was uploaded.';
    }
  }

  function externalDrag(event: DragEvent): void {
    if (!isDesktopDrag(event)) return;
    event.preventDefault();
    if (event.type === 'dragover') event.dataTransfer!.dropEffect = 'none';
  }

  function cancelUploadBatch(): void {
    uploadBatchCanceled = true;
    if (uploadCancelable) controller?.abort();
    dismissReplacement();
  }

  async function freshUploadFolder(folder: Folder): Promise<Folder> {
    if (!folder.parent_id) return (await listing.roots()).root;
    let cursor: string | null = null;
    do {
      const page = await listing.children(folder.parent_id, cursor);
      const descriptor = page.items.find((entry): entry is Folder => entry.id === folder.id && entry.kind === 'folder');
      if (descriptor) return descriptor;
      cursor = page.next_cursor;
    } while (cursor);
    throw new Error('The upload destination is no longer available.');
  }

  function uploadResultLabel(result: UploadResult): string {
    if (result.status === 'waiting') return uploadBatchBusy ? 'Waiting' : 'Not attempted';
    return { active: result.kind === 'folder' ? 'Creating folder' : 'Uploading', saved: 'Saved', created: 'Folder created', failed: 'Failed', unknown: 'Outcome unknown', canceled: 'Canceled' }[result.status];
  }

  function resolveUploadResult(status: 'saved' | 'failed' | 'canceled'): void {
    if (unresolvedUploadResult === null || !uploadResults[unresolvedUploadResult]) return;
    uploadResults[unresolvedUploadResult].status = status;
    unresolvedUploadResult = null;
    uploadBatchStatus = 'Upload stopped.';
  }

  async function runUploadBatch(read: (canceled: () => boolean) => Promise<UploadItem[]>, destination: Folder): Promise<void> {
    if (uploadBatchBusy || transferBusy || unresolvedUploadId) return;
    const generation = sessionGeneration;
    uploadReportVisible = true;
    uploadBatchBusy = true; uploadBatchCanceled = false; uploadBatchStatus = 'Reading files and folders…'; notice = '';
    uploadResults = []; uploadResultsExpanded = false; unresolvedUploadResult = null;
    let saved = 0; let created = 0; let complete = false;
    let currentPath = '';
    let currentIndex = -1;
    let creatingFolder = false;
    try {
      const items = await read(() => uploadBatchCanceled || generation !== sessionGeneration);
      if (generation !== sessionGeneration) return;
      if (!items.length) { uploadBatchStatus = 'No files or folders were selected.'; return; }
      uploadResults = items.map((item) => ({ path: item.path.join('/'), kind: item.kind, status: 'waiting' }));
      const destinations = new Map<string, Folder>([['[]', destination]]);
      for (const [index, item] of items.entries()) {
        if (uploadBatchCanceled || generation !== sessionGeneration) break;
        currentIndex = index;
        uploadResults[index].status = 'active';
        currentPath = item.path.join('/');
        const remaining = uploadResults.filter((result) => result.kind === 'file' && result.status === 'waiting').length;
        uploadBatchStatus = `Adding ${currentPath} to ${destination.name}… ${remaining} ${remaining === 1 ? 'file' : 'files'} left after this item.`;
        const parent = destinations.get(JSON.stringify(item.path.slice(0, -1)));
        if (!parent) throw new Error('The destination for this item could not be found.');
        if (item.kind === 'folder') {
          creatingFolder = true;
          const folder = await listing.createFolder(parent.id, item.path.at(-1)!, parent.etag);
          if (generation !== sessionGeneration) return;
          creatingFolder = false;
          destinations.set(JSON.stringify(item.path), folder); created++;
          uploadResults[index].status = 'created';
        } else {
          const folder = await freshUploadFolder(parent);
          if (generation !== sessionGeneration || uploadBatchCanceled) break;
          let committed = await startUpload(item.file, null, undefined, folder);
          if (generation !== sessionGeneration) return;
          if (!committed && replacementPrompt && !uploadBatchCanceled) {
            const prompt = await new Promise<ReplacementPrompt | null>((resolve) => { replacementDecision = resolve; });
            if (generation !== sessionGeneration) return;
            if (prompt && !uploadBatchCanceled) {
              const refreshed = await freshUploadFolder(prompt.folder);
              if (generation !== sessionGeneration || uploadBatchCanceled) break;
              committed = await startUpload(prompt.file, null, { entry_id: prompt.target.id, etag: prompt.target.etag }, refreshed);
            } else uploadBatchCanceled = true;
          }
          if (!committed) {
            uploadResults[index].status = unresolvedUploadId ? 'unknown' : uploadBatchCanceled ? 'canceled' : 'failed';
            if (unresolvedUploadId) unresolvedUploadResult = index;
            break;
          }
          saved++;
          uploadResults[index].status = 'saved';
        }
      }
      complete = !uploadBatchCanceled && !unresolvedUploadId && saved + created === items.length;
      if (currentIndex >= 0 && uploadResults[currentIndex].status === 'active') uploadResults[currentIndex].status = 'canceled';
      uploadResultsExpanded = !complete;
      uploadBatchStatus = complete ? '' : `Upload stopped${currentPath ? ` at ${currentPath}` : ''}.`;
    } catch (error) {
      if (generation !== sessionGeneration) return;
      const uncertain = creatingFolder && (!(error instanceof HttpError) || error.status >= 500);
      if (currentIndex >= 0) uploadResults[currentIndex].status = uncertain ? 'unknown' : uploadBatchCanceled ? 'canceled' : 'failed';
      uploadResultsExpanded = true;
      const reason = error instanceof HttpError && error.status === 409 ? 'An item already has this name. Rename it before trying again.' :
        error instanceof HttpError && error.status >= 400 && error.status < 500 ? 'The item could not be added. Check its name and destination.' :
        uncertain ? 'The last item may have been added. Refresh your files before retrying it.' : error instanceof Error ? error.message : 'The dropped items could not be read.';
      uploadBatchStatus = `Upload stopped${currentPath ? ` at ${currentPath}` : ''}. ${reason}`;
    } finally {
      if (generation === sessionGeneration) {
        if (uploadBatchCanceled) dismissReplacement();
        replacementDecision = null;
        try { await refreshRoot(); if (generation === sessionGeneration) await refresh(); } catch { /* Keep the confirmed item counts visible. */ }
        finally {
          if (generation === sessionGeneration) {
            uploadBatchBusy = false;
            if (uploadBatchCanceled) { await tick(); restoreFocus(document.getElementById('choose-file')); }
          }
        }
      }
    }
  }

  function openPreview(entry: Entry, trigger: HTMLElement): void { previewTrigger = trigger; previewEntry = entry; }
  function closePreview(): void {
    previewEntry = null;
    void tick().then(() => { restoreFocus(previewTrigger); previewTrigger = null; });
  }
  function entryAction(action: 'download' | 'rename' | 'move' | 'delete', entry: Entry, trigger: HTMLElement): void {
    if (bulkBusy || (bulkNeedsRefresh && action !== 'download')) return;
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
    await navigate('trash');
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
    notice = uploadReplacement ? 'Replacement complete. The previous file is in trash. Rename the current file before restoring the previous one.' : '';
    try { if (!await refreshRoot()) return; } catch { /* Keep the confirmed commit outcome visible. */ }
    if (generation !== sessionGeneration) return;
    await refresh();
  }

  function chooseUploads(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = '';
    if (!files.length || !canUploadDrop()) return;
    void runUploadBatch(async () => filesToUpload(files), currentFolder);
  }

  function dismissReplacement(): void {
    replacementPrompt = null;
    const decide = replacementDecision; replacementDecision = null;
    decide?.(null);
  }

  async function confirmReplacement(): Promise<void> {
    const prompt = replacementPrompt;
    if (!prompt || transferBusy || unresolvedUploadId) return;
    replacementPrompt = null;
    if (replacementDecision) {
      const decide = replacementDecision; replacementDecision = null;
      decide(prompt); return;
    }
    await startUpload(prompt.file, null, { entry_id: prompt.target.id, etag: prompt.target.etag }, prompt.folder);
  }

  function resetTransferActivity(): void {
    if (transferPhase !== 'complete') { transferPhase = ''; progress = 0; }
  }

  async function startUpload(file: File, input: HTMLInputElement | null, replacement?: Replacement, folder = currentFolder): Promise<boolean> {
    const generation = sessionGeneration;
    const activeTransfers = transfers;
    let committed = false;
    transferBusy = true;
    uploadReplacement = !!replacement;
    progress = 0;
    transferPhase = 'uploading';
    transferName = file.name;
    const uploadController = new AbortController();
    controller = uploadController;
    try {
      if (!await refreshRoot() || (uploadBatchBusy && uploadBatchCanceled)) return committed;
      const targetFolder = folder;
      const admission = await activeTransfers.admit(targetFolder.id, file.name, file.size, targetFolder.etag, (id) => { if (generation === sessionGeneration) uploadId = id; }, replacement);
      if (generation !== sessionGeneration) return committed;
      uploadId = admission.id;
      progress = 0;
      uploadCancelable = true;
      if (uploadBatchBusy && uploadBatchCanceled) throw new DOMException('Upload canceled', 'AbortError');
      await activeTransfers.send(admission.id, file, uploadController.signal, (sent, total) => { if (generation === sessionGeneration) progress = total ? Math.floor((sent / total) * 100) : 0; });
      if (generation !== sessionGeneration) return committed;
      if (uploadBatchBusy && uploadBatchCanceled) throw new DOMException('Upload canceled', 'AbortError');
      uploadCancelable = false;
      progress = 100;
      transferPhase = 'saving';
      try { await activeTransfers.commit(admission.id); }
      catch (error) {
        if (generation !== sessionGeneration) return committed;
        if (replacement && error instanceof HttpError && (error.status === 409 || error.status === 412)) {
          try {
            await activeTransfers.cancel(admission.id);
            if (generation !== sessionGeneration) return committed;
            notice = 'Replacement was not saved because the file or folder changed. This upload did not replace the existing file. Refresh and try again.';
            return committed;
          } catch { /* Check durable status when cancellation cannot be confirmed. */ }
        }
        const outcome = await uploadOutcome(admission.id);
        if (generation !== sessionGeneration) return committed;
        if (outcome === 'rejected') {
          notice = 'Upload failed. No saved file is implied.';
          return committed;
        }
        if (outcome !== 'committed') {
          unresolvedUploadId = admission.id;
          unresolvedUploadName = file.name;
          notice = 'Upload outcome could not be confirmed. Check your files before retrying.';
          return committed;
        }
      }
      if (generation !== sessionGeneration) return committed;
      committed = true;
      await showCommittedUpload();
    } catch (error) {
      if (generation !== sessionGeneration) return committed;
      if (error instanceof HttpError && error.status === 409 && error.details?.code === 'name_collision') {
        if (uploadId) activeTransfers.forget(uploadId);
        uploadId = '';
        const target = error.details.existing;
        if (uploadBatchBusy && uploadBatchCanceled) notice = 'Upload canceled before sending bytes.';
        else if (target?.kind === 'file') {
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
          if (generation !== sessionGeneration) return committed;
          notice = uploadReplacement ? 'Replacement canceled. This upload did not replace the existing file.' : 'Upload canceled. No saved file is implied.';
        } catch {
          if (generation !== sessionGeneration) return committed;
          const outcome = uploadId ? await uploadOutcome(uploadId) : 'unknown';
          if (generation !== sessionGeneration) return committed;
          if (outcome === 'committed') {
            committed = true;
            await showCommittedUpload();
          } else if (outcome === 'canceled') notice = uploadReplacement ? 'Replacement canceled. This upload did not replace the existing file.' : 'Upload canceled. No saved file is implied.';
          else { unresolvedUploadId = uploadId; unresolvedUploadName = file.name; unresolvedCancel = true; notice = 'Upload outcome could not be confirmed. Check your files before retrying.'; }
        }
      } else if (uploadId) {
        const outcome = await uploadOutcome(uploadId);
        if (generation !== sessionGeneration) return committed;
        if (outcome === 'committed') { committed = true; await showCommittedUpload(); }
        else if (outcome === 'canceled' || outcome === 'rejected') notice = uploadReplacement ? 'Replacement failed. This upload did not replace the existing file.' : 'Upload failed. No saved file was created.';
        else { unresolvedUploadId = uploadId; unresolvedUploadName = file.name; notice = 'Upload outcome could not be confirmed. Check status before retrying.'; }
      } else notice = 'Upload failed before it was admitted. No saved file is implied.';
    } finally {
      if (generation !== sessionGeneration) return committed;
      uploadCancelable = false;
      transferBusy = false;
      resetTransferActivity();
      controller = null;
      if (!unresolvedUploadId) uploadId = '';
      if (input) input.value = '';
    }
    return committed;
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
              resolveUploadResult('failed');
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
      resolveUploadResult('saved');
      uploadId = unresolvedUploadId;
      unresolvedUploadId = '';
      unresolvedCancel = false;
      await showCommittedUpload();
      if (generation !== sessionGeneration) return;
      if (!uploadReplacement) notice = '';
    } else if (outcome === 'canceled' || outcome === 'rejected') {
      resolveUploadResult(outcome === 'canceled' ? 'canceled' : 'failed');
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
      resolveUploadResult('canceled');
      unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false;
      notice = 'Upload canceled. No saved file was created.';
    } catch {
      if (generation !== sessionGeneration) return;
      const outcome = await uploadOutcome(unresolvedUploadId);
      if (generation !== sessionGeneration) return;
      if (outcome === 'canceled' || outcome === 'rejected') {
        resolveUploadResult(outcome === 'canceled' ? 'canceled' : 'failed');
        unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false;
        notice = outcome === 'canceled' ? 'Upload canceled. No saved file was created.' : 'Upload was rejected. No saved file was created.';
      } else if (outcome === 'committed') {
        resolveUploadResult('saved');
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
    if (bulkBusy) return;
    closeBulk();
    hideUploadReport();
    clearSearch();
    const index = folderPath.findIndex((item) => item.id === folder.id);
    folderPath = index >= 0 ? folderPath.slice(0, index + 1) : [...folderPath, folder];
    const generation = sessionGeneration;
    currentFolder = folder; accountView = false; settingsView = false; showTrash = false; recordNavigation(); await refresh();
    if (generation !== sessionGeneration) return;
    await tick(); document.getElementById('folder-title')?.focus();
  }
</script>

<svelte:window ondragover={externalDrag} ondrop={externalDrag} />
<svelte:head>
  <title>{instanceName} · Private files</title>
  <meta name="description" content="Private file storage" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</svelte:head>

{#snippet betaNotice()}
  <footer class="beta-notice">Beta · Durability not qualified. Keep independent copies.</footer>
{/snippet}

<div class="app-shell">
  <a class="skip-link" href="#main-content">Skip to content</a>
  {#if !user || loggingOut}
  <header class="app-header">
    <a class="brand" href="#files" onclick={home}><FolderOpenIcon /><span>{instanceName}</span></a>
    <div class="header-actions">
      <HeaderIconButton label={darkMode ? 'Switch to light mode' : 'Switch to dark mode'} pressed={darkMode} onclick={toggleTheme}>{#if darkMode}<SunIcon />{:else}<MoonIcon />{/if}</HeaderIconButton>
      {#if user && !loggingOut}<HeaderIconButton label="Log out" disabled={loggingOut} onclick={signOut}><LogOutIcon /></HeaderIconButton>{/if}
    </div>
  </header>
  {/if}
  {#if loggingOut}
    <main id="main-content" class="signin-main"><p role="status">Signing out…</p></main>
  {:else if !user}
    <main id="main-content" class="signin-main">
      <section class="signin-form" aria-labelledby="auth-heading">
        <h1 id="auth-heading">{view === 'setup' ? 'Set up ' + instanceName : 'Sign in to ' + instanceName}</h1>
        <p class="signin-intro">{view === 'setup' ? 'Create the first administrator account.' : 'Private file storage.'}</p>
        <form class="grid gap-5" onsubmit={signIn}>
          {#if view === 'setup'}<div class="grid gap-2"><Label for="setup-auth">One-use setup authorization</Label><p id="setup-help" class="text-muted-foreground text-sm">Read <code class="break-all">secrets/setup.authorization</code> in the initialized state directory, or ask your server operator.</p><Input aria-describedby="setup-help" id="setup-auth" type="password" bind:value={setupAuthorization} autocomplete="off" required /></div>{/if}
          <div class="grid gap-2"><Label for="handle">Account name</Label><Input id="handle" bind:value={handle} autocomplete="username" pattern={view === 'setup' ? '[a-z0-9][a-z0-9._-]*' : undefined} maxlength={64} required />{#if view === 'setup'}<p class="text-muted-foreground text-xs">Lowercase letters, numbers, dots, underscores or hyphens. Start with a letter or number.</p>{/if}</div>
          <div class="grid gap-2"><Label for="password">Password</Label><Input id="password" type="password" bind:value={password} autocomplete={view === 'setup' ? 'new-password' : 'current-password'} minlength={view === 'setup' ? 15 : undefined} maxlength={view === 'setup' ? 128 : undefined} required />{#if view === 'setup'}<p class="text-muted-foreground text-xs">Use 15 to 128 characters.</p>{/if}</div>
          <Button type="submit" disabled={authBusy}>{authBusy ? (view === 'setup' ? 'Setting up…' : 'Signing in…') : (view === 'setup' ? 'Create administrator' : 'Sign in')}</Button>
        </form>
        {#if notice}<p class="form-notice" role="status">{notice}</p>{/if}
        <Button class="setup-link" type="button" variant="link" disabled={authBusy} onclick={() => { view = view === 'setup' ? 'login' : 'setup'; notice = ''; }}>{view === 'setup' ? 'Back to sign in' : 'First-time setup'}</Button>
      </section>
    </main>
  {:else}
    <Sidebar.Provider>
      <AppSidebar {instanceName} onhome={home}>
        {#snippet navigation(close)}
          <nav aria-label="Files"><Sidebar.Menu>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="Files" aria-label="Files" isActive={!showTrash && !accountView && !settingsView} aria-current={!showTrash && !accountView && !settingsView ? 'page' : undefined} data-drop-target={folders[0]?.id} data-drop-active={dropTarget === folders[0]?.id} ondragover={(event) => folders[0] && dragOver(event, folders[0])} ondragleave={(event) => folders[0] && dragLeave(event, folders[0])} ondrop={(event) => folders[0] && dropEntry(event, folders[0])} onclick={async () => { if (folders[0]) await openFolder(folders[0]); await navigate('files'); close(); }}><FolderIcon /><span>Files</span></Sidebar.MenuButton></Sidebar.MenuItem>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="Trash" aria-label="Trash" isActive={showTrash} aria-current={showTrash ? 'page' : undefined} onclick={async () => { await openTrash(); close(); }}><TrashIcon /><span>Trash</span></Sidebar.MenuButton></Sidebar.MenuItem>

          </Sidebar.Menu></nav>
          {#if user?.role === 'administrator'}
            <nav aria-label="Administration" class="mt-1"><Sidebar.Menu><Sidebar.MenuItem>
              <Sidebar.MenuButton tooltipContent="Settings" aria-label="Settings" isActive={settingsView} aria-current={settingsView ? 'page' : undefined} onclick={async () => { await navigate('settings'); close(); }}><SettingsIcon /><span>Settings</span></Sidebar.MenuButton>
            </Sidebar.MenuItem></Sidebar.Menu></nav>
          {/if}
        {/snippet}
        {#snippet account(close)}
          <nav aria-label="Account"><Sidebar.Menu><Sidebar.MenuItem>
            <Sidebar.MenuButton size="lg" tooltipContent="Account" aria-label="Account" isActive={accountView} aria-current={accountView ? 'page' : undefined} onclick={async () => { await navigate('account'); close(); }}>
              <UserRoundIcon /><span class="sidebar-account"><span>{user?.handle}</span><span class="sidebar-account-role">{user?.role === 'administrator' ? 'Administrator' : 'User'}</span></span>
            </Sidebar.MenuButton>
          </Sidebar.MenuItem></Sidebar.Menu></nav>
        {/snippet}
      </AppSidebar>
      <Sidebar.Inset class="min-w-0">
        <header class="app-header workspace-header">
          <div class="workspace-location"><Sidebar.Trigger /><span class="header-divider"></span><span>{settingsView ? 'Settings' : accountView ? 'Account' : showTrash ? 'Trash' : 'Files'}</span></div>
          <div class="header-actions">
            <HeaderIconButton label={darkMode ? 'Switch to light mode' : 'Switch to dark mode'} pressed={darkMode} onclick={toggleTheme}>{#if darkMode}<SunIcon />{:else}<MoonIcon />{/if}</HeaderIconButton>
            <HeaderIconButton label="Log out" disabled={loggingOut} onclick={signOut}><LogOutIcon /></HeaderIconButton>
          </div>
        </header>
        <div id="main-content" class="workspace-main">
        {#if accountView}
          <section class="account-content">
            <h1 id="account-title" tabindex="-1">Account</h1>
            <section class="settings-section"><h2>Your account</h2><dl class="account-details"><dt>Account name</dt><dd>{user.handle}</dd><dt>Role</dt><dd>{user.role === 'administrator' ? 'Administrator' : 'User'}</dd></dl></section>
            <section class="settings-section account-setting-row"><div><h2>Password</h2><p class="section-description">Change the password you use to sign in.</p></div><Button variant="outline" onclick={(event) => { passwordDialogTrigger = event.currentTarget; accountNotice = ''; currentPassword = ''; newPassword = ''; changePasswordOpen = true; }}>Change password</Button></section>
            <section class="settings-section account-setting-row"><div><h2>Session</h2><p class="section-description">Sign out of this browser.</p></div><Button variant="outline" disabled={loggingOut} onclick={signOut}>Sign out</Button></section>
            {#if accountNotice && !changePasswordOpen}<p role="status" class="form-notice">{accountNotice}</p>{/if}
          </section>
        {:else if settingsView && user.role === 'administrator'}
          <section class="settings-content">
            <h1 id="settings-title" tabindex="-1">Settings</h1>
            <nav class="settings-tabs" aria-label="Settings"><Button variant="ghost" class={settingsTab === 'accounts' ? 'active' : ''} aria-current={settingsTab === 'accounts' ? 'page' : undefined} onclick={() => settingsTab = 'accounts'}>Accounts</Button><Button variant="ghost" class={settingsTab === 'instance' ? 'active' : ''} aria-current={settingsTab === 'instance' ? 'page' : undefined} onclick={() => settingsTab = 'instance'}>Instance</Button></nav>
            {#if settingsTab === 'accounts'}
              <section class="settings-section"><div class="section-heading"><div><h2 id="accounts-heading" tabindex="-1">Accounts</h2><p class="section-description">Each account has private files.</p></div><div class="flex flex-wrap gap-2"><Button variant="outline" size="sm" disabled={accountsLoading || accountBusy} onclick={loadAccounts}>Refresh</Button><Button size="sm" disabled={accountBusy} onclick={(event) => { accountDialogTrigger = event.currentTarget; accountNotice = ''; createAccountOpen = true; }}>Create account</Button></div></div>
                {#if accountsLoading}<p role="status">Loading accounts…</p>{:else if accountsError}<p role="alert">{accountsError}</p>{:else if accounts.length === 0}<p>No accounts to show.</p>{/if}
                <div class="account-list">{#each accounts as account (account.account_id)}<div class="account-row"><div class="account-identity"><strong>{account.handle}</strong><span class="account-role">{account.role === 'administrator' ? 'Administrator' : 'User'}</span></div><span class:enabled={account.enabled} class="status-label">{account.enabled ? 'Active' : 'Disabled'}</span><div class="account-actions">{#if account.role === 'user' && account.enabled}<Button variant="ghost" size="sm" disabled={accountBusy} onclick={(event) => { accountDialogTrigger = event.currentTarget; accountNotice = ''; resetId = account.account_id; resetPassword = ''; }}>Reset password</Button><Button variant="ghost" size="sm" disabled={accountBusy} onclick={(event) => { accountDialogTrigger = event.currentTarget; accountNotice = ''; disableTarget = account; }}>Disable</Button>{/if}</div></div>{/each}</div>
              </section>
              {#if accountNotice && !createAccountOpen && !resetId && !disableTarget}<p role="status" class="form-notice">{accountNotice}</p>{/if}
            {:else}
              <section class="settings-section"><h2>Instance</h2><p class="section-description">The instance name appears for everyone, including on sign in.</p><form class="settings-form grid gap-3" onsubmit={saveInstance}><Label for="instance-name">Instance name</Label><Input id="instance-name" bind:value={instanceDraft} maxlength={40} disabled={instanceBusy || instanceLoading} required /><Button type="submit" class="justify-self-start mt-2" disabled={instanceBusy || instanceLoading || !instanceDraft.trim()}>{instanceBusy ? 'Saving…' : 'Save changes'}</Button></form>{#if instanceError}<p class="form-notice" role="alert">{instanceError}</p><Button variant="link" disabled={instanceLoading} onclick={loadInstance}>Reload instance name</Button>{/if}{#if instanceNotice}<p class="form-notice" role="status">{instanceNotice}</p>{/if}</section>
            {/if}
          </section>
        {:else if showTrash}
            <section class="trash-content grid gap-4" aria-labelledby="trash-heading" aria-busy={trashLoading}><h1 id="trash-heading" tabindex="-1">Trash</h1><p class="text-sm">Restore a parent folder before its children. Permanent removal is not available yet.</p><Button variant="outline" size="sm" disabled={trashLoading || trashBusy} onclick={loadTrash}>Refresh trash</Button>{#if trashError}<p role="alert">{trashError}</p>{/if}{#if trashLoading}<p role="status">Loading trash…</p>{:else if !trashError && deletedItems.length === 0}<p>Trash is empty.</p>{/if}{#if deletedItems.length > 0}<ul>{#each deletedItems as item (item.id)}<li class="flex flex-wrap items-center justify-between gap-2 border-t py-2"><span class="break-all">{item.name} ({item.kind})<br /><span class="text-muted-foreground text-sm">Original folder: Files{item.original_path.length ? ` / ${item.original_path.join(' / ')}` : ''}{#if needsDeletedReference(item)}<br />Item reference: {item.id}{/if}</span></span><Button variant="outline" size="sm" disabled={trashBusy} aria-label={deletedItems.some((other) => other.id !== item.id && other.name === item.name) ? `Restore ${item.name} from Files / ${item.original_path.join(' / ')}${needsDeletedReference(item) ? `, item reference ${item.id}` : ''}` : `Restore ${item.name}`} onclick={() => restoreEntry(item)}>Restore</Button></li>{/each}</ul>{/if}</section>

        {:else}
          <section class="files-content" aria-labelledby="folder-title">
            {#if unresolvedUploadId}<Alert.Root variant="destructive"><Alert.Title>Upload status needs checking</Alert.Title><Alert.Description><p>{unresolvedUploadName} may have been saved. Check its status before uploading it again.</p><div class="mt-2 flex flex-wrap gap-2"><Button variant="outline" size="sm" onclick={reconcileUpload} disabled={transferBusy}>Check upload status</Button>{#if unresolvedCancel}<Button variant="outline" size="sm" onclick={retryCancel} disabled={transferBusy}>Retry cancellation</Button>{/if}</div></Alert.Description></Alert.Root>{/if}

            <div class="page-heading">
              {#if folderPath.length > 1}<nav class="breadcrumb" aria-label="Folder path">{#each folderPath.slice(0, -1) as folder (folder.id)}<Button variant="link" class="h-auto p-0 break-all whitespace-normal" onclick={() => openFolder(folder)}>{folder.name}</Button><span aria-hidden="true">/</span>{/each}</nav>{/if}
              <h1 id="folder-title" tabindex="-1">{currentFolder.name}</h1>
            </div>
            <div class="search-toolbar">
              <Label for="file-search">Search files and folders</Label>
              <div class="search-controls"><Input id="file-search" disabled={bulkBusy} type="search" placeholder="Search by name" maxlength={255} bind:value={searchInput} oninput={(event) => { searchInput = event.currentTarget.value; searchChanged(); }} />
                {#if searchQuery}<Button variant="outline" type="button" onclick={clearSearchFromButton}>Clear search</Button>{/if}
              </div>
            </div>
            {#if searchQuery}
              <section aria-label="Search results" aria-busy={searchLoading} class="search-results">
                {#if searchLoading}<p role="status">Searching…</p>
                {:else if searchInputError}<p role="alert">{searchInputError}</p>
                {:else if searchError}<Alert.Root variant="destructive" role="alert"><Alert.Title>Search failed</Alert.Title><Alert.Description><p>{searchError}</p><Button variant="outline" size="sm" class="mt-2" onclick={() => runSearch(searchRetryCursor, searchRetryPage)}>Try again</Button></Alert.Description></Alert.Root>
                {:else if searchItems.length === 0}<p>No matching files or folders.</p>
                {:else}<div class="file-list"><div class="list-head"><span>Name and location</span><span class="file-size">Size</span><span class="sr-only">Options</span></div>
                  {#each searchItems as hit (hit.entry.id)}
                    <article aria-label={hit.entry.name} class="file-row search-row">
                      <div class="search-item-main"><button type="button" class="item-name" title={hit.entry.name} aria-label={`${hit.entry.kind === 'folder' ? 'Open' : 'Preview'} ${hit.entry.name}`} onclick={(event) => hit.entry.kind === 'folder' ? openSearchFolder(hit) : openPreview(hit.entry, event.currentTarget)}><span class:folder-icon={hit.entry.kind === 'folder'} aria-hidden="true">{#if hit.entry.kind === 'folder'}<FolderIcon />{:else}<FileIcon />{/if}</span><span class="filename">{hit.entry.name}</span></button>
                        <div class="search-location"><span>Files{#each hit.location.slice(1) as folder (folder.id)}{' / '}{folder.name}{/each}</span><Button variant="link" size="sm" onclick={() => openSearchFolder(hit, true)} aria-label={`Open containing folder for ${hit.entry.name}`}>Open folder</Button></div>
                      </div><span class="file-size">{hit.entry.kind === 'folder' ? '—' : typeof hit.entry.size === 'number' ? formatSize(hit.entry.size) : '—'}</span>
                      <EntryOptions entry={hit.entry} disabled={!!dropPending || moveBusy} moveDisabled={moveNeedsRefresh} onaction={(action, trigger) => entryAction(action, hit.entry, trigger)} />
                    </article>
                  {/each}
                </div>{/if}
                {#if searchPageIndex > 0 || searchHasMore}<div class="flex flex-wrap gap-2 mt-3">{#if searchPageIndex > 0}<Button variant="outline" size="sm" onclick={searchPrevious} disabled={searchLoading}>Previous page</Button>{/if}{#if searchHasMore}<Button variant="outline" size="sm" onclick={searchNext} disabled={searchLoading}>Next page</Button>{/if}</div>{/if}
                {#if !searchLoading && !searchError && !searchInputError}<p class="list-summary">{searchItems.length} {searchItems.length === 1 ? 'result' : 'results'}{searchHasMore || searchPageIndex > 0 ? ' on this page' : ''}</p>{/if}
              </section>
            {:else}
            <section class="upload-toolbar" aria-label="Upload files">
              <Button variant="outline" disabled={fileMutationBusy || !currentFolder.id} onclick={(event) => beginCreate(event.currentTarget as HTMLElement)}><PlusIcon />New folder</Button>
              <div class="upload-form">
                <input class="sr-only" id="upload-file" type="file" multiple tabindex={-1} aria-label="Choose files to upload" disabled={fileMutationBusy || !!unresolvedUploadId || !!replacementPrompt} onchange={chooseUploads} />
                <Button id="choose-file" variant="outline" type="button" disabled={fileMutationBusy || !!unresolvedUploadId || !!replacementPrompt} onclick={() => document.getElementById('upload-file')?.click()}>Choose files</Button>
              </div>
              <Button variant="outline" disabled={loading || bulkBusy} onclick={() => bulkNeedsRefresh ? reconcileBulk() : refresh()} aria-label="Refresh folder" aria-busy={loading}><RefreshCwIcon class={loading ? 'refreshing' : ''} data-refreshing={loading} /><span>Refresh</span></Button>
              <div class="folder-sort">
                <label for="folder-sort">Sort files by</label>
                <select id="folder-sort" value={folderSort} disabled={bulkBusy || bulkNeedsRefresh} onchange={changeFolderSort}>
                  <option value="name_asc">Name: A to Z</option>
                  <option value="name_desc">Name: Z to A</option>
                  <option value="size_asc">Size: smallest first</option>
                  <option value="size_desc">Size: largest first</option>
                </select>
              </div>

            </section>
            {#if (uploadBatchBusy || uploadReportVisible) && (uploadBatchStatus || uploadResults.length)}
              <section class="upload-batch-report" aria-label="Upload results">
                {#if uploadBatchStatus && !uploadBatchBusy && !uploadComplete}<p class="upload-batch-status" role="status">{uploadBatchStatus}</p>{/if}
                {#if uploadBatchBusy}
                  <div class="upload-pending">
                    <span role="status" aria-label="Uploading"><LoaderCircleIcon class="refreshing" aria-hidden="true" /><span class="sr-only">{transferName ? `Uploading ${transferName}. ${progress}%.` : 'Reading files and folders.'}</span></span>
                    <Button variant="ghost" size="icon" aria-label="Cancel upload" title="Cancel upload" onclick={cancelUploadBatch}><XIcon /></Button>
                  </div>
                {/if}
                {#if !uploadBatchBusy && uploadReportVisible && uploadResults.length}
                  <details open={uploadResultsExpanded} ontoggle={(event) => uploadResultsExpanded = event.currentTarget.open}>
                    <summary><span class="upload-results-summary">{#if uploadComplete}<CheckIcon aria-hidden="true" /><span class="sr-only" role="status">Upload complete.{' '}</span>{/if}<span>Results for {uploadResults.length} {uploadResults.length === 1 ? 'item' : 'items'}</span></span></summary>
                    <ul>{#each uploadResults as result, index (index)}<li data-upload-result={result.path}><span class="upload-result-path">{result.path}</span><span>{uploadResultLabel(result)}</span></li>{/each}</ul>
                  </details>
                {/if}
              </section>
            {/if}
            {#if selectedEntries.length && !loading && !listingError}
              <div class="selection-toolbar" role="group" aria-label="Selected items">
                <span aria-live="polite">{selectedEntries.length} selected</span>
                <Button variant="ghost" size="sm" disabled={fileMutationBusy} onclick={() => selectedIds = entries.map((entry) => entry.id)}>Select all loaded items</Button>
                <Button variant="ghost" size="sm" disabled={!selectedIds.length || bulkBusy} onclick={clearSelectionFromToolbar}>Clear selection</Button>
                {#if selectedEntries.length}<Button variant="outline" size="sm" disabled={fileMutationBusy} onclick={(event) => beginBulk('move', event.currentTarget)}>Move selected</Button><Button variant="outline" size="sm" disabled={fileMutationBusy} onclick={(event) => beginBulk('trash', event.currentTarget)}>Move selected to trash</Button>{/if}
              </div>
            {/if}
            <section class="file-drop-region" aria-label="Files in current folder" ondragover={(event) => dragOver(event, currentFolder)} ondragleave={(event) => dragLeave(event, currentFolder)} ondrop={(event) => dropEntry(event, currentFolder)} data-drop-target="current-folder" data-drop-active={dropTarget === currentFolder.id}>
            {#if entries.length && !listingError}<div class="file-list"><div class="list-head selectable-head"><span aria-hidden="true"></span><span>Name</span><span class="file-size">Size</span><span class="sr-only">Options</span></div></div>{/if}
            {#if loading}<div class="flex flex-col items-center gap-1 rounded-lg border border-dashed px-4 py-10 text-center" role="status"><strong class="font-medium">Loading files…</strong></div>
            {:else if listingError}<Alert.Root variant="destructive" role="alert"><Alert.Title>Could not open this folder</Alert.Title><Alert.Description><p>{listingError}</p><Button variant="outline" size="sm" class="mt-2" onclick={refresh}>Try again</Button></Alert.Description></Alert.Root>
            {:else if entries.length === 0}<div class="flex flex-col items-center gap-1 rounded-lg border border-dashed px-4 py-10 text-center"><strong class="font-medium">This folder is empty</strong><p class="text-muted-foreground text-sm">Upload a file or create a folder.</p></div>
            {:else}<div class="file-list">{#each entries as entry (entry.id)}
              <article aria-label={entry.name} draggable={!fileMutationBusy && !moveNeedsRefresh} ondragstart={(event) => startDrag(event, entry)} ondragend={endDrag} ondragover={(event) => entry.kind === 'folder' && dragOver(event, entry)} ondragleave={(event) => entry.kind === 'folder' && dragLeave(event, entry)} ondrop={(event) => entry.kind === 'folder' && dropEntry(event, entry)} data-drop-target={entry.kind === 'folder' ? entry.id : undefined} data-drop-active={dropTarget === entry.id} class={`file-row selectable-row ${selectedIds.includes(entry.id) ? 'selected-row' : ''} ${dropTarget === entry.id ? 'drop-active' : ''}`}>
                <label class="entry-selection"><input type="checkbox" aria-label={`Select ${entry.name}`} checked={selectedIds.includes(entry.id)} disabled={fileMutationBusy} onchange={(event) => selectEntry(entry.id, event.currentTarget.checked)} /></label>
                <button type="button" class="item-name" title={entry.name} aria-label={`${entry.kind === 'folder' ? 'Open' : 'Preview'} ${entry.name}`} disabled={bulkBusy} onclick={(event) => entry.kind === 'folder' ? openFolder(entry) : openPreview(entry, event.currentTarget)}><span class:folder-icon={entry.kind === 'folder'} aria-hidden="true">{#if entry.kind === 'folder'}<FolderIcon />{:else}<FileIcon />{/if}</span><span class="item-label"><span class="filename">{entry.name}</span><span class="mobile-file-size" aria-hidden="true">{entry.kind === 'file' ? formatSize(entry.size) : ''}</span></span></button><span class="file-size">{entry.kind === 'folder' ? '—' : formatSize(entry.size)}</span>
                <EntryOptions {entry} disabled={bulkBusy || bulkNeedsRefresh || !!dropPending || moveBusy} moveDisabled={moveNeedsRefresh} onaction={(action, trigger) => entryAction(action, entry, trigger)} />
              </article>
            {/each}</div>{/if}
            </section>
            {#if dropPending}<p role="status">Moving {dropPending.entry.name} to {dropPending.destination.name}…</p>{/if}
            {#if dropError}<p role="alert">{dropError}</p>{/if}
            {#if pageIndex > 0 || hasMore}<div class="flex flex-wrap gap-2">{#if pageIndex > 0}<Button variant="outline" size="sm" onclick={loadPrevious} disabled={loading || bulkBusy}>Previous page</Button>{/if}{#if hasMore}<Button variant="outline" size="sm" onclick={loadMore} disabled={loading || bulkBusy}>Next page</Button>{/if}</div>{/if}
            <p class="list-summary">{loading ? 'Loading…' : `${entries.length} ${entries.length === 1 ? 'item' : 'items'}${hasMore || pageIndex > 0 ? ' on this page' : ''}`}</p>
            {/if}
            {#if bulkReport}
              <section class="bulk-results" aria-labelledby="bulk-results-heading" aria-busy={bulkBusy}>
                <h2 id="bulk-results-heading">{bulkReport.action === 'move' ? 'Move results' : 'Trash results'}</h2>
                <p role="status">{bulkReport.results.filter((result) => result.status === 'succeeded').length} of {bulkReport.results.length} succeeded{bulkBusy ? '. Working…' : '.'}</p>
                <ul>{#each bulkReport.results as result (result.entry.id)}<li><strong>{result.entry.name}</strong><span>{result.message}</span></li>{/each}</ul>
                {#if bulkError}<p role="alert">{bulkError}</p>{/if}
                <div class="flex flex-wrap gap-2">
                  {#if bulkNeedsRefresh}<Button variant="outline" disabled={bulkBusy} onclick={reconcileBulk}>Refresh before retry</Button>{/if}
                  {#if bulkReport.results.some((result) => result.status === 'failed')}<Button variant="outline" disabled={bulkBusy || bulkNeedsRefresh || fileMutationBusy || loading} onclick={(event) => retryBulk(event.currentTarget)}>Retry failed items</Button>{/if}
                  <Button variant="ghost" disabled={bulkBusy} onclick={() => { const uncertain = bulkNeedsRefresh; bulkReport = null; bulkError = ''; bulkNeedsRefresh = false; if (uncertain) { entries = []; moveNeedsRefresh = true; void refresh(); } }}>Dismiss results</Button>
                </div>
              </section>
            {/if}
            {#if notice}<Alert.Root role="status" class="workspace-notice"><Alert.Description>{notice}</Alert.Description></Alert.Root>{/if}
          </section>
        {/if}
        </div>
        {@render betaNotice()}
      </Sidebar.Inset>
    </Sidebar.Provider>
    <FilePreview entry={previewEntry} url={previewEntry ? listing.downloadUrl(previewEntry.id) : ''} {client} onclose={closePreview} ondownload={() => { if (previewEntry) void download(previewEntry); }} />
    <Dialog.Root open={createOpen} onOpenChange={(open) => { if (!folderBusy) createOpen = open; }}>
      <Dialog.Content showCloseButton={!folderBusy} onOpenAutoFocus={(event) => { event.preventDefault(); folderInput?.focus(); }} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(createTrigger); createTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Create folder</Dialog.Title><Dialog.Description>Add a folder to {currentFolder.name}.</Dialog.Description></Dialog.Header>
        <form class="grid gap-6" aria-busy={folderBusy} onsubmit={createFolder}><div class="grid gap-2"><Label for="new-folder">Folder name</Label><Input id="new-folder" bind:ref={folderInput} bind:value={folderName} disabled={folderBusy} maxlength={255} aria-invalid={!!folderError} aria-describedby={folderError ? 'folder-error' : undefined} required /></div>{#if folderError}<p id="folder-error" role="alert">{folderError}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={folderBusy} onclick={() => createOpen = false}>Cancel</Button><Button type="submit" disabled={folderBusy}>{folderBusy ? 'Creating…' : 'Create folder'}</Button></Dialog.Footer></form>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!moveTarget || bulkAction === 'move'} onOpenChange={(open) => { if (!open && !moveBusy && !bulkBusy) closeMove(); }}>
      <Dialog.Content showCloseButton={!bulkBusy} class="max-h-[85dvh] overflow-y-auto" onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(bulkTrigger ?? moveTrigger); moveTrigger = null; bulkTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Move {bulkAction === 'move' ? `${bulkTargets.length} selected items` : moveTarget?.name}</Dialog.Title><Dialog.Description>Current folder: {folderPath.map((folder) => folder.name).join(' / ')}. Choose a destination in your private storage.{bulkAction === 'move' ? ' Each item is handled separately; some items may stay in place.' : ''}</Dialog.Description></Dialog.Header>
        <nav aria-label="Move destination path" class="flex flex-wrap gap-2">{#each movePath as folder, index (folder.id)}<Button variant="link" class="h-auto p-0 break-all whitespace-normal" disabled={moveLoading || moveBusy || bulkBusy} aria-current={index === movePath.length - 1 ? 'page' : undefined} onclick={() => loadMoveFolder(movePath.slice(0, index + 1))}>{folder.name}</Button>{/each}</nav>
        <div class="grid gap-2 rounded-lg border p-3" aria-label="Destination folders" aria-busy={moveLoading}>
          {#if moveLoading}<p role="status">Loading destinations...</p>{:else if moveFolders.length === 0}<p class="text-muted-foreground text-sm">No folders here.</p>{/if}
          {#each moveFolders as folder (folder.id)}<Button variant="ghost" class="h-auto min-h-9 justify-start text-left break-all whitespace-normal" disabled={moveLoading || moveBusy || bulkBusy || folder.id === moveTarget?.id || (bulkAction === 'move' && bulkDestinationBlocked(folder))} onclick={() => loadMoveFolder([...movePath, folder])}><FolderIcon class="size-4 shrink-0" />{folder.name}{folder.id === moveTarget?.id || (bulkAction === 'move' && bulkDestinationBlocked(folder)) ? ' (cannot move into itself)' : ''}</Button>{/each}
          {#if moveCursor}<Button variant="outline" disabled={moveLoading || moveBusy || bulkBusy} onclick={() => loadMoveFolder(movePath, moveCursor)}>More destination folders</Button>{/if}
        </div>
        <p class="text-sm break-all">Destination: {movePath.map((folder) => folder.name).join(' / ') || 'Choose a folder'}</p>
        {#if movePath.at(-1)?.id === moveTarget?.parent_id || (bulkAction === 'move' && bulkTargets.every((entry) => entry.parent_id === movePath.at(-1)?.id))}<p class="text-muted-foreground text-sm">{bulkAction === 'move' ? 'The selected items are already here.' : 'This item is already here.'} Open another folder or choose an ancestor above.</p>{/if}
        {#if moveError}<p role="alert">{moveError}</p>{#if moveNeedsRefresh}<Button variant="outline" onclick={async () => { closeMove(); await refresh(); }}>Close and refresh files</Button>{:else}<Button variant="outline" disabled={moveBusy || moveLoading} onclick={() => loadMoveFolder(movePath.length ? movePath : folders.slice(0, 1))}>Refresh destinations</Button>{/if}{/if}
        <Dialog.Footer><Button variant="outline" disabled={moveBusy || bulkBusy} onclick={closeMove}>Cancel</Button><Button disabled={moveBusy || bulkBusy || moveNeedsRefresh || moveLoading || !moveDestinationReady || !movePath.length || movePath.at(-1)?.id === moveTarget?.parent_id || (bulkAction === 'move' && bulkTargets.every((entry) => entry.parent_id === movePath.at(-1)?.id))} onclick={() => bulkAction === 'move' ? runBulk() : moveEntry()}>{moveBusy || bulkBusy ? 'Moving...' : 'Move here'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={bulkAction === 'trash'} onOpenChange={(open) => { if (!open) closeBulk(); }}>
      <Dialog.Content showCloseButton={!bulkBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(bulkTrigger); bulkTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Move {bulkTargets.length} selected items to trash?</Dialog.Title><Dialog.Description>You can restore these items later. Folders must be empty. Each item is handled separately; some items may stay in place.</Dialog.Description></Dialog.Header>
        <ul class="bulk-confirm-items">{#each bulkTargets as entry (entry.id)}<li>{entry.name}</li>{/each}</ul>
        <Dialog.Footer><Button variant="outline" disabled={bulkBusy} onclick={closeBulk}>Cancel</Button><Button disabled={bulkBusy} onclick={runBulk}>{bulkBusy ? 'Moving to trash…' : 'Move to trash'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!replacementPrompt} onOpenChange={(open) => { if (!open) dismissReplacement(); }}>
      <Dialog.Content onCloseAutoFocus={(event) => { event.preventDefault(); const choose = document.getElementById('choose-file') as HTMLButtonElement | null; restoreFocus(choose && !choose.disabled ? choose : null); }}>
        <Dialog.Header><Dialog.Title>Replace {replacementPrompt?.target.name}?</Dialog.Title><Dialog.Description>A file with this name already exists in {replacementPrompt?.folder.name}. The previous file will move to trash and stay there until restored. Rename the current file before restoring it. If this upload fails or is canceled before saving, the previous file stays unchanged.</Dialog.Description></Dialog.Header>
        <Dialog.Footer><Button variant="outline" onclick={dismissReplacement}>Keep existing file</Button><Button onclick={confirmReplacement}>Replace file</Button></Dialog.Footer>
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

    <Dialog.Root open={changePasswordOpen} onOpenChange={(open) => { if (!accountBusy) { changePasswordOpen = open; if (!open) { currentPassword = ''; newPassword = ''; accountNotice = ''; } } }}><Dialog.Content showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(passwordDialogTrigger, 'account-title'); }}><Dialog.Header><Dialog.Title>Change password</Dialog.Title><Dialog.Description>This signs out all your sessions. Use 15 to 128 characters.</Dialog.Description></Dialog.Header><form class="grid gap-4" onsubmit={changePassword}><div class="grid gap-2"><Label for="current-password">Current password</Label><Input id="current-password" type="password" autocomplete="current-password" bind:value={currentPassword} disabled={accountBusy} required /></div><div class="grid gap-2"><Label for="new-password">New password</Label><Input id="new-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={newPassword} disabled={accountBusy} required /></div>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={accountBusy} onclick={() => { changePasswordOpen = false; currentPassword = ''; newPassword = ''; accountNotice = ''; }}>Cancel</Button><Button type="submit" disabled={accountBusy}>{accountBusy ? 'Changing…' : 'Change password'}</Button></Dialog.Footer></form></Dialog.Content></Dialog.Root>
    <Dialog.Root open={createAccountOpen} onOpenChange={(open) => { if (!accountBusy) { createAccountOpen = open; if (!open) accountPassword = ''; } }}>
      <Dialog.Content showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}><Dialog.Header><Dialog.Title>Create account</Dialog.Title><Dialog.Description>Use 15 to 128 characters for the password. Share it through a private channel.</Dialog.Description></Dialog.Header><form class="grid gap-4" onsubmit={createAccount}><div class="grid gap-2"><Label for="account-handle">Account name</Label><Input id="account-handle" bind:value={accountHandle} pattern="[a-z0-9][a-z0-9._-]*" maxlength={64} disabled={accountBusy} required /><p class="text-muted-foreground text-xs">Lowercase letters, numbers, dots, underscores or hyphens.</p></div><div class="grid gap-2"><Label for="account-password">Password</Label><Input id="account-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={accountPassword} disabled={accountBusy} required /></div>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={accountBusy} onclick={() => { createAccountOpen = false; accountPassword = ''; }}>Cancel</Button><Button type="submit" disabled={accountBusy}>{accountBusy ? 'Creating…' : 'Create account'}</Button></Dialog.Footer></form></Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!resetId} onOpenChange={(open) => { if (!open && !accountBusy) { resetId = ''; resetPassword = ''; } }}><Dialog.Content showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}><Dialog.Header><Dialog.Title>Reset password for {accounts.find((account) => account.account_id === resetId)?.handle}</Dialog.Title><Dialog.Description>This signs out all their sessions. Share the new password through a private channel.</Dialog.Description></Dialog.Header><form class="grid gap-4" onsubmit={resetAccount}><div class="grid gap-2"><Label for="reset-password">New password</Label><Input id="reset-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={resetPassword} disabled={accountBusy} required /><p class="text-muted-foreground text-xs">Use 15 to 128 characters.</p></div>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={accountBusy} onclick={() => { resetId = ''; resetPassword = ''; }}>Cancel</Button><Button type="submit" disabled={accountBusy}>{accountBusy ? 'Resetting…' : 'Reset password'}</Button></Dialog.Footer></form></Dialog.Content></Dialog.Root>
    <Dialog.Root open={!!disableTarget} onOpenChange={(open) => { if (!open && !accountBusy) disableTarget = null; }}><Dialog.Content showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}><Dialog.Header><Dialog.Title>Disable {disableTarget?.handle}?</Dialog.Title><Dialog.Description>This prevents sign in and signs out all their sessions. Their files stay stored. Re-enabling accounts is not available yet.</Dialog.Description></Dialog.Header>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button variant="outline" disabled={accountBusy} onclick={() => disableTarget = null}>Cancel</Button><Button variant="destructive" disabled={accountBusy} onclick={() => disableTarget && disableAccount(disableTarget)}>{accountBusy ? 'Disabling…' : 'Disable account'}</Button></Dialog.Footer></Dialog.Content></Dialog.Root>
  {/if}
  {#if !user || loggingOut}{@render betaNotice()}{/if}
</div>

<style>
  .upload-batch-report { display: grid; gap: 12px; padding-block: 12px; }
  .upload-pending { display: flex; align-items: center; gap: 8px; color: var(--muted-foreground); }
  .upload-pending [role="status"] { display: flex; }
  .upload-pending :global(svg) { width: var(--icon-file); height: var(--icon-file); }
  .upload-batch-report summary { cursor: pointer; font-size: var(--type-ui); }
  .upload-batch-report li { display: flex; justify-content: space-between; gap: 16px; border-bottom: 1px solid var(--border); padding-block: 8px; font-size: var(--type-ui); }
  .upload-batch-report li > span:last-child { flex-shrink: 0; }
  .upload-batch-status, .upload-result-path { overflow-wrap: anywhere; min-width: 0; }
  .upload-results-summary { display: inline-flex; align-items: center; gap: 7px; vertical-align: middle; }
  .upload-results-summary :global(svg) { width: var(--icon-small); height: var(--icon-small); color: var(--primary); }
  .file-drop-region { border-radius: var(--radius-lg); }
  .file-drop-region[data-drop-active="true"] { outline: 2px solid var(--primary); outline-offset: -2px; }
  :global(.drop-active) { border-radius: var(--radius-lg); background: var(--accent); outline: 2px solid var(--primary); outline-offset: -2px; }
  :global(.refreshing) { animation: refresh-turn 1s linear infinite; }
  @keyframes refresh-turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { :global(.refreshing) { animation: none; } }
</style>

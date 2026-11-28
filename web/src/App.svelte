<script lang="ts">
  import './app.css';
  import * as Sidebar from '$lib/components/ui/sidebar/index.js';
  import AppSidebar from '$lib/components/app-sidebar.svelte';
  import AppearanceSettings from '$lib/AppearanceSettings.svelte';
  import LibraryHome from '$lib/LibraryHome.svelte';
  import DirectShareDialog from '$lib/DirectShareDialog.svelte';
  import SharedWorkspace from '$lib/SharedWorkspace.svelte';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import WebDavPasswords from '$lib/WebDavPasswords.svelte';
  import BrandLogo from '$lib/BrandLogo.svelte';
  import { appearanceStyle, defaultAppearanceSurfaces, defaultAppearance, readAppearance, type Appearance } from '$lib/appearance';
  import { onMount, tick, untrack } from 'svelte';
  import MoonIcon from '@lucide/svelte/icons/moon';
  import SunIcon from '@lucide/svelte/icons/sun';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import UserRoundIcon from '@lucide/svelte/icons/user-round';
  import UsersRoundIcon from '@lucide/svelte/icons/users-round';
  import LogOutIcon from '@lucide/svelte/icons/log-out';
  import EntryOptions, { type EntryAction } from './lib/EntryOptions.svelte';
  import PublicFolderNotice from './lib/PublicFolderNotice.svelte';
  import { publicFolderNotice } from './lib/public-folder-notice.svelte';
  import { publicFolderLinkStatus, type PublicLink } from './lib/public-folder-links';
  import ShareLinks from './lib/ShareLinks.svelte';
  import AccountOptions from './lib/AccountOptions.svelte';
  import AccountStatus from './lib/AccountStatus.svelte';
  import HeaderIconButton from './lib/HeaderIconButton.svelte';
  import FilePreview from './lib/FilePreview.svelte';
  import type { TextEditState } from './lib/text-edit';
  import FileIcon from '@lucide/svelte/icons/file';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import LibraryIcon from '@lucide/svelte/icons/library';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import CheckIcon from '@lucide/svelte/icons/check';
  import SortHeader from './lib/SortHeader.svelte';
  import RefreshButton from './lib/RefreshButton.svelte';
  import OidcApprovals from './lib/OidcApprovals.svelte';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import XIcon from '@lucide/svelte/icons/x';
  import { ApiClient, HttpError, HttpTransport, MutationApi } from './lib/api';
  import { sharingConfirmation, sharingTitle, type SharingConfirmation } from './lib/direct-grants';
  import { modifiedTime } from './lib/modified-time';
  import { actionTargets, listIntent, moveIndex, shortcutHelp, shortcutsBlocked } from './lib/list-keys';
  import { compareDeleted, ListingApi, type Entry, type Folder, type DeletedEntry, type SearchHit, type FolderSort, type ResultSort, type RemovalPreview, type RemovalResult } from './lib/listing';
  import { SessionApi, type User, type Account, type Usage, type AllUsage } from './lib/session';
  import { TransferApi, identifyFile, saveRefused, type Replacement } from './lib/transfers';
  import { directoryFilesToUpload, filesToUpload, snapshotDesktopDrop, type UploadItem } from './lib/desktop-drop';

  import { isSharingDestination, type SharingDestination, parseLocation, locationHash, savedContext, type BrowsingContext, type Destination, type Location, type AccountSort } from './lib/browsing-context';

  import { MockTransport } from './lib/mock-transport';

  // Dev-only: VITE_MOCK=1 renders the UI without a backend.
  const client = new ApiClient(import.meta.env.DEV && import.meta.env.VITE_MOCK ? new MockTransport() : new HttpTransport());
  const mutations = new MutationApi(client);
  const session = new SessionApi(client);
  const listing = new ListingApi(client);
  let transfers = new TransferApi(client, mutations);
  let view = $state<'login' | 'setup'>('login');
  let authBusy = $state(false);
  let oidcEnabled = $state(false);
  let oidcOpening = $state(false);
  let conversionOpen = $state(false);
  let conversionPassword = $state('');
  let conversionError = $state('');
  let conversionTrigger: HTMLElement | null = null;
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

  let folderChooserSupported = $state(false);

  onMount(() => {
    folderChooserSupported = 'webkitdirectory' in document.createElement('input');
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
    void loadAppearance();
    void restoreExternalSession();
    const previousScrollRestoration = window.history.scrollRestoration;
    window.history.scrollRestoration = 'manual';
    const onPageShow = (event: PageTransitionEvent) => {
      if (event.persisted) oidcOpening = false;
    };
    window.addEventListener('pageshow', onPageShow);
    window.addEventListener('scroll', scheduleScrollSave, { passive: true });
    window.addEventListener('pagehide', saveNavigation);
    window.addEventListener('popstate', restoreNavigation);
    return () => { clearRouteFeedback(); preference.removeEventListener('change', onChange); window.removeEventListener('popstate', restoreNavigation); window.removeEventListener('scroll', scheduleScrollSave); if (scrollSaveTimer) clearTimeout(scrollSaveTimer); window.removeEventListener('pagehide', saveNavigation); window.removeEventListener('pageshow', onPageShow); window.history.scrollRestoration = previousScrollRestoration; };
  });
  let user = $state<User | null>(null);
  let textEdit = $state<{ ownerId: string; state: TextEditState } | null>(null);
  let textEditSession = $state.raw<AbortController | null>(null);
  let accountView = $state(false);
  let settingsView = $state(false);
  let librariesView = $state(false);
  let sharedView = $state<SharingDestination | null>(null);
  let sharedLocation = $state({ folder: '', file: undefined as string | undefined });
  let libraries = $state<Folder[]>([]);
  let librariesError = $state('');
  let settingsTab = $state<'accounts' | 'instance' | 'appearance'>('accounts');
  let appearance = $state<Appearance>({ ...defaultAppearance });
  let appearanceLoadError = $state('');
  let appearanceLoading = $state(false);
  let appearanceRequest = 0;
  let appearanceProperties: string[] = [];
  $effect(() => {
    for (const property of appearanceProperties) document.documentElement.style.removeProperty(property);
    appearanceProperties = [];
    for (const declaration of appearanceStyle(appearance, darkMode, defaultAppearanceSurfaces()).split(';').filter(Boolean)) {
      const [property, value] = declaration.split(':');
      document.documentElement.style.setProperty(property, value);
      appearanceProperties.push(property);
    }
  });
  async function loadAppearance(): Promise<void> {
    const request = ++appearanceRequest;
    appearanceLoading = true; appearanceLoadError = '';
    try {
      const value = readAppearance(await client.request('/instance/appearance'));
      if (request === appearanceRequest) appearance = value;
    } catch { if (request === appearanceRequest) appearanceLoadError = 'Appearance could not be loaded.'; }
    finally { if (request === appearanceRequest) appearanceLoading = false; }
  }
  let instanceName = $state('Sylux');
  let instanceDraft = $state('Sylux');
  let instanceNameBusy = $state(false);
  let appearanceBusy = $state(false);
  let appearanceError = $state('');
  let instanceBusy = $derived(instanceNameBusy || appearanceBusy);
  let instanceLoading = $state(false);
  let instanceError = $state('');
  let instanceNotice = $state('');
  let instanceRequest = 0;
  let createAccountOpen = $state(false);
  let changePasswordOpen = $state(false);
  let passwordDialogTrigger: HTMLElement | null = null;
  let disableTarget = $state<Account | null>(null);
  let authorityTarget = $state<{ account: Account; action: 'enable' | 'role' } | null>(null);
  let accountDialogTrigger: HTMLElement | null = null;
  let historyReady = $state(false);
  let navigationRequest = 0;
  let restorationPending = $state(false);
  let restorationFailed = $state(false);
  let restorationUnavailable = $state(false);
  let deferredNavigation = $state(false);
  let deferredAccountOperation = $state(false);
  let deferredInstanceOperation = $state(false);
  let deferredShareOperation = $state(false);

  $effect(() => {
    if (deferredNavigation && !bulkBusy && !accountBusy && !instanceBusy && (!deferredShareOperation || (!share && !directShare)) &&
      (!deferredAccountOperation || (!changePasswordOpen && !createAccountOpen && !disableTarget && !authorityTarget && !resetId)) && (!deferredInstanceOperation || (!instanceError && !appearanceError))) {
      deferredNavigation = false; deferredAccountOperation = false; deferredInstanceOperation = false; deferredShareOperation = false;
      void untrack(restoreNavigation);
    }
  });
  let scrollSaveTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleScrollSave(): void {
    if (scrollSaveTimer) return;
    scrollSaveTimer = setTimeout(() => { scrollSaveTimer = null; saveNavigation(); }, 150);
  }

  function browsingSnapshot(): BrowsingContext {
    return { version: 1, owner: user?.id ?? '',
      destination: settingsView ? 'settings' : accountView ? 'account' : showTrash ? 'trash' : librariesView ? 'libraries' : sharedView ?? 'files',
      folder: sharedView ? sharedLocation.folder : librariesView ? '' : currentFolder.id,
      file: sharedView ? sharedLocation.file : undefined, query: searchQuery, sort: folderSort, searchSort, trashSort, accountSort,
      cursors: [...pageCursors], page: pageIndex, searchCursors: [...searchPageCursors], searchPage: searchPageIndex, scroll: window.scrollY };
  }

  function saveNavigation(): void {
    if (!historyReady || !user) return;
    const context = browsingSnapshot();
    window.history.replaceState({ sylux: context }, '', locationHash(context));
  }

  $effect(() => { saveNavigation(); });

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
    instanceNameBusy = true; instanceError = ''; instanceNotice = '';
    try {
      const result = await client.request<{ name: string }>('/instance', { method: 'PATCH', body: { name: instanceDraft.trim() } });
      if (generation !== sessionGeneration) return;
      if (!result || typeof result.name !== 'string' || !result.name.trim()) throw new Error('Invalid instance name response');
      instanceName = result.name; instanceDraft = result.name; instanceNotice = 'Instance name saved.';
    } catch { if (generation === sessionGeneration) instanceError = 'Could not save the instance name. Try again.'; }
    finally { if (generation === sessionGeneration) instanceNameBusy = false; }
  }

  const routeFeedbackDelay = 180;
  let pendingRoute = $state<string | null>(null);
  let routeFeedback = $state(false);
  let routeFeedbackTimer: ReturnType<typeof setTimeout> | null = null;
  let presentedRoute = $state(false);
  let routeFocus: ReturnType<typeof watchFocus> | null = null;
  let routeController: AbortController | null = null;
  let retainingRoute = $state(false);
  let searchDuringRoute = $state(false);
  const routeName = $derived(pendingRoute ? parseLocation(pendingRoute).destination : null);

  function clearRouteFeedback(): void {
    if (routeFeedbackTimer !== null) clearTimeout(routeFeedbackTimer);
    routeFeedbackTimer = null;
    routeController?.abort(); routeController = null;
    routeFocus?.stop(); routeFocus = null;
    pendingRoute = null;
    retainingRoute = false;
    searchDuringRoute = false;
    routeFeedback = false;
  }

  function showDestination(destination: Destination): void {
    accountView = destination === 'account';
    settingsView = destination === 'settings';
    showTrash = destination === 'trash';
    librariesView = destination === 'libraries';
    sharedView = isSharingDestination(destination) ? destination : null;
  }

  function navigate(destination: Destination, path?: Folder[], shared?: { folder: string; file?: string }): Promise<boolean> {
    if (!user || bulkBusy || accountBusy || instanceBusy || shareBusy || directShareBusy) return Promise.resolve(false);
    const next: Location = { ...browsingSnapshot(), destination, folder: shared?.folder ?? path?.at(-1)?.id ?? '', file: shared?.file, query: '' };
    const hash = locationHash(next);
    if (pendingRoute === hash) return Promise.resolve(false);
    const previous = parseLocation(window.location.hash);
    saveNavigation();
    if (previous.destination !== destination || ((destination === 'files' || isSharingDestination(destination)) &&
        (previous.folder !== next.folder || previous.file !== next.file || previous.query))) {
      window.history.pushState({ sylux: {
        ...next, version: 1, owner: user.id, cursors: [null], page: 0, searchCursors: [null], searchPage: 0, scroll: 0,
      } }, '', hash);
    }
    return presentRoute(next, null, path);
  }

  function navigateShared(folder = '', file?: string): void {
    void navigate(sharedView ?? 'shared', undefined, { folder, file });
  }

  async function restoreNavigation(): Promise<void> {
    if (!user) return;
    if (bulkBusy || accountBusy || instanceBusy || shareBusy || directShareBusy || (deferredShareOperation && (share || directShare))) {
      historyReady = false; deferredNavigation = true;
      deferredAccountOperation = accountBusy; deferredInstanceOperation = instanceBusy; deferredShareOperation = deferredShareOperation || shareBusy || !!directShareBusy;
      return;
    }
    const state = window.history.state?.sylux;
    const location = state?.owner && state.owner !== user.id ? parseLocation('#files') : parseLocation(window.location.hash);
    const context = savedContext(state, user.id, location);
    await presentRoute(location, context);
  }

  function staleCursor(error: unknown): boolean {
    return error instanceof HttpError && (error.status === 409 || (error.status === 400 && error.code === 'invalid_cursor'));
  }

  async function presentRoute(location: Location, context: BrowsingContext | null, path?: Folder[]): Promise<boolean> {
    if (!user) return false;
    const generation = sessionGeneration;
    const request = ++navigationRequest;
    const isCurrent = () => generation === sessionGeneration && request === navigationRequest;
    const destination = location.destination === 'settings' && user.role !== 'administrator' ? 'files' : location.destination;
    const isFiles = destination === 'files';
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const useShell = !presentedRoute || destination === 'account' || destination === 'settings';
    let scroll = context?.scroll ?? 0;
    let loaded = false;
    let readablePath: Folder[] | undefined;

    function showSlowFilesShell(): void {
      if (!readablePath) return;
      clearSearch();
      showDestination('files');
      retainingRoute = false;
      folderPath = readablePath; currentFolder = readablePath.at(-1)!;
      folderSort = location.sort; searchSort = location.searchSort ?? 'name_asc';
      entries = []; entriesFolderId = ''; listingError = '';
      pageCursors = [null]; pageIndex = 0; hasMore = false; nextCursor = null;
      searchDuringRoute = true;
    }

    historyReady = false;
    deferredNavigation = false; deferredAccountOperation = false; deferredInstanceOperation = false; deferredShareOperation = false;
    restorationPending = true; restorationFailed = false; restorationUnavailable = false;
    clearRouteFeedback();
    const watched = routeFocus = watchFocus(trigger);
    const controller = routeController = new AbortController();
    pendingRoute = locationHash(location);
    retainingRoute = !useShell;
    routeFeedbackTimer = setTimeout(() => {
      if (!isCurrent()) return;
      routeFeedback = true;
      showSlowFilesShell();
      void tick().then(() => {
        if (isCurrent() && watched.mayFocus() && document.activeElement === document.body) {
          document.getElementById('route-status')?.focus({ preventScroll: true });
        }
      });
    }, routeFeedbackDelay);

    listingRequest++; trashRequest++; ownUsageRequest++; accountsRequest++; allUsageRequest++;
    ownUsageRetrying = false;
    workspaceRefreshRequest++; workspaceRefreshing = false;
    searchRequest++; searchController?.abort(); searchController = null;
    if (searchTimer) { clearTimeout(searchTimer); searchTimer = null; }
    pendingMove = null;
    endDrag();
    if (!moveNeedsRefresh) dropError = '';
    clearSelection(); clearCompletedFeedback(); closeBulk();
    previewEntry = null; previewTrigger = null; share = null; directShare = null; settleTreeSharing(false);
    if (!folderBusy) createOpen = false;
    if (!renameBusy) renameId = '';
    if (!trashBusy) trashTarget = null;
    if (!moveBusy && !moveNeedsRefresh) closeMove();
    changePasswordOpen = false; createAccountOpen = false; disableTarget = null; authorityTarget = null;
    resetId = ''; resetPassword = ''; accountHandle = ''; clearCreateAccountSecrets(); currentPassword = ''; newPassword = ''; accountNotice = '';
    if (useShell) showDestination(destination);
    try {
      if (isFiles) {
        if (!folders[0] && !await refreshRoot(isCurrent, controller.signal)) return false;
        if (!isCurrent()) return false;
        const root = folders[0];
        if (!path && location.folder && location.folder !== root.id) {
          const result = await listing.path(location.folder, controller.signal);
          if (!isCurrent()) return false;
          if (result.items[0]?.parent_id !== null || result.items.at(-1)?.id !== location.folder) throw new Error('Invalid folder path');
          path = [result.items[0].id === root.id ? root : result.items[0], ...result.items.slice(1)];
        }
        path ??= [root];
        readablePath = path;
        if (routeFeedback) showSlowFilesShell();
        const folder = path.at(-1)!;
        const query = location.query;
        const resultSort = location.searchSort ?? 'name_asc';
        const read = (cursor: string | null) => query
          ? listing.search(query, folder.id === root.id ? null : folder.id, cursor, controller.signal, resultSort)
          : listing.children(folder.id, cursor, location.sort, controller.signal);
        let cursors = context?.cursors ?? [null];
        let page = context?.page ?? 0;
        let searchCursors = context?.searchCursors ?? [null];
        let searchPage = context?.searchPage ?? 0;
        const cursor = query ? searchCursors[searchPage] : cursors[page];
        let result;
        let restarted = false;
        try { result = await read(cursor); }
        catch (error) {
          if (!isCurrent()) return false;
          if (!cursor || !staleCursor(error)) throw error;
          if (query) { searchCursors = [null]; searchPage = 0; }
          else { cursors = [null]; page = 0; }
          scroll = 0;
          restarted = true;
          result = await read(null);
        }
        if (!isCurrent()) return false;
        clearSearch();
        currentFolder = folder; folderPath = path;
        pageCursors = [...cursors]; pageIndex = page;
        if (query) {
          searchInput = query; searchQuery = query; browseDirty = true;
          searchPageCursors = [...searchCursors]; searchPageIndex = searchPage;
          searchItems = result.items as SearchHit[]; searchNextCursor = result.next_cursor; searchHasMore = result.has_more;
          for (const hit of searchItems) {
            knownParents.set(hit.entry.id, hit.entry.parent_id);
            for (const ancestor of hit.location) knownParents.set(ancestor.id, ancestor.parent_id);
          }
        } else {
          entries = result.items as Entry[]; entriesFolderId = folder.id;
          nextCursor = result.next_cursor; hasMore = result.has_more;
          for (const entry of entries) knownParents.set(entry.id, entry.parent_id);
        }
        listingError = '';
        if (restarted) notice = query ? 'Files changed. Search results have been restarted.' : 'This folder changed. The listing has been restarted.';
      } else if (isSharingDestination(destination)) {
        clearSearch();
        sharedLocation = { folder: location.folder, file: location.file };
      } else if (destination === 'libraries') {
        clearSearch();
        librariesError = '';
        try {
          const result = await listing.libraries(controller.signal);
          if (!isCurrent()) return false;
          libraries = result.items;
        } catch {
          if (!isCurrent()) return false;
          libraries = [];
          librariesError = 'Could not load your libraries.';
        }
      } else if (destination === 'trash') {
        try {
          const result = await listing.deleted(null, location.trashSort, controller.signal);
          if (!isCurrent()) return false;
          deletedItems = result.items; trashNextCursor = result.next_cursor; trashHasMore = result.has_more;
          trashError = ''; trashRetry = null;
        } catch {
          if (!isCurrent()) return false;
          deletedItems = []; trashNextCursor = null; trashHasMore = false;
          trashRetry = { cursor: null, page: 0 };
          trashError = 'Could not load trash. Try again.';
        }
        trashPageCursors = [null]; trashPageIndex = 0; trashNotice = '';
        clearSearch();
      } else {
        clearSearch();
        if (destination === 'account') await loadOwnUsage(controller.signal);
        if (destination === 'settings') { await loadAccounts(); if (isCurrent()) await loadInstance(); }
      }
      if (!isCurrent()) return false;
      loaded = true;
    } catch (error) {
      if (!isCurrent()) return false;
      clearSearch();
      entries = []; entriesFolderId = ''; hasMore = false; nextCursor = null; pageIndex = 0;
      if (path?.length) { folderPath = path; currentFolder = path.at(-1)!; }
      else { folderPath = [folders[0] ?? emptyRoot]; currentFolder = folderPath[0]; }
      restorationFailed = true;
      restorationUnavailable = error instanceof HttpError && error.status === 404;
      listingError = restorationUnavailable ? 'This folder is unavailable. Open Files to continue.' : 'Could not load this folder. Check your connection and try again.';
    } finally {
      if (isCurrent()) {
        folderSort = location.sort; searchSort = location.searchSort ?? 'name_asc';
        trashSort = location.trashSort ?? 'name_asc'; accountSort = location.accountSort ?? 'name_asc';
        showDestination(destination);
        listingRequest++; trashRequest++; searchRequest++;
        loading = false; trashLoading = false; trashPaging = null; searchLoading = false;
        trashRetry = destination === 'trash' ? trashRetry : null;
        restorationPending = false; presentedRoute = true;
        routeFocus = null;
        clearRouteFeedback();
        await tick();
        if (isCurrent()) {
          const active = document.activeElement;
          const leavingSheet = trigger?.closest('[data-mobile=true]') && active?.matches('[data-sidebar=trigger]');
          const restoreScroll = !pendingMove && (!active?.closest('.account-content, .files-content, .libraries-content, .trash-content, .settings-content') || watched.mayFocus());
          if (restoreScroll) window.scrollTo(0, scroll);
          const heading = document.getElementById(accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : librariesView ? 'libraries-title' : sharedView ? 'shared-title' : 'folder-title');
          if ((pendingMove && active === heading) || watched.mayFocus() || active === document.body || leavingSheet) {
            const retry = document.getElementById(restorationFailed && !restorationUnavailable ? 'folder-page-retry' : destination === 'trash' && trashError ? 'trash-page-retry' : '');
            const items = pendingMove && active === heading ? listItems() : [];
            const moved = pendingMove ? items[moveIndex(-1, items.length, pendingMove)] : null;
            const visibleItem = isFiles && restoreScroll && scroll > 0 ? listItems().find(item => {
              const bounds = item.getBoundingClientRect();
              return bounds.top >= 0 && bounds.bottom <= window.innerHeight;
            }) : null;
            (retry ?? moved ?? visibleItem ?? heading)?.focus({ preventScroll: true });
            moved?.scrollIntoView({ block: 'nearest' });
          }
          pendingMove = null;
          if (!restorationFailed) { historyReady = true; saveNavigation(); }
        }
      }
      watched.stop();
    }
    return loaded && isCurrent();
  }

  async function home(event: MouseEvent): Promise<void> {
    event.preventDefault();
    if (!user) { view = 'login'; return; }
    await navigate('libraries');
  }

  async function createLibrary(name: string): Promise<Folder> {
    const generation = sessionGeneration;
    const created = await listing.createLibrary(name);
    if (generation === sessionGeneration && librariesView && !libraries.some((folder) => folder.id === created.id)) {
      libraries = [...libraries, created];
    }
    return created;
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    const units = ['KB', 'MB', 'GB', 'TB']; let value = bytes / 1024; let index = 0;
    while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
    return `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })} ${units[index]}`;
  }
  let sessionGeneration = $state(0);
  let loggingOut = $state(false);
  let accounts = $state<Account[]>([]);
  const enabledAdministratorCount = $derived(accounts.filter((account) => account.enabled && account.role === 'administrator').length);
  let accountHandle = $state('');
  let accountPassword = $state('');
  let accountPasswordConfirmation = $state('');
  let accountPasswordError = $state('');
  let accountPasswordVisible = $state(false);
  let accountPasswordConfirmationVisible = $state(false);
  let currentPassword = $state('');
  let newPassword = $state('');
  let resetId = $state('');
  let resetPassword = $state('');
  let accountNotice = $state('');
  let accountBusy = $state(false);
  let accountsLoading = $state(false);
  let accountsError = $state('');
  let accountsRequest = 0;
  type UsageState = 'loading' | 'ready' | 'error';
  let ownUsage = $state<Usage | null>(null);
  let ownUsageState = $state<UsageState>('loading');
  let ownUsageRequest = 0;
  let ownUsageRetrying = $state(false);
  let allUsage = $state<AllUsage | null>(null);
  let allUsageState = $state<UsageState>('loading');
  let allUsageRequest = 0;
  const usageByAccount = $derived(new Map(allUsage?.accounts.map((usage) => [usage.account_id, usage]) ?? []));
  // Loading and unknown must never read as zero.
  function usageLabel(state: UsageState, bytes: number | null | undefined): string {
    return state === 'loading' ? 'Loading…' : state === 'error' || typeof bytes !== 'number' || !Number.isFinite(bytes) ? 'Unknown' : formatSize(bytes);
  }
  let handle = $state('');
  let password = $state('');
  let setupAuthorization = $state('');
  const emptyRoot: Folder = { id: '', parent_id: null, name: 'Files', kind: 'folder', etag: '"g1"' };
  let currentFolder = $state<Folder>(emptyRoot);
  let folders = $state<Folder[]>([]);
  let entries = $state<Entry[]>([]);
  let entriesFolderId = $state('');
  let selectedIds = $state<string[]>([]);
  let selectionAnchor: string | null = null;

  function clearSelection(): void { selectedIds = []; selectionAnchor = null; }
  async function clearSelectionFromToolbar(): Promise<void> {
    const target = document.querySelector<HTMLInputElement>('.selectable-row input:checked');
    clearSelection();
    await tick();
    (target ?? document.getElementById('folder-title'))?.focus();
  }
  function selectEntry(id: string, checked: boolean, range = false): void {
    const anchorIndex = activeEntries.findIndex((entry) => entry.id === selectionAnchor);
    const targetIndex = activeEntries.findIndex((entry) => entry.id === id);
    if (targetIndex < 0) return;
    const ids = range && anchorIndex >= 0
      ? activeEntries.slice(Math.min(anchorIndex, targetIndex), Math.max(anchorIndex, targetIndex) + 1).map((entry) => entry.id)
      : [id];
    selectedIds = checked ? [...new Set([...selectedIds, ...ids])] : selectedIds.filter((item) => !ids.includes(item));
    if (!range || anchorIndex < 0) selectionAnchor = id;
  }
  function selectionKeydown(event: KeyboardEvent, id: string): void {
    if (event.code !== 'Space') return;
    event.preventDefault();
    selectEntry(id, !(event.currentTarget as HTMLInputElement).checked, event.shiftKey);
  }

  let shortcutsOpen = $state(false);
  let shortcutsInvoker: HTMLElement | null = null;
  function openShortcuts(invoker: HTMLElement | null): void {
    shortcutsInvoker = invoker;
    shortcutsOpen = true;
  }
  function restoreShortcutFocus(event: Event): void {
    event.preventDefault();
    restoreFocus(shortcutsInvoker);
    shortcutsInvoker = null;
  }
  let listFocusId: string | null = null;
  // A move key pressed on the heading while rows are disabled by a load; applied when the load ends.
  let pendingMove: 'next' | 'previous' | 'first' | 'last' | null = null;
  const itemSelector = '.item-name, .entry-selection input';
  function listItems(): HTMLElement[] { return [...document.querySelectorAll<HTMLElement>('.files-content .file-row .item-name')]; }
  function listItemFor(id: string | null): HTMLElement | null {
    return id ? document.querySelector<HTMLElement>(`.files-content .file-row[data-entry-id="${CSS.escape(id)}"] .item-name`) : null;
  }
  function trackListFocus(event: FocusEvent): void {
    const target = event.target instanceof HTMLElement ? event.target : null;
    listFocusId = target?.matches(itemSelector) ? target.closest<HTMLElement>('.file-row')?.dataset.entryId ?? null : null;
    if (target?.id !== 'folder-title') pendingMove = null;
  }
  function extendSelection(fromId: string, toId: string): void {
    const anchor = activeEntries.some((entry) => entry.id === selectionAnchor) ? selectionAnchor! : fromId;
    const a = activeEntries.findIndex((entry) => entry.id === anchor);
    const b = activeEntries.findIndex((entry) => entry.id === toId);
    if (a < 0 || b < 0) return;
    selectedIds = activeEntries.slice(Math.min(a, b), Math.max(a, b) + 1).map((entry) => entry.id);
    selectionAnchor = anchor;
  }
  function listKeydown(event: KeyboardEvent): void {
    if (!user || !filesView || shortcutsBlocked(event.target, document)) return;
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (restorationPending && target?.id !== 'folder-title') return;
    const row = target?.matches(itemSelector) ? target.closest<HTMLElement>('.file-row') : null;
    const onHeading = target?.id === 'folder-title';
    const inList = !!target?.closest('.files-content .file-row') || onHeading;
    const intent = listIntent(event, !!row || onHeading, inList);
    if (!intent) return;
    if (intent.kind === 'search') {
      const input = document.getElementById('file-search') as HTMLInputElement | null;
      if (!input || input.disabled) return;
      event.preventDefault(); input.focus(); input.select();
    } else if (intent.kind === 'help') {
      event.preventDefault(); openShortcuts(target);
    } else if (intent.kind === 'move') {
      // From the heading, arrows enter the list; this keeps keyboard flow after paging or refresh.
      const items = listItems();
      const id = row?.dataset.entryId;
      const current = row ? items.findIndex((item) => item.closest('.file-row') === row) : -1;
      const next = moveIndex(current, items.length, intent.to);
      if (onHeading && (next < 0 || items[next].matches(':disabled'))) {
        if (restorationPending) { event.preventDefault(); pendingMove = intent.to; }
        return;
      }
      if (next < 0) return;
      event.preventDefault();
      if (intent.extend && id && !fileMutationBusy) extendSelection(id, items[next].closest<HTMLElement>('.file-row')!.dataset.entryId!);
      items[next].focus();
    } else if (row) {
      const id = row.dataset.entryId!;
      const item = row.querySelector<HTMLElement>('.item-name')!;
      const entry = activeEntries.find((candidate) => candidate.id === id);
      if (!entry) return;
      if (intent.kind === 'open') {
        if (target !== item) { event.preventDefault(); item.click(); }
      } else if (intent.kind === 'clear') {
        if (selectedIds.length) { event.preventDefault(); clearSelection(); }
      } else if (fileMutationBusy || loading) {
        return;
      } else if (intent.kind === 'toggle') {
        event.preventDefault(); selectEntry(id, !selectedIds.includes(id));
      } else if (intent.kind === 'rename') {
        event.preventDefault(); entryAction('rename', entry, item);
      } else if (intent.kind === 'trash') {
        const targets = actionTargets(id, selectedEntries, activeEntries);
        event.preventDefault();
        // Same confirmation dialogs as the row menu and the selection toolbar.
        if (targets.length === 1) entryAction('delete', targets[0], item);
        else void beginBulk('trash', item, targets);
      }
    }
  }
  function searchLocation(hit: SearchHit): string {
    return [hit.location[0]?.name || 'Files', ...hit.location.slice(1).map((folder) => folder.name)].join(' / ');
  }
  type BulkAction = 'move' | 'trash';
  type BulkResult = { entry: Entry; location: string; status: 'pending' | 'succeeded' | 'failed' | 'uncertain' | 'changed'; message: string };
  type BulkReport = { action: BulkAction; destination: Folder | null; results: BulkResult[] };
  let bulkAction = $state<BulkAction | null>(null);
  let bulkTargets = $state<Entry[]>([]);
  let bulkReport = $state<BulkReport | null>(null);
  let bulkBusy = $state(false);
  const bulkSucceeded = $derived(bulkReport?.results.filter((result) => result.status === 'succeeded').length ?? 0);
  const bulkComplete = $derived(!!bulkReport && !bulkBusy && bulkSucceeded === bulkReport.results.length);
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
  let listingRequest = $state(0);
  let folderSort = $state<FolderSort>('name_asc');
  let folderSortAttempt = $state<{ request: number; sort: FolderSort; failed: boolean } | null>(null);
  const currentSortAttempt = $derived(folderSortAttempt?.request === listingRequest ? folderSortAttempt : null);
  const pendingFolderSort = $derived(currentSortAttempt?.failed ? undefined : currentSortAttempt?.sort);
  const folderSortBlocked = $derived(loading || bulkBusy || bulkNeedsRefresh || restorationPending || restorationFailed);
  let searchSort = $state<ResultSort>('name_asc');
  let trashSort = $state<ResultSort>('name_asc');
  let accountSort = $state<AccountSort>('name_asc');
  const sortedAccounts = $derived([...accounts].sort((left, right) => {
    const field = accountSort.split('_')[0];
    const value = (account: Account) => field === 'role' ? account.role : field === 'status' ? (account.enabled ? 'Active' : 'Disabled') : account.handle;
    const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
    return compare(value(left), value(right)) * (accountSort.endsWith('_desc') ? -1 : 1) || compare(left.handle, right.handle) || compare(left.account_id, right.account_id);
  }));

  function nextSort<T extends string>(current: T, column: string): T {
    return `${column}_${current === `${column}_asc` ? 'desc' : 'asc'}` as T;
  }


  async function setResultSort(surface: 'search' | 'trash', sort: ResultSort): Promise<void> {
    if (resultSortBlocked) return;
    clearCompletedFeedback(); clearSelection();
    if (surface === 'search') { searchSort = sort; searchPageCursors = [null]; searchPageIndex = 0; searchNextCursor = null; searchHasMore = false; await runSearch(); }
    else { trashSort = sort; deletedItems = []; trashPageCursors = [null]; trashPageIndex = 0; trashNextCursor = null; trashHasMore = false; await loadTrash(); }
    saveNavigation();
  }

  function setAccountSort(column: string): void {
    if (accountBusy || accountsLoading) return;
    clearCompletedFeedback(); accountSort = nextSort(accountSort, column); saveNavigation();
  }
  let searchInput = $state('');
  let searchQuery = $state('');
  let searchItems = $state<SearchHit[]>([]);
  const activeEntries = $derived(searchQuery ? searchItems.map((hit) => hit.entry) : entries);
  const selectedEntries = $derived(activeEntries.filter((entry) => selectedIds.includes(entry.id)));
  const allLoadedSelected = $derived(activeEntries.length > 0 && selectedEntries.length === activeEntries.length);
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
  let searchComposing = false;
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let searchController: AbortController | null = null;
  let browseDirty = false;
  let notice = $state('');
  let actionNotice = $state('');
  let feedbackRevision = $state(0);
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
  let moveCloseRefreshBusy = $state(false);
  const dragType = 'application/x-sylux-entry';
  const knownParents = new Map<string, string | null>();
  let trashTarget = $state<Entry | null>(null);
  let trashTrigger: HTMLElement | null = null;
  let deletedItems = $state<DeletedEntry[]>([]);
  let trashRequest = 0;
  let showTrash = $state(false);
  const filesView = $derived(!showTrash && !accountView && !settingsView && !librariesView && !sharedView);
  const namedLibrary = $derived(folderPath[0]?.id && folderPath[0].id !== folders[0]?.id ? folderPath[0] : undefined);
  const librariesCurrent = $derived(librariesView ? 'page' : filesView && namedLibrary ? 'location' : undefined);
  const filesCurrent = $derived(filesView && !namedLibrary);
  let trashError = $state('');
  let trashBusy = $state(false);
  const resultSortBlocked = $derived(bulkBusy || bulkNeedsRefresh || restorationPending || trashBusy);

  let removalSelection = $state<string[]>([]);
  let removalPreview = $state<(RemovalPreview & { all: boolean }) | null>(null);
  let removalPreparing = $state<'selected' | 'all' | null>(null);
  let removalSubmitting = $state(false);
  let removalError = $state('');
  let removalTrigger: HTMLElement | null = null;
  type PendingRemoval = { id: string; confirmation: string; result: RemovalResult | null; message: string };
  let removalPending = $state<PendingRemoval | null>(null);
  let removalChecking = $state(false);

  function saveRemoval(pending: PendingRemoval | null): void {
    removalPending = pending;
    if (!user) return;
    const key = `sylux-removal:${user.id}`;
    try {
      if (pending && !pending.result) sessionStorage.setItem(key, JSON.stringify({ id: pending.id, confirmation: pending.confirmation }));
      else sessionStorage.removeItem(key);
    } catch { removalError = 'This browser cannot save removal recovery across reloads. Keep this tab open until the outcome is confirmed.'; }
  }
  function recoverRemoval(): void {
    if (!user) return;
    try {
      const saved = sessionStorage.getItem(`sylux-removal:${user.id}`);
      if (!saved) return;
      const pending = JSON.parse(saved);
      if (!/^[a-f0-9]{32}$/.test(pending.id) || typeof pending.confirmation !== 'string' || pending.confirmation.length > 60000) throw new Error('Invalid removal recovery');
      removalPending = { ...pending, result: null, message: 'Removal outcome unconfirmed. Check its status or retry the same removal.' };
    } catch { removalError = 'Could not read saved removal recovery. Keep the original tab open to resolve its outcome.'; }
  }
  async function refreshRemovalSelection(message: string): Promise<void> {
    removalPreview = null; removalSelection = [];
    if (showTrash) await loadTrash();
    removalError = message;
  }
  async function prepareRemoval(all: boolean, trigger: HTMLElement): Promise<void> {
    if (removalBlocked || (!all && !removalSelection.length)) return;
    const generation = sessionGeneration, navigation = navigationRequest;
    removalTrigger = trigger; removalPreparing = all ? 'all' : 'selected'; removalError = '';
    try {
      const preview = await listing.removalPreview(all ? { kind: 'all' } : { kind: 'selected', items: deletedItems.filter(item => removalSelection.includes(item.id)).map(({ id, etag }) => ({ id, etag })) });
      if (generation !== sessionGeneration || navigation !== navigationRequest || !showTrash) return;
      removalPreview = { ...preview, all };
    } catch (error) {
      if (generation !== sessionGeneration || navigation !== navigationRequest) return;
      if (error instanceof HttpError && [404, 409].includes(error.status)) await refreshRemovalSelection('Trash changed. Review the current items and confirm again.');
      else removalError = 'Could not prepare removal. Try again.';
    } finally { if (generation === sessionGeneration) removalPreparing = null; }
  }
  async function finishRemoval(result: RemovalResult): Promise<void> {
    clearCompletedFeedback();
    actionNotice = `Removed ${result.count} ${result.count === 1 ? 'item' : 'items'} permanently`;
    saveRemoval(result.cleanup === 'complete' ? null : { id: result.operation_id, confirmation: '', result, message: 'Removal is complete. Storage cleanup is pending.' });
    removalSelection = [];
    if (showTrash) {
      deletedItems = []; trashPageIndex = 0; trashPageCursors = [null]; trashNextCursor = null; trashHasMore = false; trashRetry = null;
      await loadTrash();
    }
  }
  async function submitRemoval(pending: PendingRemoval): Promise<void> {
    const generation = sessionGeneration;
    removalSubmitting = true; removalError = '';
    saveRemoval(pending);
    try {
      const result = await listing.removePermanently(pending.id, pending.confirmation);
      if (generation !== sessionGeneration) return;
      removalPreview = null;
      if (result.state === 'unknown') saveRemoval({ ...pending, message: 'Removal outcome is still unconfirmed. Check its status or retry the same removal.' });
      else await finishRemoval(result);
    } catch (error) {
      if (generation !== sessionGeneration) return;
      removalPreview = null;
      if (moveOutcomeUncertain(error) || !(error instanceof HttpError)) {
        saveRemoval({ ...pending, message: 'Removal outcome unconfirmed. Check its status or retry the same removal.' });
      } else {
        saveRemoval(null);
        await refreshRemovalSelection(error.status === 409 && error.code !== 'removal_not_committed' ? 'Trash changed. Review the current items and confirm again.' : 'Removal was rejected or its confirmation expired. Review Trash and confirm again.');
      }
    } finally { if (generation === sessionGeneration) removalSubmitting = false; }
  }
  async function confirmRemoval(): Promise<void> {
    if (!removalPreview || removalSubmitting || (removalPending && !removalPending.result)) return;
    await submitRemoval({ id: crypto.randomUUID().replaceAll('-', ''), confirmation: removalPreview.confirmation, result: null, message: 'Removal outcome unconfirmed. Check its status or retry the same removal.' });
  }
  async function replayRemoval(): Promise<void> {
    if (!removalPending || removalPending.result || removalChecking || removalSubmitting) return;
    await submitRemoval(removalPending);
  }
  async function checkRemoval(): Promise<void> {
    if (!removalPending || removalChecking || removalSubmitting) return;
    const generation = sessionGeneration, pending = removalPending;
    removalChecking = true;
    try {
      const result = await listing.removalStatus(pending.id);
      if (generation !== sessionGeneration || removalPending !== pending) return;
      if (result.state === 'unknown') { saveRemoval({ ...pending, message: 'Removal outcome is still unconfirmed. Retry the same removal to resolve a request that did not arrive, including an expired confirmation.' }); return; }
      if (pending.result) saveRemoval(result.cleanup === 'complete' ? null : { ...pending, result, message: 'Removal is complete. Storage cleanup is pending.' });
      else await finishRemoval(result);
    } catch {
      if (generation === sessionGeneration && removalPending === pending) saveRemoval({ ...pending, message: pending.result ? 'Removal is complete. Could not check storage cleanup. Try again.' : 'Removal outcome is still unconfirmed. Check your connection and try again.' });
    } finally { if (generation === sessionGeneration) removalChecking = false; }
  }

  let restoringId = $state('');
  let restoringName = $state('');
  let namespaceUnknown = $state<{ action: 'trash' | 'restore'; entry: Entry; inspected: boolean; message: string } | null>(null);
  let namespaceInspecting = $state(false);
  let trashLoading = $state(false);
  let trashPageIndex = $state(0);
  let trashPageCursors = $state<(string | null)[]>([null]);
  let trashNextCursor = $state<string | null>(null);
  let trashHasMore = $state(false);
  let trashRetry = $state<{ cursor: string | null; page: number } | null>(null);
  let trashPaging = $state<'next' | 'previous' | 'retry' | null>(null);
  let trashNotice = $state('');
  const removalBlocked = $derived(trashBusy || trashLoading || !!namespaceUnknown || !!removalPreparing || removalSubmitting || (!!removalPending && !removalPending.result));

  let renameId = $state('');
  let renameName = $state('');
  let renameError = $state('');
  let renameBusy = $state(false);
  let renameTrigger: HTMLElement | null = null;
  let renameInput = $state<HTMLInputElement | null>(null);
  let destinationAction = $state<'move' | 'copy'>('move');
  type PendingCopy = { ticket: string; destination: string };
  let pendingCopy = $state<PendingCopy | null>(null);
  let copyRecovery = $state<'status' | 'retry' | null>(null);
  let copyRecoveryMessage = $state('');
  let copyStorageFailed = $state(false);
  let moveTarget = $state<Entry | null>(null);
  let movePath = $state<Folder[]>([]);
  let moveFolders = $state<Folder[]>([]);
  let moveCursor = $state<string | null>(null);
  let moveError = $state('');
  let moveLoading = $state(false);
  let moveDestinationReady = $state(false);
  let moveBusy = $state(false);
  const destinationBusy = $derived(moveBusy || bulkBusy);
  const moveTargets = $derived(bulkAction === 'move' ? bulkTargets : moveTarget ? [moveTarget] : []);
  const excludedMoveFolders = $derived(new Set(moveTargets.filter(entry => entry.kind === 'folder').map(entry => entry.id)));
  const destinationFolders = $derived(destinationAction === 'copy' ? moveFolders :
    moveFolders.filter(folder => !excludedMoveFolders.has(folder.id)));
  const moveAlreadyHere = $derived(moveTargets.length > 0 &&
    moveTargets.every(entry => entry.parent_id === movePath.at(-1)?.id));
  const canConfirmMove = $derived(moveDestinationReady && !moveLoading && !moveNeedsRefresh &&
    movePath.length > 0 && !moveAlreadyHere && !movePath.some(folder => excludedMoveFolders.has(folder.id)));
  let moveRequest = 0;
  let moveTrigger: HTMLElement | null = null;
  let moveDialog = $state<HTMLElement | null>(null);
  type ReplacementPrompt = { file: File; folder: Folder; target: { id: string; name: string; etag: string } };
  let replacementPrompt = $state<ReplacementPrompt | null>(null);
  const createAudience = publicFolderNotice(() => client, () => createOpen ? currentFolder.id || null : null);
  const copyAudience = publicFolderNotice(() => client, () =>
    destinationAction === 'copy' && moveDestinationReady ? movePath.at(-1)?.id ?? null : null);
  const replacementAudience = publicFolderNotice(() => client, () => replacementPrompt?.folder.id ?? null);
  let uploadReplacement = $state(false);
  let transferName = $state('');
  let progress = $state(0);
  let transferPhase = $state<'checking' | 'uploading' | 'saving' | 'complete' | ''>('');
  let transferBusy = $state(false);
  let uploadCancelable = $state(false);
  let uploadId = $state('');
  let unresolvedUploadId = $state('');
  let unresolvedUploadName = $state('');
  let unresolvedCancel = $state(false);
  let resumeReady = $state(false);
  let finishReady = $state(false);
  let cancelPending = $state(false);
  let uploadExpired = $state(false);
  // Recovery actions belong to one upload; a newly unresolved upload starts unchecked.
  function markUnresolved(id: string, name: string) {
    unresolvedUploadId = id; unresolvedUploadName = name;
    resumeReady = false; finishReady = false; uploadExpired = false;
  }
  let transferWaiting = $state('');
  const expiredResumeNotice = 'The 24-hour upload window ended before this upload finished, so it cannot be resumed. Cancel it, then upload the file again.';
  const expiredSaveNotice = 'The 24-hour upload window ended before this upload was saved, so it cannot be saved. Cancel it, then upload the file again.';
  let resumeFile = $state<File | null>(null);
  let reselectInput = $state<HTMLInputElement>();
  let controller: AbortController | null = null;
  let publicUploadCheck: AbortController | null = null;
  let uploadBatchBusy = $state(false);
  let uploadBatchStatus = $state('');
  type UploadResult = { path: string; kind: 'file' | 'folder'; size: number; sent: number; sentComplete: boolean; parent?: Folder; inspected?: boolean; inspectionError?: string; status: 'waiting' | 'active' | 'saved' | 'created' | 'failed' | 'unknown' | 'canceled' };
  let uploadResults = $state<UploadResult[]>([]);
  let uploadBatchDestination = $state('');
  let uploadNotice = $state('');
  let uploadBatchStage = $state('Preparing');
  let uploadBatchCanceled = $state(false);
  const batchFiles = $derived(uploadResults.filter((item) => item.kind === 'file'));
  const batchBytes = $derived(batchFiles.reduce((total, item) => total + item.size, 0));
  const batchSent = $derived(batchFiles.reduce((total, item) => total + item.sent, 0));
  const batchSaved = $derived(batchFiles.filter((item) => item.status === 'saved').length);
  const batchFoldersCreated = $derived(uploadResults.filter((item) => item.status === 'created').length);
  const batchActive = $derived(uploadResults.find((item) => item.status === 'active'));
  let uploadBatchCurrentIndex = $state(-1);
  const batchCurrent = $derived(uploadResults[uploadBatchCurrentIndex]);
  const batchCurrentDestination = $derived([uploadBatchDestination, ...(batchCurrent?.path.split('/').slice(0, -1) ?? [])].join(' / '));
  const batchDisplayStage = $derived(!uploadBatchCanceled ? uploadBatchStage : uploadBatchStage === 'Saving' ? 'Stopping after this save' : uploadBatchStage === 'Creating folder' ? 'Stopping after this folder' : uploadBatchStage === 'Sending' ? 'Canceling current upload' : 'Stopping upload');
  const batchSendPercent = $derived(Math.floor(100 * (batchBytes ? batchSent / batchBytes : batchFiles.length ? batchFiles.filter((item) => item.sentComplete).length / batchFiles.length : 0)));
  let uploadResultsExpanded = $state(false);
  let uploadReportVisible = $state(false);
  let compactUnresolvedUpload = $state(false);
  const uploadComplete = $derived(!uploadBatchBusy && uploadResults.length > 0 && uploadResults.every((item) => item.status === 'saved' || item.status === 'created'));

  type UploadReport = { id: number; destination: string; status: string; notice: string; results: UploadResult[]; expanded: boolean };
  let previousUploadReports = $state<UploadReport[]>([]);
  let uploadReportSequence = 0;

  function retainUploadReport(): void {
    if (!uploadReportVisible || !uploadResults.some((result) => result.status === 'unknown')) return;
    previousUploadReports.push({ id: ++uploadReportSequence, destination: uploadBatchDestination, status: uploadBatchStatus, notice: uploadNotice, results: uploadResults.map((result) => ({ ...result })), expanded: uploadResultsExpanded });
  }

  function clearCompletedFeedback(): void {
    feedbackRevision++;
    if (!removalSubmitting) removalPreview = null;
    if (!pendingCopy) copyRecoveryMessage = '';
    notice = ''; actionNotice = ''; removalError = ''; removalSelection = [];
    if (!bulkBusy) {
      if (!bulkNeedsRefresh) { bulkReport = null; bulkError = ''; }
    }
    previousUploadReports = previousUploadReports.filter((report) => report.results.some((result) => result.status === 'unknown'));
    if (!uploadBatchBusy) {
      if (unresolvedUploadId || uploadResults.some((result) => result.status === 'unknown')) compactUnresolvedUpload = true;
      else { uploadReportVisible = false; uploadResultsExpanded = false; }
    }
  }

  async function dismissActionNotice(): Promise<void> {
    feedbackRevision++; actionNotice = '';
    await tick();
    document.getElementById(accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : 'folder-title')?.focus();
  }

  async function dismissBulkReport(): Promise<void> {
    if (bulkBusy || bulkNeedsRefresh) return;
    bulkReport = null; bulkError = '';
    await tick();
    document.getElementById(accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : 'folder-title')?.focus();
  }

  let workspaceRefreshing = $state(false);
  let refreshSourceFolder = $state('');
  let folderRetryBusy = $state(false);
  let folderRetryRequest = $state(0);
  const currentFolderRetryBusy = $derived(folderRetryBusy && folderRetryRequest === listingRequest);
  let workspaceRefreshRequest = 0;
  async function refreshWorkspace(): Promise<void> {
    if (restorationPending) return;
    if (restorationFailed && !showTrash) {
      await retryFolder();
      return;
    }
    if (showTrash) {
      if (trashLoading || trashBusy) return;
      clearCompletedFeedback();
      await loadTrash();
      return;
    }
    if (loading || searchLoading || bulkBusy) return;
    const request = ++workspaceRefreshRequest;
    const generation = sessionGeneration;
    clearCompletedFeedback();
    publicLinksRefresh++;
    workspaceRefreshing = true; refreshSourceFolder = currentFolder.id;
    try {
      if (bulkNeedsRefresh) await reconcileBulk();
      else await refresh();
    } finally {
      if (generation === sessionGeneration && request === workspaceRefreshRequest) {
        workspaceRefreshing = false;
        // Rows are disabled while loading, which drops a focused row to body. Put it back.
        await tick();
        if (generation === sessionGeneration && request === workspaceRefreshRequest && (!document.activeElement || document.activeElement === document.body)) listItemFor(listFocusId)?.focus({ preventScroll: true });
      }
    }
  }

  async function retryFolder(): Promise<void> {
    if (restorationFailed) { await restoreNavigation(); return; }
    if ((folderRetryBusy && folderRetryRequest === listingRequest) || loading) return;
    const generation = sessionGeneration;
    const request = listingRequest + 1;
    folderRetryBusy = true; folderRetryRequest = request;
    try { await refresh(); }
    finally { if (generation === sessionGeneration && request === folderRetryRequest) folderRetryBusy = false; }
  }

  function uploadSuccessMessage(): string {
    return uploadResults.length === 1 ? `Uploaded to ${uploadBatchDestination}` : `${uploadResults.length} items uploaded`;
  }

  async function inspectUploadDestination(result: UploadResult): Promise<void> {
    if (!result.parent) return;
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    result.inspectionError = '';
    const parent = await freshUploadFolder(result.parent).catch(() => null);
    if (generation !== sessionGeneration || navigation !== navigationRequest) return;
    if (!parent) { result.inspectionError = 'Could not open this destination. Check your connection and try again.'; return; }
    const loaded = await openFolder(parent);
    if (generation !== sessionGeneration) return;
    if (loaded) result.inspected = true;
    else result.inspectionError = 'Destination was not opened. Inspect it before dismissing this result.';
  }

  function uploadReportDismissible(results: UploadResult[]): boolean {
    return results.every((result) => result.status !== 'unknown' || (result.kind === 'folder' && result.inspected));
  }

  async function dismissUploadReport(id?: number): Promise<void> {
    if (id !== undefined) {
      const report = previousUploadReports.find((report) => report.id === id);
      if (!report || !uploadReportDismissible(report.results)) return;
      previousUploadReports = previousUploadReports.filter((report) => report.id !== id);
    } else {
      if (uploadBatchBusy || unresolvedUploadId || !uploadReportDismissible(uploadResults)) return;
      uploadReportVisible = false; uploadResultsExpanded = false;
    }
    await tick();
    document.getElementById(accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : 'folder-title')?.focus();
  }
  let unresolvedUploadResult: number | null = null;
  let replacementDecision: ((prompt: ReplacementPrompt | null) => void) | null = null;
  const fileMutationBusy = $derived(restorationPending || restorationFailed || (workspaceRefreshing && loading) || bulkBusy || bulkNeedsRefresh || moveNeedsRefresh || !!dropPending || moveBusy || renameBusy || trashBusy || !!namespaceUnknown || folderBusy || transferBusy || uploadBatchBusy);

  function restoreFocus(trigger: HTMLElement | null, fallback = 'folder-title'): void {
    if (trigger?.isConnected && !trigger.matches(':disabled')) trigger.focus({ preventScroll: true });
    else document.getElementById(fallback)?.focus({ preventScroll: true });
  }

  function expireSession(generation: number): void {
    if (generation !== sessionGeneration || !user) return;
    sessionGeneration++; clearAccountForms(); clearWorkspace(true);
    client.setSessionExpiredHandler(null); client.setCsrfToken(null);
    user = null; accountView = false; settingsView = false; view = 'login';
    notice = 'Your session ended. Sign in again to continue.';
    void tick().then(() => document.getElementById('handle')?.focus());
  }

  async function refreshRoot(isCurrent: () => boolean = () => true, signal?: AbortSignal): Promise<boolean> {
    const generation = sessionGeneration;
    const listingData = await listing.roots(null, undefined, signal);
    if (generation !== sessionGeneration || !isCurrent()) return false;
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
    if (restorationFailed) return false;
    if (searchQuery) { browseDirty = true; return runSearch(); }
    const generation = sessionGeneration;
    const sort = pendingFolderSort ?? folderSort;
    let folderId = currentFolder.id;
    const requestId = ++listingRequest;
    clearSelection();
    pendingMove = null; loading = true;
    listingError = '';
    try {
      if (!folderId) {
        if (!await refreshRoot(() => requestId === listingRequest) || requestId !== listingRequest) return false;
        currentFolder = folders[0]; folderPath = [currentFolder]; folderId = currentFolder.id;
      }
      const page = await listing.children(folderId, null, sort);
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return false;
      folderSort = sort;
      entriesFolderId = currentFolder.id; entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      moveNeedsRefresh = false;
      pageCursors = [null]; pageIndex = 0; nextCursor = page.next_cursor; hasMore = page.has_more;
      return true;
    }
    catch { if (generation === sessionGeneration && currentFolder.id === folderId && requestId === listingRequest) listingError = 'Could not load this folder. Try again.'; return false; }
    finally { if (generation === sessionGeneration && requestId === listingRequest) loading = false; }
  }

  async function sortFolderBy(column: 'name' | 'modified' | 'size'): Promise<void> {
    if (folderSortBlocked || searchQuery || accountView || settingsView || showTrash) return;

    const sort = nextSort(pendingFolderSort ?? folderSort, column);
    if (pendingFolderSort && sort === folderSort) {
      listingRequest++;
      folderSortAttempt = null;
      return;
    }
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    const folderId = currentFolder.id;
    const request = ++listingRequest;
    const isCurrent = () => request === listingRequest && generation === sessionGeneration &&
      navigation === navigationRequest && folderId === currentFolder.id;
    folderSortAttempt = { request, sort, failed: false };
    clearCompletedFeedback();
    pendingMove = null;

    try {
      const page = await listing.children(folderId, null, sort);
      if (!isCurrent()) return;
      if (!Array.isArray(page?.items)) throw new Error('Invalid sorted folder listing');
      const focusedRow = document.activeElement?.closest('.file-row');
      clearSelection();
      folderSort = sort;
      entriesFolderId = folderId; entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      pageCursors = [null]; pageIndex = 0; nextCursor = page.next_cursor; hasMore = page.has_more;
      folderSortAttempt = null;
      if (focusedRow) {
        await tick();
        if (isCurrent() && !focusedRow.isConnected && document.activeElement === document.body) {
          document.querySelector<HTMLElement>(`.folder-sort-head [aria-label="${column[0].toUpperCase() + column.slice(1)}"] button`)?.focus({ preventScroll: true });
        }
      }
    } catch {
      if (isCurrent()) folderSortAttempt = { request, sort, failed: true };
    }
  }

  function clearSearch(restoreBrowse = false, preserveComposition = false): void {
    clearSelection();
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    searchController?.abort(); searchController = null;
    searchRequest++;
    searchInput = ''; searchQuery = ''; searchItems = [];
    if (!preserveComposition) searchComposing = false;
    searchLoading = false; searchError = ''; searchInputError = ''; searchRetryCursor = null; searchRetryPage = 0;
    searchPageCursors = [null]; searchPageIndex = 0;
    searchNextCursor = null; searchHasMore = false;
    if (restoreBrowse && browseDirty && user) void restoreBrowsingPage();
    else browseDirty = false;
  }

  async function clearSearchFromButton(): Promise<void> {
    if (restorationPending) return;
    navigationRequest++; historyReady = true;
    clearCompletedFeedback();
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
    pendingMove = null; loading = true; listingError = '';
    try {
      const page = await listing.children(folderId, cursor, folderSort);
      if (generation !== sessionGeneration || request !== listingRequest || searchQuery || currentFolder.id !== folderId) return;
      entriesFolderId = currentFolder.id; entries = page.items; nextCursor = page.next_cursor; hasMore = page.has_more; browseDirty = false;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
    } catch (error) {
      if (generation !== sessionGeneration || request !== listingRequest || searchQuery || currentFolder.id !== folderId) return;
      if (error instanceof HttpError && (error.status === 409 || (error.status === 400 && error.code === 'invalid_cursor'))) {
        const restarted = refresh();
        const restartRequest = listingRequest;
        await restarted;
        if (generation === sessionGeneration && restartRequest === listingRequest && !searchQuery && currentFolder.id === folderId) notice = 'This folder changed. The listing has been restarted.';
      } else listingError = 'Could not load this folder. Try again.';
    } finally { if (generation === sessionGeneration && request === listingRequest) loading = false; }
  }

  function searchChanged(): void {
    if (bulkBusy || (restorationPending && !searchDuringRoute)) return;
    if (searchDuringRoute) {
      clearRouteFeedback(); restorationPending = false; presentedRoute = true;
    }
    if (loading || entriesFolderId !== currentFolder.id) browseDirty = true;
    navigationRequest++; historyReady = true; pendingMove = null; loading = false; listingRequest++;

    clearCompletedFeedback();
    clearSelection();
    if (!searchInput) { clearSearch(true, searchComposing); return; }
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
    if (searchComposing) return;
    searchTimer = setTimeout(() => { searchTimer = null; void runSearch(); }, 120);
  }

  function searchCompositionStarted(): void {
    searchComposing = true;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    searchController?.abort(); searchController = null;
    searchRequest++;
    searchItems = []; searchLoading = !!searchQuery;
  }

  function searchKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Enter' || event.isComposing || searchComposing || bulkBusy || restorationPending) return;
    event.preventDefault();
    if (!searchController) void runSearch();
  }

  async function runSearch(cursor: string | null = null, targetPage = 0, onRequest?: (request: number) => void): Promise<boolean> {
    if (!user || !searchQuery || searchInputError || searchComposing) return false;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    searchController?.abort();
    const controller = new AbortController(); searchController = controller;
    const request = ++searchRequest;
    onRequest?.(request);
    const generation = sessionGeneration;
    const query = searchQuery;
    const folderId = currentFolder.id === folders[0]?.id ? null : currentFolder.id;
    clearSelection();
    searchLoading = true; searchError = '';
    try {
      const page = await listing.search(query, folderId, cursor, controller.signal, searchSort);
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
      if (error instanceof HttpError && (error.status === 409 || (error.status === 400 && error.code === 'invalid_cursor'))) {
        if (cursor) {
          let restartRequest = request;
          const restarted = await runSearch(null, 0, value => { restartRequest = value; onRequest?.(value); });
          if (restartRequest !== searchRequest || generation !== sessionGeneration || query !== searchQuery) return false;
          notice = 'Files changed. Search results have been restarted.';
          return restarted;
        }
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
    if (!searchHasMore || !searchNextCursor || searchLoading) return;
    clearCompletedFeedback();
    const generation = sessionGeneration;
    const query = searchQuery;
    let request = searchRequest + 1;
    const page = searchPageIndex + 1;
    const isCurrent = () => generation === sessionGeneration && request === searchRequest && query === searchQuery && !accountView && !settingsView && !showTrash;
    if (await runSearch(searchNextCursor, page, value => { request = value; })) await focusPagedResults(isCurrent);
    else if (isCurrent() && searchError) await focusPagedResults(isCurrent, 'search-page-retry');
  }

  async function searchPrevious(): Promise<void> {
    if (searchPageIndex === 0 || searchLoading) return;
    clearCompletedFeedback();
    const generation = sessionGeneration;
    const query = searchQuery;
    let request = searchRequest + 1;
    const page = searchPageIndex - 1;
    const isCurrent = () => generation === sessionGeneration && request === searchRequest && query === searchQuery && !accountView && !settingsView && !showTrash;
    if (await runSearch(searchPageCursors[page], page, value => { request = value; })) await focusPagedResults(isCurrent);
    else if (isCurrent() && searchError) await focusPagedResults(isCurrent, 'search-page-retry');
  }

  async function focusPagedResults(isCurrent: () => boolean, retryId?: string): Promise<void> {
    await tick();
    if (!isCurrent()) return;
    if (retryId) {
      document.getElementById(retryId)?.focus();
      return;
    }
    (document.querySelector<HTMLElement>('.search-results .item-name, .file-drop-region .item-name') ?? document.getElementById('folder-title'))?.focus();
  }

  async function openSearchFolder(hit: SearchHit, containing = false): Promise<void> {
    const path = containing ? hit.location : [...hit.location, hit.entry as Folder];
    if (!path.length) return;
    await navigate('files', path.map((item, index) => index === 0 && !item.name ? { ...item, name: 'Files' } : item));
  }

  async function loadMore(): Promise<void> {
    if (!hasMore || !nextCursor || loading || pendingFolderSort || bulkBusy) return;
    clearCompletedFeedback();

    clearSelection();
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const cursor = nextCursor;
    const requestId = ++listingRequest;
    const isCurrent = () => generation === sessionGeneration && currentFolder.id === folderId && requestId === listingRequest && !searchQuery && !accountView && !settingsView && !showTrash;
    pendingMove = null; loading = true; listingError = '';
    try {
      const page = await listing.children(folderId, cursor, folderSort);
      if (generation !== sessionGeneration || currentFolder.id !== folderId || nextCursor !== cursor || requestId !== listingRequest) return;
      pageCursors = [...pageCursors.slice(0, pageIndex + 1), nextCursor];
      pageIndex += 1; entriesFolderId = currentFolder.id; entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      nextCursor = page.next_cursor; hasMore = page.has_more;
      loading = false;
      await focusPagedResults(isCurrent);
    } catch (error) {
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return;
      if (error instanceof HttpError && (error.status === 409 || (error.status === 400 && error.code === 'invalid_cursor'))) {
        const refreshed = refresh();
        const restartRequest = listingRequest;
        const restarted = await refreshed;
        const isRestartCurrent = () => generation === sessionGeneration && currentFolder.id === folderId && restartRequest === listingRequest && !searchQuery && !accountView && !settingsView && !showTrash;
        if (!isRestartCurrent()) return;
        notice = 'This folder changed. The listing has been restarted.';
        if (restarted) await focusPagedResults(isRestartCurrent);
        else if (listingError) await focusPagedResults(isRestartCurrent, 'folder-page-retry');
      } else {
        listingError = 'Could not load the next page. Try again.';
        loading = false;
        await focusPagedResults(isCurrent, 'folder-page-retry');
      }
    } finally { if (generation === sessionGeneration && requestId === listingRequest) loading = false; }
  }

  async function loadPrevious(): Promise<void> {
    if (pageIndex === 0 || loading || pendingFolderSort || bulkBusy) return;
    clearCompletedFeedback();

    clearSelection();
    const generation = sessionGeneration;
    const folderId = currentFolder.id;
    const requestId = ++listingRequest;
    const isCurrent = () => generation === sessionGeneration && currentFolder.id === folderId && requestId === listingRequest && !searchQuery && !accountView && !settingsView && !showTrash;
    pendingMove = null; loading = true; listingError = '';
    const target = pageIndex - 1;
    try {
      const page = await listing.children(folderId, pageCursors[target], folderSort);
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return;
      pageIndex = target; entriesFolderId = currentFolder.id; entries = page.items;
      for (const entry of page.items) knownParents.set(entry.id, entry.parent_id);
      nextCursor = page.next_cursor; hasMore = page.has_more;
      loading = false;
      await focusPagedResults(isCurrent);
    } catch (error) {
      if (generation !== sessionGeneration || currentFolder.id !== folderId || requestId !== listingRequest) return;
      if (error instanceof HttpError && (error.status === 409 || (error.status === 400 && error.code === 'invalid_cursor'))) {
        const refreshed = refresh();
        const restartRequest = listingRequest;
        const restarted = await refreshed;
        const isRestartCurrent = () => generation === sessionGeneration && currentFolder.id === folderId && restartRequest === listingRequest && !searchQuery && !accountView && !settingsView && !showTrash;
        if (!isRestartCurrent()) return;
        notice = 'This folder changed. The listing has been restarted.';
        if (restarted) await focusPagedResults(isRestartCurrent);
        else if (listingError) await focusPagedResults(isRestartCurrent, 'folder-page-retry');
      } else {
        listingError = 'Could not load the previous page. Try again.';
        loading = false;
        await focusPagedResults(isCurrent, 'folder-page-retry');
      }
    } finally { if (generation === sessionGeneration && requestId === listingRequest) loading = false; }
  }

  async function signIn(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (authBusy || oidcOpening || user || loggingOut) return;
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
      await openSession(user, generation);
    } catch {
      if (generation !== sessionGeneration) return;
      notice = authView === 'setup' ? 'Setup could not be completed. Check the authorization and account details, then try again.' : 'Sign-in could not be completed. Check the details and try again.';
      password = '';
    } finally { if (generation === sessionGeneration) authBusy = false; }
  }

  async function openSession(signedIn: User, generation: number): Promise<void> {
    if (textEdit?.ownerId !== signedIn.id) {
      textEdit = { ownerId: signedIn.id, state: { draft: null, operation: null, stale: false } };
    }
    textEditSession = new AbortController();
    recoverRemoval();
    recoverCopy();
    client.setSessionExpiredHandler(() => expireSession(generation));
    try {
      const pending = transfers.restore(signedIn.id);
      if (pending) {
        unresolvedUploadId = pending.id; unresolvedUploadName = pending.body.name;
        uploadReplacement = !!pending.body.replacement;
        uploadNotice = 'An unfinished upload is retained in this tab. Check its status to continue.';
        resumeFile = null; resumeReady = false; finishReady = false; uploadExpired = false;
      }
    } catch (error) { uploadNotice = error instanceof Error ? error.message : 'Upload recovery could not be loaded.'; }
    await restoreNavigation();
  }

  async function restoreExternalSession(): Promise<void> {
    const generation = sessionGeneration;
    const url = new URL(window.location.href);
    const outcome = url.searchParams.get('oidc');
    if (outcome === 'failed' || outcome === 'unavailable') {
      url.searchParams.delete('oidc');
      window.history.replaceState(window.history.state, '', url);
      notice = outcome === 'unavailable' ? 'Keycloak sign-in is unavailable. Try again later.'
        : 'Keycloak sign-in could not be completed. Check your account name and approval, then try again.';
    }
    void session.oidcEnabled().then((enabled) => { oidcEnabled = enabled; }).catch(() => {});
    try {
      const restored = await session.externalSession();
      if (generation !== sessionGeneration || authBusy || oidcOpening || user) return;
      client.setCsrfToken(restored.csrfToken);
      user = restored.user;
      await openSession(restored.user, ++sessionGeneration);
    } catch { /* Local sign-in is independent of the external session probe. */ }
  }

  async function startKeycloak(event?: SubmitEvent): Promise<void> {
    event?.preventDefault();
    if (oidcOpening || authBusy || !oidcEnabled || user?.external) return;
    const converting = user !== null;
    if (!converting && !/^[a-z0-9][a-z0-9._-]*$/.test(handle)) {
      notice = 'Enter the Sylux account name your administrator approved.';
      document.getElementById('handle')?.focus();
      return;
    }
    oidcOpening = true;
    notice = ''; conversionError = '';
    const approvedHandle = user?.handle ?? handle;
    const localProof = converting ? conversionPassword : undefined;
    const generation = sessionGeneration;
    password = ''; conversionPassword = '';
    try {
      if (!converting) client.setCsrfToken((await session.csrf()).csrfToken);
      const url = await session.startOidc(approvedHandle, localProof);
      if (generation === sessionGeneration) window.location.assign(url);
    } catch (error) {
      if (generation !== sessionGeneration) return;
      const unavailable = error instanceof HttpError && [404, 503].includes(error.status);
      const message = unavailable ? 'Keycloak sign-in is unavailable. Try again later.'
        : converting ? 'Could not start the switch. Check your password and approval, then try again.'
        : 'Could not start Keycloak sign-in. Check the account name and try again.';
      if (converting) conversionError = message;
      else notice = message;
      oidcOpening = false;
      await tick();
      if (generation === sessionGeneration) document.getElementById(converting ? 'conversion-password' : 'keycloak-sign-in')?.focus();
    }
  }

  function closeConversion(): void {
    if (oidcOpening) return;
    conversionOpen = false;
    conversionPassword = ''; conversionError = '';
  }

  async function retryOwnUsage(): Promise<void> {
    if (restorationPending || ownUsageRetrying) return;
    const generation = sessionGeneration;
    const button = () => document.querySelector('#usage-heading ~ .refresh-button') as HTMLElement | null;
    const trigger = button();
    const watch = watchFocus(trigger);
    const hadFocus = !!trigger && document.activeElement === trigger;
    ownUsageRetrying = true;
    const retry = loadOwnUsage();
    const request = ownUsageRequest;
    const applied = await retry;
    watch.stop();
    if (generation !== sessionGeneration || request !== ownUsageRequest) return;
    ownUsageRetrying = false;
    await tick();
    if (!applied || !hadFocus || !watch.mayFocus()) return;
    if (ownUsageState === 'ready') document.getElementById('usage-heading')?.focus();
    else button()?.focus();
  }

  async function loadOwnUsage(signal?: AbortSignal): Promise<boolean> {
    const generation = sessionGeneration;
    const request = ++ownUsageRequest;
    if (!restorationPending || !ownUsage) ownUsageState = 'loading';
    const current = () => generation === sessionGeneration && request === ownUsageRequest;
    try {
      const result = await session.usage(signal);
      if (current()) { ownUsage = result; ownUsageState = 'ready'; }
    } catch {
      if (current()) { ownUsage = null; ownUsageState = 'error'; }
    }
    return current();
  }
  async function loadAllUsage(): Promise<void> {
    const generation = sessionGeneration;
    const request = ++allUsageRequest;
    allUsageState = 'loading';
    try {
      const result = await session.accountsUsage();
      if (generation === sessionGeneration && request === allUsageRequest) { allUsage = result; allUsageState = 'ready'; }
    } catch {
      if (generation === sessionGeneration && request === allUsageRequest) { allUsage = null; allUsageState = 'error'; }
    }
  }
  async function loadAccounts(): Promise<void> {
    void loadAllUsage();
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
    event.preventDefault();
    if (accountBusy) return;
    if (accountPassword !== accountPasswordConfirmation) {
      accountPasswordError = 'Passwords do not match.';
      return;
    }
    accountPasswordError = '';
    const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.createAccount(accountHandle, accountPassword);
      if (generation !== sessionGeneration) return;
      createAccountOpen = false; accountHandle = ''; clearCreateAccountSecrets(); accountNotice = 'Account created. Share the password through a private channel.';
      await loadAccounts();
    } catch { if (generation === sessionGeneration) accountNotice = 'Account could not be created. Check the name and password, then try again.'; }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }
  function clearCreateAccountSecrets(): void {
    accountPassword = ''; accountPasswordConfirmation = ''; accountPasswordError = '';
    accountPasswordVisible = false; accountPasswordConfirmationVisible = false;
  }
  async function disableAccount(account: Account): Promise<void> {
    const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      await session.disableAccount(account.account_id);
      if (generation !== sessionGeneration) return;
      disableTarget = null; accountNotice = 'Account disabled and sessions signed out.';
      await loadAccounts();
      if (generation === sessionGeneration) { await tick(); document.getElementById('accounts-heading')?.focus(); }
    } catch (error) { if (generation === sessionGeneration) accountNotice = accountChangeError(error); }
    finally { if (generation === sessionGeneration) accountBusy = false; }
  }
  function accountChangeError(error: unknown): string {
    if (error instanceof HttpError && error.status === 409) return 'This account changed or the action would remove administrator access. Cancel, refresh accounts and try again.';
    if (error instanceof HttpError && error.status === 404) return 'This account no longer exists. Cancel and refresh accounts.';
    return 'The result could not be confirmed. Cancel, then refresh accounts before trying again.';
  }
  async function changeAccountAuthority(): Promise<void> {
    if (!authorityTarget || accountBusy) return;
    const { account, action } = authorityTarget;
    const generation = sessionGeneration; accountBusy = true; accountNotice = '';
    try {
      if (action === 'enable') await session.enableAccount(account.account_id);
      else await session.changeRole(account.account_id, account.role === 'user' ? 'administrator' : 'user');
      if (generation !== sessionGeneration) return;
      authorityTarget = null;
      accountNotice = action === 'enable' ? (account.credential_mode === 'external' ? 'Account enabled. They can sign in with Keycloak.' : 'Account enabled. They can sign in with their current password.') : 'Role changed and sessions signed out. They can sign in again.';
      await loadAccounts();
      if (generation === sessionGeneration) { await tick(); document.getElementById('accounts-heading')?.focus(); }
    } catch (error) { if (generation === sessionGeneration) accountNotice = accountChangeError(error); }
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
  function clearWorkspace(preserveTextEdit = false): void {
    textEditSession?.abort();
    textEditSession = null;
    if (textEdit) {
      const { draft, operation } = textEdit.state;
      if (!preserveTextEdit || !draft || draft.wire === draft.original) textEdit = null;
      else if (!operation?.inProgress) textEdit.state.operation = null;
    }
    share = null; shareBusy = false; directShare = null; directShareBusy = ''; settleTreeSharing(false);
    updateFolderLinks([]);
    sharedView = null; sharedLocation = { folder: '', file: undefined };
    webdavAvailable = false; webdavOpen = false; webdavTrigger = null;
    clearSearch();
    archiveBusy = false; archiveTargets = [];
    actionNotice = ''; compactUnresolvedUpload = false;
    workspaceRefreshing = false; workspaceRefreshRequest++; folderRetryBusy = false; moveCloseRefreshBusy = false; entriesFolderId = '';
    bulkAction = null; bulkTargets = []; bulkReport = null; bulkBusy = false; bulkNeedsRefresh = false; bulkError = '';
    clearRouteFeedback(); presentedRoute = false;
    historyReady = false; navigationRequest++; restorationPending = false; restorationFailed = false; restorationUnavailable = false; deferredNavigation = false; deferredAccountOperation = false; deferredInstanceOperation = false; deferredShareOperation = false;
    window.history.replaceState(null, '', '#files');
    controller?.abort(); controller = null;
    publicUploadCheck?.abort(); publicUploadCheck = null;
    dismissReplacement(); uploadReplacement = false;
    uploadBatchCanceled = true; uploadBatchBusy = false; uploadBatchStatus = '';
    uploadResults = []; uploadResultsExpanded = false; uploadReportVisible = false; previousUploadReports = []; uploadNotice = ''; unresolvedUploadResult = null;
    transfers.invalidate(); transfers = new TransferApi(client, mutations);
    resumeReady = false; finishReady = false; uploadExpired = false; transferWaiting = ''; resumeFile = null;
    transferName = ''; progress = 0; transferPhase = ''; transferBusy = false; uploadCancelable = false;
    uploadId = ''; unresolvedUploadId = ''; unresolvedUploadName = ''; unresolvedCancel = false; cancelPending = false; notice = '';
    folderSort = 'name_asc'; searchSort = 'name_asc'; trashSort = 'name_asc'; accountSort = 'name_asc';
    listingRequest++; currentFolder = emptyRoot; folders = []; entries = []; folderPath = [emptyRoot];
    librariesView = false; libraries = []; librariesError = '';
    pageCursors = [null]; pageIndex = 0; nextCursor = null; hasMore = false; loading = false; listingError = '';
    folderName = ''; folderBusy = false; createOpen = false; folderError = ''; previewEntry = null; previewTrigger = null;
    draggedEntry = null; dropTarget = ''; dropPending = null; dropError = ''; moveNeedsRefresh = false; knownParents.clear();
    renameId = ''; renameName = ''; renameError = ''; renameBusy = false;
    closeMove(); moveBusy = false;
    pendingCopy = null; copyRecovery = null; copyRecoveryMessage = ''; copyStorageFailed = false;
    removalSelection = []; removalPreview = null; removalPreparing = null; removalSubmitting = false; removalPending = null; removalChecking = false; removalError = '';
    trashRequest++; deletedItems = []; trashPageIndex = 0; trashPageCursors = [null]; trashNextCursor = null; trashHasMore = false; trashRetry = null; trashPaging = null; trashNotice = ''; showTrash = false; trashError = ''; trashTarget = null; trashBusy = false; trashLoading = false; restoringId = ''; restoringName = ''; namespaceUnknown = null; namespaceInspecting = false;
  }
  function clearAccountForms(): void {
    changePasswordOpen = false; conversionOpen = false; conversionPassword = ''; conversionError = ''; oidcOpening = false; createAccountOpen = false; disableTarget = null; authorityTarget = null; instanceNameBusy = false; appearanceBusy = false; appearanceError = ''; instanceNotice = '';
    accountHandle = ''; clearCreateAccountSecrets(); currentPassword = ''; newPassword = '';
    resetId = ''; resetPassword = ''; accounts = []; accountNotice = ''; accountBusy = false; accountsLoading = false; accountsError = ''; accountsRequest++;
    ownUsage = null; ownUsageState = 'loading'; ownUsageRequest++; ownUsageRetrying = false; allUsage = null; allUsageState = 'loading'; allUsageRequest++;
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
    transfers.clearRecovery();
    loggingOut = true; client.setSessionExpiredHandler(null); sessionGeneration++; clearAccountForms(); clearWorkspace(); accountView = false; settingsView = false; user = null;
    let logoutFailed = false;
    try { await session.logout(); } catch { logoutFailed = true; }
    sessionGeneration++; clearAccountForms(); clearWorkspace();
    if (logoutFailed) notice = 'Sign-out could not be confirmed. Check your connection.';
    client.setCsrfToken(null);
    loggingOut = false;
    await tick();
    document.getElementById('handle')?.focus();
  }

  function beginCreate(trigger: HTMLElement): void {
    if (restorationPending || restorationFailed || namespaceUnknown) return;
    createTrigger = trigger; folderName = ''; folderError = ''; createOpen = true;
  }

  async function createFolder(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (namespaceUnknown) { folderError = 'Close this dialog and check Files and Trash before continuing.'; return; }
    if (restorationPending || restorationFailed || folderBusy || createAudience.pending || !currentFolder.id) return;
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
    if (namespaceUnknown) { renameError = 'Close this dialog and check Files and Trash before continuing.'; return; }
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
    moveRequest++; moveCloseRefreshBusy = false; moveTarget = null; destinationAction = 'move'; movePath = []; moveFolders = []; moveCursor = null; moveLoading = false; moveDestinationReady = false; moveError = '';
  }
  async function closeMoveAndRefresh(): Promise<void> {
    if (moveCloseRefreshBusy) return;
    const generation = sessionGeneration;
    const request = moveRequest;
    moveCloseRefreshBusy = true;
    try {
      const refreshed = await refresh();
      if (generation !== sessionGeneration || request !== moveRequest) return;
      if (refreshed) closeMove();
      else moveError = 'Could not refresh files. Try again before moving this item.';
    } finally { if (generation === sessionGeneration && request === moveRequest) moveCloseRefreshBusy = false; }
  }
  async function loadMoveFolder(path: Folder[], cursor: string | null = null): Promise<void> {
    const destination = path.at(-1);
    if (!destination || (!moveTarget && bulkAction !== 'move')) return;
    if (destinationAction === 'move' && path.some(folder => excludedMoveFolders.has(folder.id))) return;
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
  async function navigateMoveFolder(path: Folder[]): Promise<void> {
    const loading = loadMoveFolder(path);
    if (destinationAction === 'move') {
      await tick();
      moveDialog?.querySelector<HTMLElement>('[aria-current="location"]')?.focus();
    }
    await loading;
  }
  async function loadMoveLibraries(): Promise<void> {
    const request = ++moveRequest;
    const generation = sessionGeneration;
    moveLoading = true; moveDestinationReady = false; moveError = '';
    movePath = []; moveFolders = []; moveCursor = null;
    try {
      const { items } = await listing.libraries();
      if (request !== moveRequest || generation !== sessionGeneration) return;
      moveFolders = items;
      for (const folder of items) knownParents.set(folder.id, null);
    } catch {
      if (request === moveRequest && generation === sessionGeneration) moveError = 'Could not load your libraries. Try again.';
    } finally {
      if (request === moveRequest) moveLoading = false;
    }
  }
  async function beginDestination(entry: Entry, trigger: HTMLElement, action: 'move' | 'copy'): Promise<void> {
    if (bulkBusy || bulkNeedsRefresh || dropPending || moveNeedsRefresh || namespaceUnknown) return;
    if (action === 'copy' && pendingCopy) return;
    closeMove(); moveTarget = entry; moveTrigger = trigger; destinationAction = action;
    const root = folderPath[0];
    if (root) await loadMoveFolder([root]);
    else moveError = 'Your storage is unavailable. Close this dialog and refresh your files.';
  }

  type TreeSharingPrompt = {
    action: 'move' | 'restore'; item: Entry; destination?: string; audience: SharingConfirmation; changed: boolean;
  } | { action: 'upload'; item: Folder; audienceKnown: boolean; audience?: undefined; changed?: undefined; destination?: undefined };
  let treeSharingPrompt = $state<TreeSharingPrompt | null>(null);
  let treeSharingAnswer: ((answer: boolean) => void) | undefined;
  let treeSharingFocus: HTMLElement | null = null;

  function settleTreeSharing(answer: boolean): void {
    treeSharingPrompt = null;
    const resolve = treeSharingAnswer;
    treeSharingAnswer = undefined;
    resolve?.(answer);
  }

  function askAboutSharing(prompt: TreeSharingPrompt): Promise<boolean> {
    treeSharingFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    treeSharingPrompt = prompt;
    return new Promise(resolve => { treeSharingAnswer = resolve; });
  }

  async function withTreeSharingConfirmation(
    item: Entry, action: 'move' | 'restore', submit: (token?: string) => Promise<unknown>, destination?: string,
  ): Promise<boolean> {
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    let token: string | undefined;
    while (generation === sessionGeneration && navigation === navigationRequest) {
      try {
        await submit(token);
        return true;
      } catch (error) {
        const audience = sharingConfirmation(error);
        if (!audience) throw error;
        if (generation !== sessionGeneration || navigation !== navigationRequest) return false;
        const approved = await askAboutSharing({ action, item, destination, audience, changed: token !== undefined });
        if (!approved) return false;
        token = audience.token;
      }
    }
    return false;
  }

  async function moveEntry(): Promise<void> {
    const target = moveTarget;
    const destination = movePath.at(-1);
    if (namespaceUnknown) { moveError = 'Close this dialog and check Files and Trash before continuing.'; return; }
    if (!target || !destination || moveBusy || !canConfirmMove) return;
    const generation = sessionGeneration;
    moveBusy = true; moveError = '';
    try {
      if (!await withTreeSharingConfirmation(target, 'move', token => listing.move(target.id, destination.id, target.etag, token), destination.name)) {
        if (generation === sessionGeneration) moveBusy = false;
        return;
      }
      if (generation === sessionGeneration) knownParents.set(target.id, destination.id);
    } catch (error) {
      if (generation !== sessionGeneration) return;
      moveError = moveFailure(error);
      if (moveOutcomeUncertain(error)) moveNeedsRefresh = true;
      moveBusy = false;
      return;
    }
    if (generation !== sessionGeneration) return;
    // Refresh can replace the source row in both folder and search views.
    moveTrigger = null;
    closeMove(); moveBusy = false;
    clearCompletedFeedback(); actionNotice = `Moved to ${destination.name}`;
    const completion = feedbackRevision;
    try {
      await refreshRoot();
      if (generation === sessionGeneration) await refresh();
    } catch { if (generation === sessionGeneration && feedbackRevision === completion) actionNotice = `Moved to ${destination.name}. Refresh your files to update the folder list.`; }
  }

  function saveCopy(pending: PendingCopy | null): void {
    pendingCopy = pending;
    if (!user) return;
    const key = `sylux-copy:${user.id}`;
    try {
      if (pending) sessionStorage.setItem(key, JSON.stringify(pending));
      else sessionStorage.removeItem(key);
      copyStorageFailed = false;
    } catch { copyStorageFailed = !!pending; }
  }

  function recoverCopy(): void {
    if (!user) return;
    try {
      const saved = sessionStorage.getItem(`sylux-copy:${user.id}`);
      if (!saved) return;
      const pending = JSON.parse(saved) as PendingCopy;
      if (typeof pending?.ticket !== 'string' || !pending.ticket || pending.ticket.length > 1024 ||
          typeof pending.destination !== 'string' || !pending.destination) throw new Error('Invalid copy recovery');
      pendingCopy = pending;
      copyRecoveryMessage = 'Copy outcome unconfirmed.';
    } catch { console.warn('Copy recovery could not be read.'); }
  }

  function copyFailure(error: unknown): string {
    if (error instanceof HttpError) {
      if (error.code === 'name_conflict') return 'An item with this name is already there. Choose another folder, or rename your item.';
      if (error.code === 'invalid_destination') return 'A folder cannot be copied into itself or its children. Choose another folder.';
      if (error.code === 'copy_too_large') return 'This folder is too large to copy. Copy a smaller folder instead.';
      if (error.code === 'invalid_copy_ticket') return 'This copy request is invalid. Check the destination for an earlier copy before trying again.';
      if (error.code === 'depth_limit') return 'Choose a destination closer to Files. This copy would nest folders too deeply.';
      if (error.code === 'copy_ticket_expired') return 'The copy request expired. Choose a destination again.';
      if (error.status === 404 || error.status === 412) return 'The item or destination changed. Refresh Files before trying again.';
    }
    return 'Could not copy this item. Try again.';
  }

  async function finishCopy(pending: PendingCopy, feedback: number): Promise<void> {
    saveCopy(null);
    copyRecoveryMessage = '';
    const generation = sessionGeneration;
    if (feedback === feedbackRevision) {
      clearCompletedFeedback();
      actionNotice = `Copied to ${pending.destination}`;
    }
    if (!accountView && !settingsView && !showTrash) {
      const completion = feedbackRevision;
      const refreshed = await refresh();
      if (!refreshed && generation === sessionGeneration && completion === feedbackRevision &&
          actionNotice === `Copied to ${pending.destination}`) {
        actionNotice += '. Refresh Files to see the copy.';
      }
    } else {
      entriesFolderId = '';
    }
  }

  function copyMayBeUnconfirmed(error: unknown): boolean {
    return !(error instanceof HttpError && error.code === 'invalid_copy_ticket') &&
      (moveOutcomeUncertain(error) || (error instanceof HttpError && [400, 401, 403].includes(error.status)));
  }

  async function copyEntry(): Promise<void> {
    const target = moveTarget, destination = movePath.at(-1);
    if (!target || !destination || pendingCopy || moveBusy || moveLoading || !moveDestinationReady || copyAudience.pending ||
        namespaceUnknown || destination.id === target.parent_id || movePath.some(folder => folder.id === target.id)) return;
    const generation = sessionGeneration, feedback = feedbackRevision;
    moveBusy = true; moveError = '';
    let pending: PendingCopy | null = null;
    try {
      const ticket = await listing.prepareCopy(target.id, destination.id, target.etag);
      if (generation !== sessionGeneration) return;
      pending = { ticket, destination: destination.name };
      saveCopy(pending);
      const state = await listing.copyStatus(ticket, false);
      if (generation !== sessionGeneration) return;
      closeMove();
      if (state === 'committed') await finishCopy(pending, feedback);
      else copyRecoveryMessage = 'Copy outcome unconfirmed.';
    } catch (error) {
      if (generation !== sessionGeneration) return;
      if (pending && copyMayBeUnconfirmed(error)) {
        closeMove();
        copyRecoveryMessage = 'Copy outcome unconfirmed.';
        await reconcileCopy('status', true, feedback);
      } else {
        if (pending) saveCopy(null);
        moveError = copyFailure(error);
      }
    } finally { if (generation === sessionGeneration) moveBusy = false; }
  }

  async function reconcileCopy(mode: 'status' | 'retry', automatic = false, feedback = feedbackRevision): Promise<void> {
    if (!pendingCopy || copyRecovery || (moveBusy && !automatic)) return;
    const pending = pendingCopy, generation = sessionGeneration;
    copyRecovery = mode;
    try {
      const state = await listing.copyStatus(pending.ticket, mode === 'status');
      if (generation !== sessionGeneration) return;
      if (state === 'committed') await finishCopy(pending, feedback);
      else copyRecoveryMessage = 'Copy outcome unconfirmed. Retry the same copy to resolve it.';
    } catch (error) {
      if (generation !== sessionGeneration) return;
      if (error instanceof HttpError && error.code === 'invalid_copy_ticket') {
        saveCopy(null);
        copyRecoveryMessage = copyFailure(error);
      } else if (mode === 'retry' && !copyMayBeUnconfirmed(error) && !(error instanceof HttpError && error.code === 'copy_not_committed')) {
        saveCopy(null);
        if (feedback === feedbackRevision) copyRecoveryMessage = copyFailure(error);
      } else {
        copyRecoveryMessage = 'Copy outcome unconfirmed. Check your connection and try again.';
      }
    } finally { if (generation === sessionGeneration) copyRecovery = null; }
  }

  function trashRoots(targets: Entry[]): Entry[] {
    const selected = new Set(targets.map((entry) => entry.id));
    return targets.filter((entry) => {
      let parent = entry.parent_id;
      const seen = new Set<string>();
      while (parent && !seen.has(parent)) {
        if (selected.has(parent)) return false;
        seen.add(parent); parent = knownParents.get(parent) ?? null;
      }
      return true;
    });
  }

  async function beginBulk(action: BulkAction, trigger: HTMLElement, targets = selectedEntries, retry = false): Promise<void> {
    if (!targets.length || fileMutationBusy || loading || searchLoading || searchError || searchInputError) return;
    closeMove();
    bulkTargets = (action === 'trash' ? trashRoots(targets) : targets).map((entry) => ({ ...entry }));
    bulkAction = action; bulkRetry = retry; bulkTrigger = trigger; bulkError = '';
    if (action === 'move') {
      const root = folderPath[0];
      if (root) await loadMoveFolder([root]);
      else moveError = 'Your storage is unavailable. Close this dialog and refresh your files.';
    }
  }

  function closeBulk(): void {
    if (bulkBusy) return;
    if (bulkAction === 'move') closeMove();
    bulkAction = null; bulkTargets = [];
  }

  async function runBulk(): Promise<void> {
    const action = bulkAction;
    const destination = action === 'move' ? movePath.at(-1) ?? null : null;
    if (!action || !bulkTargets.length || fileMutationBusy || loading || searchLoading ||
      (action === 'move' && (!destination || !canConfirmMove))) return;
    const generation = sessionGeneration;
    const targets = bulkTargets.map((entry) => ({ ...entry }));
    const report: BulkReport = bulkRetry && bulkReport ? bulkReport : {
      action, destination, results: targets.map((entry) => ({ entry, location: searchItems.find((hit) => hit.entry.id === entry.id) ? searchLocation(searchItems.find((hit) => hit.entry.id === entry.id)!) : folderPath.map((folder) => folder.name).join(' / '), status: 'pending', message: 'Waiting.' })),
    };
    report.destination = destination;
    if (!bulkRetry) clearCompletedFeedback();
    bulkReport = report;
    bulkBusy = true; bulkError = ''; clearSelection();
    for (const target of targets) {
      if (generation !== sessionGeneration) return;
      const result = bulkReport!.results.find((item) => item.entry.id === target.id)!;
      result.status = 'pending'; result.message = action === 'move' ? 'Moving…' : 'Moving to trash…';
      try {
        if (action === 'move') {
          const folder = destination!;
          if (!await withTreeSharingConfirmation(target, action, token => listing.move(target.id, folder.id, target.etag, token), folder.name)) {
            if (generation !== sessionGeneration) return;
            for (const remaining of bulkReport!.results) {
              if (remaining.status === 'pending') {
                remaining.status = 'failed';
                remaining.message = 'Kept in place.';
              }
            }
            break;
          }
        } else await listing.trash(target.id, target.etag);
        if (generation !== sessionGeneration) return;
        result.status = 'succeeded'; result.message = action === 'move' ? `Moved to ${destination!.name}.` : 'Moved to trash.';
      } catch (error) {
        if (generation !== sessionGeneration) return;
        result.status = moveOutcomeUncertain(error) ? 'uncertain' : 'failed';
        result.message = result.status === 'uncertain' ? 'Outcome unconfirmed. Refresh before retrying.' :
          action === 'move' ? moveFailure(error) : error instanceof HttpError && error.status === 403 ? 'You do not have permission to move this item to trash.' : error instanceof HttpError && error.status === 409 ? 'This item could not be moved to trash. Refresh before retrying.' :
          error instanceof HttpError && error.status === 412 ? 'This item changed. Refresh before retrying.' : 'Could not move to trash. Refresh before retrying.';
        if (result.status === 'uncertain') bulkNeedsRefresh = true;
        if (error instanceof HttpError && error.status === 401) break;
      }
    }
    if (generation !== sessionGeneration) return;
    if (action === 'move') closeMove();
    bulkAction = null; bulkTargets = [];
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
      const source: Entry[] = [];
      for (const parentId of new Set(report.results.map((result) => result.entry.parent_id))) {
        if (parentId) source.push(...await folderEntries(parentId, generation));
      }
      if (generation !== sessionGeneration) return false;
      const destination = report.action === 'move' && report.destination ? await folderEntries(report.destination.id, generation) : [];
      if (generation !== sessionGeneration) return false;
      const deleted = report.action === 'trash' ? await listing.deletedTargets(report.results.map(result => result.entry.id), () => generation === sessionGeneration) : [];
      if (generation !== sessionGeneration) return false;
      for (const result of report.results) {
        if (result.status === 'succeeded') continue;
        const live = source.find((entry) => entry.id === result.entry.id);
        const completed = destination.some((entry) => entry.id === result.entry.id) || deleted.some((entry) => entry.id === result.entry.id);
        if (completed) { result.status = 'succeeded'; result.message = report.action === 'move' ? `Moved to ${report.destination!.name}.` : 'Moved to trash.'; }
        else if (live && live.etag === result.entry.etag) { result.status = 'failed'; result.message = 'Still in the original folder. Ready to retry.'; }
        else if (live) { result.status = 'changed'; result.message = 'This item changed. Select it again to act on its current version.'; }
        else { result.status = 'uncertain'; result.message = 'Outcome could not be confirmed. Item is no longer in the original folder. Check its location before selecting it again.'; }
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
    const targets: Entry[] = [];
    for (const result of bulkReport.results) {
      if (result.status !== 'failed') continue;
      const entry = activeEntries.find((entry) => entry.id === result.entry.id && entry.etag === result.entry.etag);
      if (entry) targets.push(entry);
      else {
        result.status = 'changed';
        result.message = 'This version is no longer on the loaded page. Find and select the item again to act on it.';
      }
    }
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
      if (error.code === 'sharing_audience_changed') return 'Sharing changed. Review the destination and try again.';
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
      if (!await withTreeSharingConfirmation(target, 'move', token => listing.move(target.id, destination.id, target.etag, token), destination.name)) return;
      if (generation === sessionGeneration) knownParents.set(target.id, destination.id);
      if (generation !== sessionGeneration) return;
      clearCompletedFeedback(); actionNotice = `Moved to ${destination.name}`;
      const completion = feedbackRevision;
      try { await refreshRoot(); if (generation === sessionGeneration) await refresh(); }
      catch { if (generation === sessionGeneration && feedbackRevision === completion) actionNotice += '. Refresh your files to update the folder list.'; }
    } catch (error) { if (generation === sessionGeneration) { dropError = moveFailure(error); if (moveOutcomeUncertain(error)) moveNeedsRefresh = true; } }
    finally { if (generation === sessionGeneration) dropPending = null; }
  }

  function isDesktopDrag(event: DragEvent): boolean {
    return !!event.dataTransfer?.types.includes('Files') && !event.dataTransfer.types.includes(dragType);
  }

  function canUploadDrop(): boolean {
    return !!user && filesView && !searchQuery && !!currentFolder.id && !fileMutationBusy && !unresolvedUploadId &&
      !replacementPrompt && !createOpen && !renameId && !moveTarget && !trashTarget && !share && !directShare;
  }

  function acceptDesktopDrop(event: DragEvent, destination: Folder): void {
    event.preventDefault();
    event.stopPropagation(); dropTarget = '';
    if (!canUploadDrop() || !event.dataTransfer) return;
    try {
      const read = snapshotDesktopDrop(event.dataTransfer);
      void runUploadBatch(read, destination);
    } catch (error) {
      retainUploadReport();
      uploadBatchDestination = destination.name; uploadNotice = '';
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
    publicUploadCheck?.abort();
    dismissReplacement();
    if (treeSharingPrompt?.action === 'upload') settleTreeSharing(false);
  }

  async function freshUploadFolder(folder: Folder): Promise<Folder> {
    const path = await listing.path(folder.id);
    const descriptor = path.items.at(-1);
    if (descriptor?.id === folder.id) return descriptor;
    throw new Error('The upload destination is no longer available.');
  }

  function uploadResultLabel(result: UploadResult, busy = uploadBatchBusy): string {
    if (result.status === 'waiting') return busy ? 'Waiting' : 'Not attempted';
    return { active: result.kind === 'folder' ? 'Creating folder' : 'Uploading', saved: 'Saved', created: 'Folder created', failed: 'Failed', unknown: 'Outcome unknown', canceled: 'Canceled' }[result.status];
  }

  function resolveUploadResult(status: 'saved' | 'failed' | 'canceled'): void {
    if (unresolvedUploadResult === null || !uploadResults[unresolvedUploadResult]) return;
    uploadResults[unresolvedUploadResult].status = status;
    unresolvedUploadResult = null;
    uploadBatchStatus = 'Upload stopped.';
  }

  async function runUploadBatch(read: (canceled: () => boolean) => Promise<UploadItem[]>, destination: Folder): Promise<void> {
    if (restorationPending || restorationFailed || namespaceUnknown || uploadBatchBusy || transferBusy || unresolvedUploadId) return;
    const generation = sessionGeneration;
    clearCompletedFeedback();
    retainUploadReport();
    uploadNotice = ''; actionNotice = ''; compactUnresolvedUpload = false;
    uploadReportVisible = true;
    uploadBatchDestination = destination.name; uploadBatchStage = 'Preparing'; uploadBatchCurrentIndex = -1;
    uploadBatchBusy = true; uploadBatchCanceled = false; uploadBatchStatus = 'Reading files and folders…'; notice = '';
    uploadResults = []; uploadResultsExpanded = false; unresolvedUploadResult = null;
    let saved = 0; let created = 0; let complete = false;
    let currentPath = '';
    let currentIndex = -1;
    let creatingFolder = false;
    try {
      const navigation = navigationRequest;
      const check = new AbortController();
      publicUploadCheck = check;
      const publiclyShared = await publicFolderLinkStatus(client, destination.id, check.signal).catch(() => null);
      if (publicUploadCheck === check) publicUploadCheck = null;
      if (generation !== sessionGeneration) return;
      if (uploadBatchCanceled) { uploadBatchStatus = 'Upload canceled.'; return; }
      if (publiclyShared !== false && (navigation !== navigationRequest || !await askAboutSharing({ action: 'upload', item: destination, audienceKnown: publiclyShared === true }))) {
        if (generation === sessionGeneration) {
          uploadBatchCanceled = true;
          uploadBatchStatus = 'Upload canceled.';
        }
        return;
      }
      const items = await read(() => uploadBatchCanceled || generation !== sessionGeneration);
      if (generation !== sessionGeneration) return;
      if (!items.length) { uploadBatchStatus = 'No files or folders were selected.'; return; }
      uploadResults = items.map((item) => ({ path: item.path.join('/'), kind: item.kind, size: item.kind === 'file' ? item.file.size : 0, sent: 0, sentComplete: false, status: 'waiting' }));
      const destinations = new Map<string, Folder>([['[]', destination]]);
      for (const [index, item] of items.entries()) {
        if (namespaceUnknown) { uploadBatchStatus = 'Upload stopped. Check Files and Trash before adding more items.'; break; }
        if (uploadBatchCanceled || generation !== sessionGeneration) break;
        currentIndex = index; uploadBatchCurrentIndex = index;
        uploadResults[index].status = 'active';
        uploadBatchStage = item.kind === 'folder' ? 'Creating folder' : 'Preparing';
        currentPath = item.path.join('/');
        const remaining = uploadResults.filter((result) => result.kind === 'file' && result.status === 'waiting').length;
        uploadBatchStatus = `Adding ${currentPath} to ${destination.name}… ${remaining} ${remaining === 1 ? 'file' : 'files'} left after this item.`;
        const parent = destinations.get(JSON.stringify(item.path.slice(0, -1)));
        if (!parent) throw new Error('The destination for this item could not be found.');
        if (item.kind === 'folder') {
          uploadResults[index].parent = { ...parent };
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
            uploadBatchStage = 'Waiting for replacement decision';
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
      uploadBatchStage = 'Refreshing files';
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
            if (uploadBatchCanceled) { await tick(); restoreFocus(document.getElementById('choose-file'), accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : 'folder-title'); }
          }
        }
      }
    }
  }

  function openPreview(entry: Entry, trigger: HTMLElement): void { previewTrigger = trigger; previewEntry = entry; }
  function closePreview(): void {
    const trigger = previewTrigger;
    const generation = sessionGeneration;
    const request = navigationRequest;
    const fallback = accountView ? 'account-title' : settingsView ? 'settings-title' : showTrash ? 'trash-heading' : 'folder-title';
    previewEntry = null; previewTrigger = null;
    void tick().then(() => { if (generation === sessionGeneration && request === navigationRequest) restoreFocus(trigger, fallback); });
  }
  async function textEditSaved(entry: Entry): Promise<void> {
    const visible = previewEntry?.id === entry.id;
    if (visible) closePreview();
    const generation = sessionGeneration;
    const feedback = feedbackRevision;
    if (searchQuery) browseDirty = true;
    if (!retainingRoute && !accountView && !settingsView && !showTrash) {
      const refreshed = await refresh();
      if (generation === sessionGeneration && feedback === feedbackRevision) {
        actionNotice = refreshed ? 'Changes saved.' : 'Changes saved. Refresh files to see them.';
        await tick();
        if (visible && generation === sessionGeneration && feedback === feedbackRevision && document.activeElement === document.body) {
          restoreFocus(null);
        }
      }
    } else if (generation === sessionGeneration && feedback === feedbackRevision) actionNotice = 'Changes saved.';
  }
  let share = $state<{ entry: Entry | null; trigger: HTMLElement } | null>(null);
  let directShare = $state<{ folder: Folder; trigger: HTMLElement } | null>(null);
  let directShareBusy = $state('');
  let webdavAvailable = $state(false);
  let webdavOpen = $state(false);
  let webdavTrigger: HTMLElement | null = null;
  let shareBusy = $state(false);

  let ownedFolderLinks = $state<PublicLink[]>([]);
  let publicLinksRefresh = $state(0);
  let publicLinksError = $state(false);
  let folderLinkClock = $state(Date.now());
  const activeFolderLinks = $derived(ownedFolderLinks.filter(link => link.expires_at * 1000 > folderLinkClock));
  const publicFolderIds = $derived(new Set(activeFolderLinks.map(link => link.entry_id)));

  function updateFolderLinks(links: PublicLink[]): void {
    ownedFolderLinks = links.filter(link => link.kind === 'folder');
    publicLinksError = false;
    folderLinkClock = Date.now();
  }

  $effect(() => {
    const scope = librariesView ? 'libraries' : filesView ? currentFolder.id : '';
    const generation = sessionGeneration;
    const refresh = publicLinksRefresh;
    if (!user || !scope || restorationPending) return;
    const controller = new AbortController();
    const isCurrent = () => !controller.signal.aborted && generation === sessionGeneration && refresh === publicLinksRefresh;
    void client.request<PublicLink[]>('/share-links', { signal: controller.signal }).then(links => {
      if (isCurrent()) updateFolderLinks(links);
    }).catch(() => {
      if (isCurrent()) publicLinksError = true;
    });
    return () => controller.abort();
  });

  $effect(() => {
    if (!activeFolderLinks.length) return;
    const nextExpiry = Math.min(...activeFolderLinks.map(link => link.expires_at * 1000));
    const timer = window.setTimeout(() => { folderLinkClock = Date.now(); },
      Math.min(Math.max(0, nextExpiry - Date.now()), 2_147_483_647));
    return () => window.clearTimeout(timer);
  });

  function folderLinksChanged(links?: PublicLink[]): void {
    if (links) updateFolderLinks(links);
    publicLinksRefresh++;
  }

  function closeShare(): void {
    const trigger = share?.trigger ?? null;
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    share = null;
    void tick().then(() => {
      if (generation === sessionGeneration && navigation === navigationRequest) {
        restoreFocus(trigger, accountView ? 'account-title' : librariesView ? 'libraries-title' : 'folder-title');
      }
    });
  }

  function closeDirectShare(): void {
    const trigger = directShare?.trigger ?? null;
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    directShare = null;
    void tick().then(() => {
      if (generation === sessionGeneration && navigation === navigationRequest) {
        restoreFocus(trigger, librariesView ? 'libraries-title' : 'folder-title');
      }
    });
  }

  function entryAction(action: EntryAction, entry: Entry, trigger: HTMLElement): void {
    if (bulkBusy || ((bulkNeedsRefresh || namespaceUnknown) && action !== 'download')) return;
    if (action === 'download') void (entry.kind === 'folder' ? downloadArchive([entry]) : download(entry));
    else if (action === 'share') {
      if (entry.kind === 'folder') directShare = { folder: entry, trigger };
      else share = { entry, trigger };
    }
    else if (action === 'public-link') share = { entry, trigger };
    else if (action === 'rename') beginRename(entry, trigger);
    else if (action === 'move' || action === 'copy') void beginDestination(entry, trigger, action);
    else { trashError = ''; trashTarget = entry; trashTrigger = trigger; }
  }

  function watchFocus(trigger: HTMLElement | null): { mayFocus: () => boolean; stop: () => void } {
    let moved = false;
    const changed = (event: FocusEvent) => { if (event.target !== trigger && event.target !== document.body) moved = true; };
    document.addEventListener('focusin', changed);
    return { mayFocus: () => !moved, stop: () => document.removeEventListener('focusin', changed) };
  }

  async function loadTrash(cursor: string | null = null, targetPage = 0, focus = false): Promise<boolean> {
    if (!user || !showTrash) return false;
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    const request = ++trashRequest;
    const isCurrent = () => request === trashRequest && generation === sessionGeneration && navigation === navigationRequest && showTrash;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const watched = watchFocus(trigger);
    trashError = ''; trashNotice = ''; trashLoading = true;
    let loaded = false;
    try {
      const result = await listing.deleted(cursor, trashSort);
      if (!isCurrent()) return false;
      if (result.next_cursor && trashPageCursors.slice(0, targetPage + 1).includes(result.next_cursor)) throw new Error('Trash pagination repeated a cursor');
      if (targetPage > trashPageIndex && deletedItems.length && result.items.length && compareDeleted(deletedItems.at(-1)!, result.items[0], trashSort) >= 0) throw new Error('Trash pagination did not advance');
      deletedItems = result.items; removalSelection = [];
      trashPageIndex = targetPage;
      trashPageCursors = targetPage === 0 ? [null] : [...trashPageCursors.slice(0, targetPage), cursor];
      trashNextCursor = result.next_cursor; trashHasMore = result.has_more; trashRetry = null;
      loaded = true;
    } catch (error) {
      if (!isCurrent()) return false;
      if (cursor && error instanceof HttpError && (error.status === 400 || error.status === 409)) {
        deletedItems = []; trashPageIndex = 0; trashPageCursors = [null]; trashNextCursor = null; trashHasMore = false; trashRetry = null;
        const restarting = loadTrash(null, 0, focus && watched.mayFocus());
        const restartRequest = trashRequest;
        const restarted = await restarting;
        if (restartRequest === trashRequest && generation === sessionGeneration && navigation === navigationRequest && showTrash) {
          if (restarted) trashNotice = 'Trash changed. The listing has been restarted.';
          else trashError = 'Trash changed. Could not reload it. Try again.';
        }
        return restarted;
      }
      trashRetry = { cursor, page: targetPage };
      trashError = 'Could not load trash. Try again.';
    } finally {
      if (isCurrent()) {
        trashLoading = false; trashPaging = null;
        await tick();
        if (focus && watched.mayFocus() && isCurrent()) {
          (loaded ? document.querySelector<HTMLElement>('.trash-row button:not(:disabled)') ?? document.getElementById('trash-heading') : document.getElementById('trash-page-retry'))?.focus();
        }
      }
      watched.stop();
    }
    return loaded;
  }

  async function pageTrash(direction: 'next' | 'previous' | 'retry'): Promise<void> {
    if (trashLoading || trashBusy) return;
    const target = direction === 'retry' ? trashRetry : direction === 'next' && trashHasMore && trashNextCursor ? { cursor: trashNextCursor, page: trashPageIndex + 1 } : direction === 'previous' && trashPageIndex > 0 ? { cursor: trashPageCursors[trashPageIndex - 1], page: trashPageIndex - 1 } : null;
    if (!target) return;
    clearCompletedFeedback();
    trashPaging = direction;
    await loadTrash(target.cursor, target.page, true);
  }
  async function openTrash(): Promise<void> {
    await navigate('trash');
  }
  function rememberNamespaceUnknown(entry: Entry, action: 'trash' | 'restore'): void {
    clearCompletedFeedback();
    namespaceUnknown = { entry: { ...entry }, action, inspected: false, message: 'Outcome unconfirmed. Check Files and Trash before acting again.' };
    trashTarget = null; trashError = '';
  }

  async function inspectNamespace(event: MouseEvent): Promise<void> {
    const pending = namespaceUnknown;
    if (!pending || namespaceInspecting) return;
    const generation = sessionGeneration;
    const navigation = navigationRequest;
    const trigger = event.currentTarget as HTMLElement;
    let foundLocation: 'Files' | 'Trash' | null = null;
    namespaceInspecting = true;
    try {
      const deleted = await listing.deletedTargets([pending.entry.id], () => generation === sessionGeneration && namespaceUnknown === pending);
      let source: Entry[] = [];
      let parentUnavailable = false;
      try { if (pending.entry.parent_id) source = await folderEntries(pending.entry.parent_id, generation); }
      catch (error) { if (error instanceof HttpError && error.status === 404) parentUnavailable = true; else throw error; }
      if (generation !== sessionGeneration || namespaceUnknown !== pending) return;
      const live = source.find((entry) => entry.id === pending.entry.id);
      const hidden = deleted.find((entry) => entry.id === pending.entry.id);
      if (live && hidden) {
        pending.inspected = false;
        pending.message = 'Files and Trash changed while checking. Check again to confirm the current location.';
      } else if (live || hidden) {
        foundLocation = hidden ? 'Trash' : 'Files';
        pending.inspected = false;
        pending.message = `Found in ${foundLocation}. Refreshing this view…`;
        await refreshRoot();
        if (generation !== sessionGeneration || namespaceUnknown !== pending) return;
        if (navigation !== navigationRequest) {
          pending.message = `Found in ${foundLocation}. Check again to refresh the current view before continuing.`;
          return;
        }
        if (showTrash) { await loadTrash(); if (trashError) throw new Error('Trash refresh failed'); }
        else if (!accountView && !settingsView && !await refresh()) throw new Error('Folder refresh failed');
        if (generation !== sessionGeneration || namespaceUnknown !== pending) return;
        if (navigation !== navigationRequest) {
          pending.message = `Found in ${foundLocation}. Check again to refresh the current view before continuing.`;
          return;
        }
        const shouldFocus = document.activeElement === trigger || document.activeElement === document.body;
        namespaceUnknown = null;
        clearCompletedFeedback(); actionNotice = `${pending.entry.name} is in ${foundLocation}`;
        await tick();
        if (shouldFocus && generation === sessionGeneration && navigation === navigationRequest &&
          (document.activeElement === trigger || document.activeElement === document.body)) {
          document.getElementById(showTrash ? 'trash-heading' : accountView ? 'account-title' : settingsView ? 'settings-title' : 'folder-title')?.focus();
        }
      } else {
        pending.inspected = true;
        pending.message = parentUnavailable ? 'Its original folder is unavailable. Open Trash to recover its parent, or dismiss this result after reviewing it.' : 'This item is absent from its original folder and Trash. Check its location before selecting it again.';
      }
    } catch {
      if (generation === sessionGeneration && namespaceUnknown === pending) pending.message = foundLocation ? `Found in ${foundLocation}, but this view could not refresh. Check again before continuing.` : 'Could not check Files and Trash. Check your connection and try again.';
    } finally { if (generation === sessionGeneration) namespaceInspecting = false; }
  }

  async function dismissNamespaceUnknown(): Promise<void> {
    if (!namespaceUnknown?.inspected || namespaceInspecting) return;
    namespaceUnknown = null;
    await tick();
    document.getElementById(showTrash ? 'trash-heading' : accountView ? 'account-title' : settingsView ? 'settings-title' : 'folder-title')?.focus();
  }

  async function deleteEntry(): Promise<void> {
    const target = trashTarget;
    if (!target || trashBusy || namespaceUnknown) return;
    trashBusy = true;
    const sessionVersion = sessionGeneration;
    try {
      await listing.trash(target.id, target.etag);
      if (sessionVersion !== sessionGeneration) return;
      clearCompletedFeedback(); actionNotice = 'Moved to trash';
      await refresh();
      if (sessionVersion !== sessionGeneration) return;
      trashTarget = null;
      try { await refreshRoot(); } catch { /* The deletion was confirmed. */ }
      if (sessionVersion === sessionGeneration && showTrash) await loadTrash();
    } catch (error) {
      if (sessionVersion !== sessionGeneration) return;
      if (moveOutcomeUncertain(error)) rememberNamespaceUnknown(target, 'trash');
      else trashError = error instanceof HttpError && error.status === 412 ? 'This item changed. Refresh the folder before trying again.' : 'Could not move this item to trash. Refresh the folder before trying again.';
    } finally { if (sessionVersion === sessionGeneration) trashBusy = false; }
  }
  function needsDeletedReference(item: DeletedEntry): boolean {
    return !!item.needs_reference || deletedItems.some((other) => other.id !== item.id && other.name === item.name &&
      other.library_name === item.library_name &&
      other.original_path.length === item.original_path.length &&
      other.original_path.every((part, index) => part === item.original_path[index]));
  }
  function trashLocation(item: DeletedEntry): string {
    return [item.library_name ?? 'Files', ...item.original_path].join(' / ');
  }
  async function restoreEntry(item: Entry, trigger: HTMLElement): Promise<void> {
    if (trashBusy || trashLoading || namespaceUnknown || removalSubmitting || (removalPending && !removalPending.result) || removalPreparing) return;
    const navigation = navigationRequest;
    const watched = watchFocus(trigger);
    trashBusy = true; restoringId = item.id; restoringName = item.name; trashError = '';
    const sessionVersion = sessionGeneration;
    let failed = false;
    try {
      if (!await withTreeSharingConfirmation(item, 'restore', token => listing.restore(item.id, item.etag, token))) {
        failed = true;
        return;
      }
      if (sessionVersion !== sessionGeneration) return;
      clearCompletedFeedback(); actionNotice = 'Restored';
      if (showTrash && navigation === navigationRequest) {
        deletedItems = []; trashPageIndex = 0; trashPageCursors = [null]; trashNextCursor = null; trashHasMore = false; trashRetry = null;
        await loadTrash();
      }
      if (sessionVersion === sessionGeneration) await refresh();
    } catch (error) {
      if (sessionVersion !== sessionGeneration) return;
      failed = true;
      if (moveOutcomeUncertain(error)) rememberNamespaceUnknown(item, 'restore');
      else if (error instanceof HttpError && error.code === 'sharing_audience_changed') trashError = 'Sharing changed. Review the original folder and try again.';
      else trashError = error instanceof HttpError && error.status === 409 ? 'Restore its parent folder first, or rename the item occupying its original name.' : 'Could not restore this item. Refresh trash before trying again.';
    } finally {
      if (sessionVersion === sessionGeneration) {
        trashBusy = false; restoringId = ''; restoringName = '';
        await tick();
        if (sessionVersion === sessionGeneration && navigation === navigationRequest && showTrash && watched.mayFocus()) {
          if (failed) restoreFocus(trigger, 'trash-heading');
          else (trashError && trashRetry ? document.getElementById('trash-page-retry') : document.querySelector<HTMLElement>('.trash-row button:not(:disabled)') ?? document.getElementById('trash-heading'))?.focus();
        }
      }
      watched.stop();
    }
  }

  async function uploadOutcome(id: string): Promise<'committed' | 'canceled' | 'rejected' | 'unknown'> {
    try {
      const status = await transfers.status(id);
      if (status.state === 'committed' || status.state === 'rejected') transfers.forget(id);
      if (status.state === 'committed') return 'committed';
      if (status.state === 'rejected' && status.status === 'canceled') return 'canceled';
      if (status.state === 'rejected') return 'rejected';
      return 'unknown';
    } catch { return 'unknown'; }
  }

  async function showCommittedUpload(): Promise<void> {
    const generation = sessionGeneration;
    transfers.forget(uploadId || unresolvedUploadId);
    progress = 100; transferPhase = 'complete';
    if (uploadBatchBusy) {
      if (batchActive?.kind === 'file') batchActive.status = 'saved';
    }
    uploadNotice = uploadReplacement ? 'Replacement complete. The previous file is in trash. Rename the current file before restoring the previous one.' : '';
    if (uploadBatchBusy) return;
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

  function chooseFolderUploads(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = '';
    if (!files.length || !folderChooserSupported || !canUploadDrop()) return;
    void runUploadBatch(async () => directoryFilesToUpload(files), currentFolder);
  }

  function dismissReplacement(): void {
    replacementPrompt = null;
    const decide = replacementDecision; replacementDecision = null;
    decide?.(null);
  }

  async function confirmReplacement(): Promise<void> {
    const prompt = replacementPrompt;
    if (!prompt || transferBusy || unresolvedUploadId || replacementAudience.pending) return;
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

  async function startUpload(file: File, input: HTMLInputElement | null, replacement?: Replacement, folder = currentFolder, resumeId?: string): Promise<boolean> {
    if (namespaceUnknown) return false;
    const generation = sessionGeneration;
    const activeTransfers = transfers;
    let committed = false;
    transferBusy = true;
    uploadReplacement = !!replacement;
    progress = 0;
    transferPhase = 'checking';
    transferName = file.name;
    const uploadController = new AbortController();
    controller = uploadController;
    try {
      if (uploadBatchBusy ? uploadBatchCanceled : !await refreshRoot()) return committed;
      const targetFolder = folder;
      if (uploadBatchBusy) uploadBatchStage = 'Checking file';
      const identity = await identifyFile(file, uploadController.signal, (read, total) => { if (generation === sessionGeneration) progress = total ? Math.floor(read / total * 100) : 100; });
      if (generation !== sessionGeneration) return committed;
      const pending = activeTransfers.pending();
      if (resumeId && (!pending || pending.id !== resumeId || pending.identity.name !== identity.name || pending.identity.size !== identity.size || pending.identity.fingerprint !== identity.fingerprint)) {
        uploadNotice = 'This file does not match the original upload. Select the original file; its saved offset is unchanged.';
        return committed;
      }
      const admission = resumeId ? { id: resumeId } : await activeTransfers.admit(targetFolder.id, file.name, file.size, targetFolder.etag, (id) => { if (generation === sessionGeneration) uploadId = id; }, replacement, identity);
      resumeFile = file;
      transferPhase = 'uploading';
      if (generation !== sessionGeneration) return committed;
      uploadId = admission.id;
      progress = 0;
      uploadCancelable = true;
      if (uploadBatchBusy && uploadBatchCanceled) throw new DOMException('Upload canceled', 'AbortError');
      if (uploadBatchBusy) uploadBatchStage = 'Sending';
      await activeTransfers.send(admission.id, file, uploadController.signal, (sent, total) => {
        if (generation !== sessionGeneration) return;
        progress = total ? Math.floor((sent / total) * 100) : 0;
        transferWaiting = sent >= total ? 'All bytes sent. The server is verifying the upload…' : '';
        if (batchActive?.kind === 'file') batchActive.sent = Math.max(batchActive.sent, Math.min(file.size, sent));
      }, (message) => { if (generation === sessionGeneration) transferWaiting = message; });
      if (generation !== sessionGeneration) return committed;
      transferWaiting = '';
      if (uploadBatchBusy && uploadBatchCanceled) throw new DOMException('Upload canceled', 'AbortError');
      uploadCancelable = false;
      progress = 100;
      transferPhase = 'saving';
      if (batchActive?.kind === 'file') { batchActive.sent = file.size; batchActive.sentComplete = true; }
      if (uploadBatchBusy) uploadBatchStage = 'Saving';
      try { await activeTransfers.commit(admission.id); }
      catch (error) {
        if (generation !== sessionGeneration) return committed;
        if (replacement && saveRefused(error)) {
          try {
            await activeTransfers.cancel(admission.id);
            if (generation !== sessionGeneration) return committed;
            uploadNotice = 'Replacement was not saved because the file or folder changed. This upload did not replace the existing file. Refresh and try again.';
            return committed;
          } catch { /* Check durable status when cancellation cannot be confirmed. */ }
        }
        const outcome = await uploadOutcome(admission.id);
        if (generation !== sessionGeneration) return committed;
        if (outcome === 'rejected') {
          uploadNotice = 'Upload failed. No saved file is implied.';
          return committed;
        }
        if (outcome !== 'committed') {
          markUnresolved(admission.id, file.name);
          uploadNotice = 'Upload outcome could not be confirmed. Check your files before retrying.';
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
        if (uploadBatchBusy && uploadBatchCanceled) uploadNotice = 'Upload canceled before sending bytes.';
        else if (target?.kind === 'file') {
          replacementPrompt = { file, folder, target };
          uploadNotice = 'A file already has this name. Confirm replacement to continue.';
        } else uploadNotice = 'A folder already has this name. Rename the upload or the folder and try again.';
      } else if (error instanceof HttpError && error.status >= 400 && error.status < 500 && !uploadCancelable && (transferPhase === 'checking' || transferPhase === 'uploading')) {
        if (uploadId) activeTransfers.forget(uploadId);
        uploadId = '';
        uploadNotice = replacement ? 'Replacement was not admitted. The file or folder changed. Refresh and try again.' : 'Upload was not admitted. Refresh the folder and try again.';
      } else if (error instanceof DOMException && error.name === 'AbortError') {
        if (!uploadId) { uploadNotice = 'Upload canceled before sending bytes.'; return committed; }
        try {
          if (!uploadId) throw new Error('Upload was not admitted');
          await activeTransfers.cancel(uploadId);
          if (generation !== sessionGeneration) return committed;
          uploadNotice = uploadReplacement ? 'Replacement canceled. This upload did not replace the existing file.' : 'Upload canceled. No saved file is implied.';
        } catch {
          if (generation !== sessionGeneration) return committed;
          const outcome = uploadId ? await uploadOutcome(uploadId) : 'unknown';
          if (generation !== sessionGeneration) return committed;
          if (outcome === 'committed') {
            committed = true;
            await showCommittedUpload();
          } else if (outcome === 'canceled') uploadNotice = uploadReplacement ? 'Replacement canceled. This upload did not replace the existing file.' : 'Upload canceled. No saved file is implied.';
          else { markUnresolved(uploadId, file.name); unresolvedCancel = true; cancelPending = true; uploadNotice = 'Upload outcome could not be confirmed. Check your files before retrying.'; }
        }
      } else if (uploadId) {
        const outcome = await uploadOutcome(uploadId);
        if (generation !== sessionGeneration) return committed;
        if (outcome === 'committed') { committed = true; await showCommittedUpload(); }
        else if (outcome === 'canceled' || outcome === 'rejected') uploadNotice = uploadReplacement ? 'Replacement failed. This upload did not replace the existing file.' : 'Upload failed. No saved file was created.';
        else { markUnresolved(uploadId, file.name); uploadNotice = 'Upload outcome could not be confirmed. Check status before retrying.'; }
      } else uploadNotice = error instanceof Error ? error.message : 'Upload failed before it was admitted.';
    } finally {
      if (generation !== sessionGeneration) return committed;
      uploadCancelable = false;
      transferBusy = false;
      transferWaiting = '';
      resetTransferActivity();
      controller = null;
      if (!unresolvedUploadId) { uploadId = ''; resumeFile = null; resumeReady = false; }
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
          if (retryError instanceof HttpError && (retryError.status === 410 || (retryError.status === 401 && retryError.code === 'invalid_mutation_ticket' && (activeTransfers.pending()?.expires ?? Infinity) <= Date.now() / 1000))) {
            // An expired ticket can no longer be admitted, so a second 404 is final.
            let admitted = true;
            try { status = await activeTransfers.status(unresolvedUploadId); }
            catch (finalError) { admitted = !(finalError instanceof HttpError && finalError.status === 404); }
            if (generation !== sessionGeneration) return;
            if (!admitted) {
              activeTransfers.forget(unresolvedUploadId);
              unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false; cancelPending = false;
              resolveUploadResult('failed');
              uploadNotice = 'The upload was never admitted and its ticket expired. No saved file was created.';
              transferBusy = false;
              return;
            }
          }
        }
        if (!status) {
          uploadNotice = 'Admission could not be confirmed. The original request is retained; retry checking before starting another upload.';
          transferBusy = false;
          return;
        }
      } else {
        uploadNotice = 'Upload status is unavailable. Try checking again before starting another upload.';
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
      unresolvedCancel = false; cancelPending = false;
      await showCommittedUpload();
      if (generation !== sessionGeneration) return;
      if (!uploadReplacement) uploadNotice = '';
    } else if (outcome === 'canceled' || outcome === 'rejected') {
      activeTransfers.forget(unresolvedUploadId);
      resolveUploadResult(outcome === 'canceled' ? 'canceled' : 'failed');
      unresolvedUploadId = '';
      uploadId = '';
      unresolvedCancel = false; cancelPending = false;
      uploadNotice = outcome === 'canceled' ? 'Upload canceled. No saved file was created.' : 'Upload was rejected. No saved file was created.';
    } else {
      const idle = !status.writer_active;
      uploadExpired = idle && (status.state === 'admitted' || status.state === 'publishing') && activeTransfers.expired(unresolvedUploadId);
      finishReady = !uploadExpired && activeTransfers.finishable(unresolvedUploadId, status);
      resumeReady = !uploadExpired && status.state === 'admitted' && idle;
      unresolvedCancel = true; cancelPending = false;
      uploadNotice = uploadExpired ? (status.state === 'publishing' ? expiredSaveNotice : expiredResumeNotice)
        : finishReady ? `${formatSize(status.length)} received. The file is not saved yet. Finish saving to add it.`
        : resumeReady ? `${formatSize(status.bytes_received)} received. ${resumeFile ? 'Resume this upload' : 'Select the original file to resume'}.`
        : status.status === 'recovery_required' ? 'This upload needs server recovery. Ask the administrator to restart the service, then check again. Its bytes remain private.'
        : !activeTransfers.canCommit(unresolvedUploadId) ? 'Saving is unconfirmed. Check this upload again before continuing.'
        : 'The server is still processing this upload. Check again shortly.';
    }
    transferBusy = false;
  }

  async function resumeUpload(file: File): Promise<void> {
    if (transferBusy || uploadBatchBusy || !resumeReady || !unresolvedUploadId) return;
    const pending = transfers.pending();
    if (!pending || pending.id !== unresolvedUploadId) { uploadNotice = 'The original upload record is unavailable. Check its status or cancel it.'; return; }
    const id = pending.id;
    // Block before re-reading a large file: the server refuses expired tickets.
    if (transfers.expired(id)) { resumeReady = false; uploadExpired = true; uploadNotice = expiredResumeNotice; return; }
    await startUpload(file, null, pending.body.replacement, { id: pending.body.parent_id, etag: pending.body.parent_etag, name: 'Original destination', parent_id: null, kind: 'folder' }, id);
    if (transferPhase === 'complete') { unresolvedUploadId = ''; resumeReady = false; unresolvedCancel = false; cancelPending = false; transfers.forget(id); resolveUploadResult('saved'); }

  }
  async function finishUpload(): Promise<void> {
    if (!unresolvedUploadId || !finishReady || transferBusy || uploadBatchBusy) return;
    const generation = sessionGeneration;
    const activeTransfers = transfers;
    const id = unresolvedUploadId;
    transferBusy = true; transferPhase = 'saving'; progress = 100;
    try {
      await activeTransfers.finish(id);
      if (generation !== sessionGeneration) return;
      finishReady = false;
      resolveUploadResult('saved');
      uploadId = id; unresolvedUploadId = ''; unresolvedCancel = false; cancelPending = false;
      await showCommittedUpload();
      if (generation !== sessionGeneration) return;
      uploadId = '';
      if (!uploadReplacement) uploadNotice = '';
    } catch (error) {
      if (generation !== sessionGeneration) return;
      finishReady = false;
      if (saveRefused(error)) {
        const reason = error.code === 'name_collision' ? 'An item with this name now exists in the destination' : error.code === 'replacement_changed' ? 'The file being replaced changed' : 'The destination folder changed';
        uploadNotice = `${reason}, so this upload was not saved. Cancel it, then upload the file again.`;
      } else {
        // Status decides: a lost reply after a real save resolves as saved, and nothing is re-sent automatically.
        transferBusy = false; transferPhase = '';
        await reconcileUpload();
        if (generation === sessionGeneration && finishReady) uploadNotice = 'Saving did not complete, and the file is not saved yet. Finish saving to try again.';
      }
    } finally {
      if (generation === sessionGeneration) { transferBusy = false; resetTransferActivity(); }
    }
    // The finish control disappears with its outcome; keep keyboard focus in the recovery alert.
    await tick();
    if (generation === sessionGeneration && unresolvedUploadId && (document.activeElement === document.body || !document.activeElement)) document.getElementById('upload-check')?.focus();
  }
  function reselectUpload(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0]; input.value = '';
    if (file) void resumeUpload(file);
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
      unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false; cancelPending = false;
      uploadNotice = 'Upload canceled. No saved file was created.';
    } catch {
      if (generation !== sessionGeneration) return;
      const outcome = await uploadOutcome(unresolvedUploadId);
      if (generation !== sessionGeneration) return;
      if (outcome === 'canceled' || outcome === 'rejected') {
        resolveUploadResult(outcome === 'canceled' ? 'canceled' : 'failed');
        unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false; cancelPending = false;
        uploadNotice = outcome === 'canceled' ? 'Upload canceled. No saved file was created.' : 'Upload was rejected. No saved file was created.';
      } else if (outcome === 'committed') {
        resolveUploadResult('saved');
        unresolvedUploadId = ''; uploadId = ''; unresolvedCancel = false; cancelPending = false;
        await showCommittedUpload();
      } else { cancelPending = true; uploadNotice = 'Cancellation is still unconfirmed. Retry cancellation or check status.'; }
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

  let archiveBusy = $state(false);
  let archiveTargets = $state<string[]>([]);
  function archiveFailure(error: unknown): string {
    if (error instanceof HttpError && error.code === 'archive_entry_limit') return `Too many items to download together. The limit is ${(error.details?.limit ?? 10000).toLocaleString()} files and folders, including folder contents. Download smaller groups.`;
    if (error instanceof HttpError && error.code === 'archive_name_conflict') return 'Two selected items have the same name. Download them separately.';
    if (error instanceof HttpError && error.status === 404) return 'Some selected items are no longer available. Refresh your files.';
    return 'Download could not be started.';
  }
  async function downloadArchive(targets: Entry[]): Promise<void> {
    if (archiveBusy || !targets.length) return;
    if (targets.length === 1 && targets[0].kind === 'file') return download(targets[0]);
    const generation = sessionGeneration;
    archiveBusy = true; archiveTargets = targets.map((entry) => entry.id); clearCompletedFeedback();
    const feedback = feedbackRevision;
    try {
      const ticket = await listing.archive(targets.map((entry) => entry.id));
      if (generation !== sessionGeneration) return;
      const anchor = document.createElement('a');
      anchor.href = ticket.url;
      anchor.rel = 'noopener';
      anchor.download = ticket.name;
      anchor.click();
      if (feedback === feedbackRevision) notice = 'Download requested. Check your browser for progress.';
    } catch (error) {
      if (generation === sessionGeneration && feedback === feedbackRevision) notice = archiveFailure(error);
    } finally { if (generation === sessionGeneration) { archiveBusy = false; archiveTargets = []; } }
  }

  async function openFolder(folder: Folder): Promise<boolean> {
    const index = folderPath.findIndex((item) => item.id === folder.id);
    const path = index >= 0 ? folderPath.slice(0, index + 1) : [...folderPath, folder];
    return navigate('files', path);
  }
</script>

{#snippet publicLinksStatus()}
  <p class="text-sm text-muted-foreground empty:absolute" aria-live="polite" aria-atomic="true">{#if publicLinksError && !restorationPending}Could not check public links. Open Shared links in Account to check.{/if}</p>
{/snippet}

{#snippet routePlaceholder()}
  <div class="route-placeholder" aria-hidden="true"><span></span><span></span><span></span></div>
{/snippet}

{#snippet resultSortOptions()}
  <option value="name_asc">Name: A to Z</option><option value="name_desc">Name: Z to A</option>
  <option value="location_asc">Location: A to Z</option><option value="location_desc">Location: Z to A</option>
  <option value="modified_asc">Modified: oldest first</option><option value="modified_desc">Modified: newest first</option>
  <option value="size_asc">Size: smallest first</option><option value="size_desc">Size: largest first</option>
{/snippet}

{#snippet modifiedDateLabel(entry: Entry)}
  {@const date = modifiedTime(entry)}
  {#if date}<time datetime={date.datetime} title={date.full}><span aria-hidden="true">{date.compact}</span><span class="sr-only">Modified {date.full}</span></time>
  {:else}{@const label = entry.kind === 'folder' ? 'Modified date not applicable' : 'Modified date unknown'}<span title={label}><span class="sr-only">{label}</span><span aria-hidden="true">—</span></span>{/if}
{/snippet}

<svelte:window ondragover={externalDrag} ondrop={externalDrag} onkeydown={listKeydown} onfocusin={trackListFocus} onpointerdowncapture={(event) => { if (destinationAction === 'move' && destinationBusy && moveDialog && event.target instanceof Node && !moveDialog.contains(event.target)) event.preventDefault(); }} />
<svelte:head>
  <title>{instanceName} · Private files</title>
  <meta name="description" content="Private file storage" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</svelte:head>

{#snippet selectionHeader(disabled: boolean)}
  <div role="columnheader" class="entry-selection">
    <label title="Select all loaded items">
      <input
        type="checkbox"
        aria-label="Select all loaded items"
        checked={allLoadedSelected}
        indeterminate={selectedEntries.length > 0 && !allLoadedSelected}
        disabled={disabled || fileMutationBusy || !activeEntries.length}
        onchange={(event) => {
          if (event.currentTarget.checked) selectedIds = activeEntries.map((entry) => entry.id);
          else clearSelection();
        }}
      />
    </label>
  </div>
{/snippet}

{#snippet selectionToolbar()}
              <div class="selection-toolbar" role="group" aria-label="Selected items">
                <span aria-live="polite">{selectedEntries.length} selected</span>
                {#if searchQuery}<span class="text-xs text-muted-foreground">Current loaded page only</span>{/if}
                <Button variant="ghost" size="sm" disabled={!selectedIds.length || bulkBusy} onclick={clearSelectionFromToolbar}>Clear selection</Button>
                <Button variant="outline" size="sm" disabled={archiveBusy || bulkBusy} aria-busy={archiveBusy} onclick={() => downloadArchive(selectedEntries)}>{archiveBusy ? 'Preparing…' : 'Download'}</Button><Button variant="outline" size="sm" disabled={fileMutationBusy} onclick={(event) => beginBulk('move', event.currentTarget)}>Move selected</Button><Button variant="outline" size="sm" disabled={fileMutationBusy} onclick={(event) => beginBulk('trash', event.currentTarget)}>Move selected to trash</Button>
              </div>
{/snippet}

{#snippet betaNotice(withShortcuts: boolean)}
  <footer class="beta-notice" class:beta-notice-with-shortcuts={withShortcuts}>
    <span>Beta · Durability not qualified. Keep independent copies.</span>
    {#if withShortcuts}
      <Button variant="link" size="xs" class="footer-shortcuts" aria-haspopup="dialog" onclick={(event) => openShortcuts(event.currentTarget)}>Keyboard shortcuts</Button>
    {/if}
  </footer>
{/snippet}

<div class="app-shell">
  <a class="skip-link" href="#main-content">Skip to content</a>
  {#if !user || loggingOut}
  <header class="app-header">
    <a class="brand" href="#files" onclick={home}><BrandLogo url={appearance.logoUrl} /><span>{instanceName}</span></a>
    <div class="header-actions">
      <HeaderIconButton label={darkMode ? 'Switch to light mode' : 'Switch to dark mode'} pressed={darkMode} onclick={toggleTheme}>{#if darkMode}<SunIcon />{:else}<MoonIcon />{/if}</HeaderIconButton>
      {#if user && !loggingOut}<HeaderIconButton label="Log out" disabled={loggingOut} onclick={signOut}><LogOutIcon /></HeaderIconButton>{/if}
    </div>
  </header>
  {/if}
  {#if loggingOut}
    <main id="main-content" class="signin-main"><p role="status">Signing out…</p></main>
  {:else if !user}
    <main id="main-content" class="signin-main" class:branded-signin={!!appearance.loginBackgroundColor || !!appearance.loginImageUrl} style:background-color={appearance.loginBackgroundColor ?? undefined} style:background-image={appearance.loginImageUrl ? `url("${appearance.loginImageUrl}")` : undefined}>
      <section class="signin-form" aria-labelledby="auth-heading">
        {#if appearanceLoadError}<p role="alert">{appearanceLoadError} <Button variant="link" disabled={appearanceLoading} onclick={loadAppearance}>Reload appearance</Button></p>{/if}
        <h1 id="auth-heading">{view === 'setup' ? 'Set up ' + instanceName : 'Sign in to ' + instanceName}</h1>
        <p class="signin-intro">{view === 'setup' ? 'Create the first administrator account.' : 'Private file storage.'}</p>
        <form class="grid gap-5" onsubmit={signIn}>
          {#if view === 'setup'}<div class="grid gap-2"><Label for="setup-auth">One-use setup authorization</Label><p id="setup-help" class="text-muted-foreground text-sm">Read <code class="break-all">secrets/setup.authorization</code> in the initialized state directory, or ask your server operator.</p><Input aria-describedby="setup-help" id="setup-auth" type="password" bind:value={setupAuthorization} autocomplete="off" required /></div>{/if}
          <div class="grid gap-2"><Label for="handle">Account name</Label><Input id="handle" bind:value={handle} autocomplete="username" pattern={view === 'setup' ? '[a-z0-9][a-z0-9._-]*' : undefined} maxlength={64} required />{#if view === 'setup'}<p class="text-muted-foreground text-xs">Lowercase letters, numbers, dots, underscores or hyphens. Start with a letter or number.</p>{/if}</div>
          <div class="grid gap-2"><Label for="password">Password</Label><Input id="password" type="password" bind:value={password} autocomplete={view === 'setup' ? 'new-password' : 'current-password'} minlength={view === 'setup' ? 15 : undefined} maxlength={view === 'setup' ? 128 : undefined} required />{#if view === 'setup'}<p class="text-muted-foreground text-xs">Use 15 to 128 characters.</p>{/if}</div>
          <Button type="submit" disabled={authBusy || oidcOpening}>{authBusy ? (view === 'setup' ? 'Setting up…' : 'Signing in…') : (view === 'setup' ? 'Create administrator' : 'Sign in')}</Button>
        </form>
        {#if view === 'login' && oidcEnabled}
          <Button id="keycloak-sign-in" type="button" variant="outline" class="mt-3 w-full" disabled={authBusy || oidcOpening} aria-busy={oidcOpening} onclick={() => void startKeycloak()}>{oidcOpening ? 'Opening Keycloak…' : 'Sign in with Keycloak'}</Button>
        {/if}
        {#if notice}<p class="form-notice" role="status">{notice}</p>{/if}
        <Button class="setup-link" type="button" variant="link" disabled={authBusy || oidcOpening} onclick={() => { view = view === 'setup' ? 'login' : 'setup'; notice = ''; }}>{view === 'setup' ? 'Back to sign in' : 'First-time setup'}</Button>
      </section>
    </main>
  {:else}
    <Sidebar.Provider>
      <AppSidebar {instanceName} logoUrl={appearance.logoUrl} onhome={home}>
        {#snippet navigation(close)}
          <nav aria-label="Files"><Sidebar.Menu>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="My libraries" aria-label="My libraries" aria-busy={routeName === 'libraries'} isActive={!!librariesCurrent} aria-current={librariesCurrent} onclick={() => { close(); void navigate('libraries'); }}><LibraryIcon /><span>My libraries</span></Sidebar.MenuButton></Sidebar.MenuItem>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="Shared with me" aria-label="Shared with me" isActive={sharedView === 'shared'} aria-current={sharedView === 'shared' ? 'page' : undefined} onclick={() => { close(); void navigate('shared'); }}><FolderOpenIcon /><span>Shared with me</span></Sidebar.MenuButton></Sidebar.MenuItem>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="Shared with all" aria-label="Shared with all" isActive={sharedView === 'published'} aria-current={sharedView === 'published' ? 'page' : undefined} onclick={() => { close(); void navigate('published'); }}><UsersRoundIcon /><span>Shared with all</span></Sidebar.MenuButton></Sidebar.MenuItem>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="Files" aria-label="Files" aria-busy={routeName === 'files'} isActive={filesCurrent} aria-current={filesCurrent ? 'page' : undefined} data-drop-target={folders[0]?.id} data-drop-active={dropTarget === folders[0]?.id} ondragover={(event) => folders[0] && dragOver(event, folders[0])} ondragleave={(event) => folders[0] && dragLeave(event, folders[0])} ondrop={(event) => folders[0] && dropEntry(event, folders[0])} onclick={() => { close(); void navigate('files', folders[0] ? [folders[0]] : undefined); }}><FolderIcon /><span>Files</span></Sidebar.MenuButton></Sidebar.MenuItem>
          <Sidebar.MenuItem><Sidebar.MenuButton tooltipContent="Trash" aria-label="Trash" aria-busy={routeName === 'trash'} isActive={showTrash} aria-current={showTrash ? 'page' : undefined} onclick={() => { close(); void openTrash(); }}><TrashIcon /><span>Trash</span></Sidebar.MenuButton></Sidebar.MenuItem>

          </Sidebar.Menu></nav>
          {#if user?.role === 'administrator'}
            <nav aria-label="Administration" class="mt-1"><Sidebar.Menu><Sidebar.MenuItem>
              <Sidebar.MenuButton tooltipContent="Settings" aria-label="Settings" aria-busy={routeName === 'settings'} isActive={settingsView} aria-current={settingsView ? 'page' : undefined} onclick={() => { close(); void navigate('settings'); }}><SettingsIcon /><span>Settings</span></Sidebar.MenuButton>
            </Sidebar.MenuItem></Sidebar.Menu></nav>
          {/if}
        {/snippet}
        {#snippet account(close)}
          <nav aria-label="Account"><Sidebar.Menu><Sidebar.MenuItem>
            <Sidebar.MenuButton size="lg" tooltipContent="Account" aria-label="Account" aria-busy={routeName === 'account'} isActive={accountView} aria-current={accountView ? 'page' : undefined} onclick={() => { close(); void navigate('account'); }}>
              <UserRoundIcon /><span class="sidebar-account"><span>{user?.handle}</span><span class="sidebar-account-role">{user?.role === 'administrator' ? 'Administrator' : 'User'}</span></span>
            </Sidebar.MenuButton>
          </Sidebar.MenuItem></Sidebar.Menu></nav>
        {/snippet}
      </AppSidebar>
      <Sidebar.Inset class="min-w-0">
        <header class="app-header workspace-header">
          <div class="workspace-location"><Sidebar.Trigger /><span class="header-divider shrink-0"></span><span class="truncate" title={filesView ? namedLibrary?.name : undefined}>{settingsView ? 'Settings' : accountView ? 'Account' : showTrash ? 'Trash' : librariesView ? 'My libraries' : sharedView ? sharingTitle(sharedView) : (namedLibrary?.name ?? 'Files')}</span></div>
          <div class="header-actions">
            <HeaderIconButton label={darkMode ? 'Switch to light mode' : 'Switch to dark mode'} pressed={darkMode} onclick={toggleTheme}>{#if darkMode}<SunIcon />{:else}<MoonIcon />{/if}</HeaderIconButton>
            <HeaderIconButton label="Log out" disabled={loggingOut} onclick={signOut}><LogOutIcon /></HeaderIconButton>
          </div>
        </header>
        <div id="main-content" class="workspace-main">
            {#key sessionGeneration}
              {#if textEdit?.ownerId === user.id && textEditSession}
                <FilePreview entry={previewEntry} url={previewEntry ? listing.downloadUrl(previewEntry.id) : ''} {client}
                  bind:edit={textEdit.state} editSignal={textEditSession.signal}
                  onclose={closePreview} ondownload={() => { if (previewEntry) void download(previewEntry); }}
                  onresume={openPreview} onsaved={textEditSaved} />
              {/if}
            {/key}
            {#if archiveBusy}<p role="status" class="workspace-feedback">Preparing download…</p>{/if}
            {#if actionNotice}
              {#key feedbackRevision}<div class="action-feedback workspace-feedback">
                <p role="status"><CheckIcon aria-hidden="true" /><span>{actionNotice}</span></p>
                <Button variant="ghost" size="icon" aria-label="Dismiss notification" title="Dismiss notification" onclick={dismissActionNotice}><XIcon /></Button>
              </div>{/key}
            {/if}
            {#if trashBusy && restoringId && !showTrash}
              <div class="action-feedback workspace-feedback" aria-busy="true">
                <p role="status">Restoring {restoringName}…</p><Button variant="ghost" onclick={openTrash}>Open Trash</Button>
              </div>
            {/if}
            {#if pendingCopy && (!moveBusy || !moveTarget)}
              <section class="upload-batch-report" aria-label="Copy status" aria-busy={!!copyRecovery}>
                <p role="status">{copyRecoveryMessage || 'Checking copy status…'}</p>
                {#if copyStorageFailed}<p>Keep this tab open until the copy is confirmed. This browser could not save recovery.</p>{/if}
                <Button variant="outline" size="sm" disabled={!!copyRecovery || moveBusy} onclick={() => reconcileCopy('status')}>{copyRecovery === 'status' ? 'Checking…' : 'Check copy status'}</Button>
                <Button variant="outline" size="sm" disabled={!!copyRecovery || moveBusy} onclick={() => reconcileCopy('retry')}>{copyRecovery === 'retry' ? 'Retrying…' : 'Retry same copy'}</Button>
              </section>
            {:else if !pendingCopy && copyRecoveryMessage}
              <div class="action-feedback workspace-feedback">
                <p role="alert">{copyRecoveryMessage}</p>
                <Button variant="ghost" size="icon" aria-label="Dismiss copy notification" onclick={() => copyRecoveryMessage = ''}><XIcon /></Button>
              </div>
            {/if}
            {#if removalPending}
              <section class="upload-batch-report" aria-label="Permanent removal status">
                <p role="status">{removalPending.message}</p>
                <Button variant="outline" size="sm" disabled={removalChecking || removalSubmitting} aria-busy={removalChecking} onclick={checkRemoval}>{removalChecking ? 'Checking…' : 'Check removal status'}</Button>
                {#if !removalPending.result}<Button variant="outline" size="sm" disabled={removalChecking || removalSubmitting} aria-busy={removalSubmitting} onclick={replayRemoval}>{removalSubmitting ? 'Retrying…' : 'Retry same removal'}</Button>{/if}
              </section>
            {/if}
            {#if namespaceUnknown}
              <section class="bulk-results" aria-label="Unconfirmed file action" aria-busy={namespaceInspecting}>
                <h2>{namespaceUnknown.action === 'trash' ? 'Move to trash unconfirmed' : 'Restore unconfirmed'}</h2>
                <p role="status"><strong class="break-all">{namespaceUnknown.entry.name}</strong>. {namespaceUnknown.message}</p>
                <div class="flex flex-wrap gap-2">
                  <Button variant="outline" disabled={namespaceInspecting} aria-busy={namespaceInspecting} onclick={inspectNamespace}>{namespaceInspecting ? 'Checking Files and Trash…' : 'Check Files and Trash'}</Button>
                  <Button variant="ghost" disabled={namespaceInspecting} onclick={openTrash}>Open Trash</Button>
                  {#if namespaceUnknown.inspected}<Button variant="ghost" disabled={namespaceInspecting} onclick={dismissNamespaceUnknown}>Dismiss result</Button>{/if}
                </div>
              </section>
            {/if}
            {#if bulkReport}
              <section class:action-feedback={bulkComplete} class:bulk-results={!bulkComplete} aria-label={bulkReport.action === 'move' ? 'Move results' : 'Trash results'} aria-busy={bulkBusy}>
                {#if bulkComplete}
                  <p role="status"><CheckIcon aria-hidden="true" /><span>{bulkReport.results.length === 1 ? bulkReport.action === 'move' ? `Moved to ${bulkReport.destination?.name}` : 'Moved to trash' : `${bulkReport.results.length} items ${bulkReport.action === 'move' ? 'moved' : 'moved to trash'}`}</span></p>
                {:else}
                  <div class="action-feedback-heading"><h2>{bulkReport.action === 'move' ? 'Move results' : 'Trash results'}</h2>{#if !bulkBusy && !bulkNeedsRefresh}<Button variant="ghost" size="icon" aria-label="Dismiss results" title="Dismiss results" onclick={dismissBulkReport}><XIcon /></Button>{/if}</div>
                  <p role="status">{bulkSucceeded} of {bulkReport.results.length} succeeded{bulkBusy ? '. Working…' : '.'}{#if bulkReport.results.some((result) => result.status === 'uncertain')}{' '}{bulkReport.results.filter((result) => result.status === 'uncertain').length} unconfirmed.{/if}</p>
                  <ul>{#each bulkReport.results.filter((result) => result.status !== 'succeeded') as result (result.entry.id)}<li><strong>{result.entry.name}</strong><span>{result.location}</span><span>{result.message}</span></li>{/each}</ul>
                {/if}
                {#if bulkError}<p role="alert">{bulkError}</p>{/if}
                {#if bulkComplete}<Button variant="ghost" size="icon" aria-label="Dismiss results" title="Dismiss results" onclick={dismissBulkReport}><XIcon /></Button>{:else}<div class="flex flex-wrap gap-2">
                  {#if bulkNeedsRefresh}<RefreshButton label="Refresh before retry" busyLabel="Checking results…" busy={bulkBusy} onclick={reconcileBulk} />{/if}
                  {#if bulkReport.results.some((result) => result.status === 'failed')}<Button variant="outline" disabled={bulkBusy || bulkNeedsRefresh || fileMutationBusy || loading} onclick={(event) => retryBulk(event.currentTarget)}>Retry failed items</Button>{/if}
                </div>{/if}
              </section>
            {/if}
            {#if unresolvedUploadId}<Alert.Root variant={resumeReady || finishReady ? 'default' : 'destructive'}><Alert.Title>{resumeReady ? 'Upload paused' : finishReady ? 'Upload not saved yet' : uploadExpired ? 'Upload expired' : 'Upload status needs checking'}</Alert.Title><Alert.Description><p>{unresolvedUploadName}</p><p role="status">{uploadNotice}</p>{#if transferBusy}<p role="status">{transferPhase === 'checking' ? 'Checking file' : transferPhase === 'saving' ? 'Saving' : 'Uploading'}: {progress}%{#if transferWaiting}. {transferWaiting}{/if}</p>{/if}<div class="mt-2 flex flex-wrap gap-2"><Button id="upload-check" variant="outline" size="sm" onclick={reconcileUpload} disabled={transferBusy}>Check upload status</Button>{#if resumeReady}<input class="sr-only" type="file" bind:this={reselectInput} onchange={reselectUpload} aria-label="Original upload file" /><Button variant="outline" size="sm" onclick={() => resumeFile ? void resumeUpload(resumeFile) : reselectInput?.click()} disabled={transferBusy || uploadBatchBusy} aria-busy={transferBusy}>{resumeFile ? 'Resume upload' : 'Select original file'}</Button>{/if}{#if finishReady}<Button variant="outline" size="sm" onclick={finishUpload} disabled={transferBusy} aria-busy={transferBusy && transferPhase === 'saving'}>Finish saving</Button>{/if}{#if unresolvedCancel}<Button variant="outline" size="sm" onclick={retryCancel} disabled={transferBusy}>{cancelPending ? 'Retry cancellation' : 'Cancel upload'}</Button>{/if}</div></Alert.Description></Alert.Root>{/if}
            {#if (uploadBatchBusy || uploadReportVisible) && (uploadBatchStatus || uploadResults.length)}
              <section class="upload-batch-report" aria-label="Upload results">
                {#if uploadNotice}<p class="upload-feedback" role="status">{uploadNotice}</p>{/if}
                {#if uploadBatchStatus && !uploadBatchBusy && !uploadComplete}<p class="upload-batch-status" role="status">{uploadBatchStatus}</p>{/if}
                {#if uploadBatchBusy}
                  {#if batchFiles.length > 1}
                    <div class="upload-batch-progress">
                      <div class="upload-batch-heading">
                        <span>{batchSaved} of {batchFiles.length} files saved{#if batchFoldersCreated} · {batchFoldersCreated} {batchFoldersCreated === 1 ? 'folder' : 'folders'} created{/if}</span>
                        <Button variant="ghost" size="icon" aria-label="Cancel upload" title="Cancel upload" disabled={uploadBatchCanceled} onclick={cancelUploadBatch}><XIcon /></Button>
                      </div>
                      <div class="upload-batch-current" role="status" aria-label="Uploading" aria-atomic="true">
                        <span>{batchDisplayStage}{batchCurrent ? ':' : ''}</span>
                        {#if batchCurrent}<span class="upload-current-path" title={batchCurrent.path}>{batchCurrent.path.split('/').at(-1)}</span>{/if}
                        <span class="sr-only">To {batchCurrentDestination}. {batchSaved} of {batchFiles.length} files saved. At least {Math.floor(batchSendPercent / 10) * 10}% sent.</span>
                      </div>
                      <div class="upload-batch-destination" title={batchCurrentDestination}>To {batchCurrentDestination}</div>
                      {#if transferWaiting}<p class="upload-waiting" role="status">{transferWaiting}</p>{/if}
                      <div class="upload-byte-progress" role="progressbar" aria-label={batchBytes ? 'Batch bytes sent' : 'Batch files sent'} aria-valuemin="0" aria-valuemax="100" aria-valuenow={batchSendPercent} aria-valuetext={`${batchSendPercent}% sent. ${batchSaved} of ${batchFiles.length} files saved.`}>
                        <span style:width={`${batchSendPercent}%`}></span>
                      </div>
                      <div class="upload-batch-bytes">{#if batchBytes}{formatSize(batchSent)} of {formatSize(batchBytes)} sent{:else}No file bytes to send{/if}<span>{batchSendPercent}% sent</span></div>
                      <details class="upload-location-details">
                        <summary>Upload location</summary>
                        <dl><dt>File or folder</dt><dd>{batchCurrent?.path ?? 'Preparing'}</dd><dt>Destination</dt><dd>{batchCurrentDestination}</dd></dl>
                      </details>
                    </div>
                  {:else}
                    <div class="upload-pending">
                      <div class="min-w-0" role="status" aria-label="Uploading" aria-atomic="true"><span class="sr-only">{transferName ? `${transferName}. ${progress}% ${transferPhase === 'checking' ? 'checked' : 'sent'}.` : 'Reading files and folders.'}</span><div class="upload-batch-current"><LoaderCircleIcon class="refreshing" aria-hidden="true" /><span>{batchDisplayStage}{batchCurrent ? ':' : ''}</span>{#if batchCurrent}<span class="upload-current-path" title={batchCurrent.path}>{batchCurrent.path.split('/').at(-1)}</span>{/if}</div><div class="upload-batch-destination" title={batchCurrentDestination}>To {batchCurrentDestination}{#if transferPhase === 'checking'} · {progress}% checked{/if}</div>{#if transferWaiting}<p class="upload-waiting">{transferWaiting}</p>{/if}</div>
                      <Button variant="ghost" size="icon" aria-label="Cancel upload" title="Cancel upload" disabled={uploadBatchCanceled} onclick={cancelUploadBatch}><XIcon /></Button>
                    </div>
                  {/if}
                {/if}
                {#if !uploadBatchBusy && uploadReportVisible}
                  {#if uploadComplete}
                    <div class="action-feedback"><p role="status"><CheckIcon aria-hidden="true" /><span>{uploadSuccessMessage()}</span></p><Button variant="ghost" size="icon" aria-label="Dismiss results" title="Dismiss results" onclick={() => dismissUploadReport()}><XIcon /></Button></div>
                  {:else}
                    <div class="action-feedback-heading"><p role="status">{batchSaved + batchFoldersCreated} of {uploadResults.length} items added{#if uploadResults.some((result) => result.status === 'unknown')}; {uploadResults.filter((result) => result.status === 'unknown').length} unconfirmed{/if}</p>{#if !unresolvedUploadId && uploadReportDismissible(uploadResults)}<Button variant="ghost" size="icon" aria-label="Dismiss results" title="Dismiss results" onclick={() => dismissUploadReport()}><XIcon /></Button>{/if}</div>
                    {#if uploadResults.length}<details open={uploadResultsExpanded} ontoggle={(event) => uploadResultsExpanded = event.currentTarget.open}>
                      <summary>Results for {uploadResults.length} {uploadResults.length === 1 ? 'item' : 'items'}</summary>
                      <ul>{#each uploadResults.filter((result) => !compactUnresolvedUpload || (result.status !== 'saved' && result.status !== 'created')) as result, index (index)}<li data-upload-result={result.path}><span class="upload-result-path">{result.path}</span><span>{uploadResultLabel(result)}</span>{#if result.status === 'unknown' && result.kind === 'folder'}<Button variant="outline" size="sm" onclick={() => inspectUploadDestination(result)}>Inspect destination</Button>{#if result.inspectionError}<span role="alert">{result.inspectionError}</span>{/if}{/if}</li>{/each}</ul>
                    </details>{/if}
                  {/if}
                {/if}
              </section>
            {/if}
        {#each previousUploadReports as report (report.id)}
          <section class="upload-batch-report" aria-label="Unconfirmed folder upload">
            <p class="upload-batch-destination">To {report.destination}</p>
            {#if report.notice}<p class="upload-feedback">{report.notice}</p>{/if}
            {#if report.status && !(report.results.length && report.results.every((result) => result.status === 'saved' || result.status === 'created'))}<p class="upload-batch-status">{report.status}</p>{/if}
            {#if report.results.length}<details open={report.expanded} ontoggle={(event) => report.expanded = event.currentTarget.open}>
              <summary>{report.results.filter((result) => result.status === 'unknown').length} folders need checking</summary>
              <ul>{#each report.results.filter((result) => result.status === 'unknown') as result, index (index)}<li data-upload-result={result.path}><span class="upload-result-path">{result.path}</span><span>{uploadResultLabel(result, false)}</span>{#if result.status === 'unknown' && result.kind === 'folder'}<Button variant="outline" size="sm" onclick={() => inspectUploadDestination(result)}>Inspect destination</Button>{#if result.inspectionError}<span role="alert">{result.inspectionError}</span>{/if}{/if}</li>{/each}</ul>
            </details>{/if}
            {#if uploadReportDismissible(report.results)}<Button variant="ghost" size="icon" aria-label="Dismiss checked results" title="Dismiss checked results" onclick={() => dismissUploadReport(report.id)}><XIcon /></Button>{/if}
          </section>
        {/each}
        {#if routeFeedback}
          <p id="route-status" class="workspace-feedback" role="status" tabindex="-1">
            {routeName === 'trash' ? 'Opening Trash…' : routeName === 'account' ? 'Loading storage use…' : routeName === 'settings' ? 'Opening Settings…' : routeName === 'libraries' ? 'Opening My libraries…' : 'Opening files…'}
          </p>
        {/if}
        {#if accountView}
          <section class="account-content" inert={retainingRoute}>
            <h1 id="account-title" tabindex="-1">Account</h1>
            <section class="settings-section"><h2>Your account</h2><dl class="account-details"><dt>Account name</dt><dd>{user.handle}</dd><dt>Role</dt><dd>{user.role === 'administrator' ? 'Administrator' : 'User'}</dd></dl></section>
            <section class="settings-section" aria-labelledby="usage-heading" aria-busy={routeName === 'account' || ownUsageState === 'loading'}><h2 id="usage-heading" tabindex="-1">Storage</h2><p class="section-description">Total size of your files. Trash includes files replaced by uploads.</p><dl class="account-details usage-details"><dt>Files</dt><dd>{ownUsageState === 'loading' && pendingRoute && !routeFeedback ? '—' : usageLabel(ownUsageState, ownUsage?.files_bytes)}</dd><dt>Trash</dt><dd>{ownUsageState === 'loading' && pendingRoute && !routeFeedback ? '—' : usageLabel(ownUsageState, ownUsage?.trash_bytes)}</dd></dl>{#if ownUsageState === 'error'}<p class="form-notice" role="alert">Could not load storage use.</p>{/if}{#if ownUsageState === 'error' || ownUsageRetrying}<RefreshButton variant="outline" class="mt-3" label="Try again" busyLabel="Loading…" keepFocus busy={ownUsageRetrying} onclick={() => void retryOwnUsage()} />{/if}</section>
            <section class="settings-section account-setting-row"><div><h2>Public links</h2><p class="section-description">Review and revoke public links to files and folders.</p></div><Button variant="outline" onclick={(event) => { share = { entry: null, trigger: event.currentTarget }; }}>Shared links</Button></section>
            {#if webdavAvailable && !user.external}
              <section class="settings-section account-setting-row">
                <div><h2>WebDAV</h2><p class="section-description">Connect a desktop client to your private libraries.</p></div>
                <Button type="button" variant="outline" onclick={(event) => { webdavTrigger = event.currentTarget; webdavOpen = true; }}>Manage app passwords</Button>
              </section>
            {/if}
            {#if !user.external}
            <section class="settings-section account-setting-row"><div><h2>Password</h2><p class="section-description">Change the password you use to sign in.</p></div><Button variant="outline" onclick={(event) => { passwordDialogTrigger = event.currentTarget; accountNotice = ''; currentPassword = ''; newPassword = ''; changePasswordOpen = true; }}>Change password</Button></section>
              {#if oidcEnabled && user.role === 'user'}
                <section class="settings-section account-setting-row"><div><h2>Keycloak sign-in</h2><p class="section-description">Ask a Sylux administrator to approve the switch first.</p></div><Button variant="outline" onclick={(event) => { conversionTrigger = event.currentTarget; conversionOpen = true; }}>Switch to Keycloak</Button></section>
              {/if}
            {:else}
              <section class="settings-section"><h2>Keycloak sign-in</h2><p class="section-description">Sign in again when your 15-minute session ends.</p></section>
            {/if}
            <section class="settings-section account-setting-row"><div><h2>Session</h2><p class="section-description">Sign out of this browser.</p></div><Button variant="outline" disabled={loggingOut} onclick={signOut}>Sign out</Button></section>
            {#if accountNotice && !changePasswordOpen}<p role="status" class="form-notice">{accountNotice}</p>{/if}
          </section>
        {:else if settingsView && user.role === 'administrator'}
          <section class="settings-content" inert={retainingRoute}>
            <h1 id="settings-title" tabindex="-1">Settings</h1>
            <nav class="settings-tabs" aria-label="Settings"><Button disabled={instanceBusy} variant="ghost" class={settingsTab === 'accounts' ? 'active' : ''} aria-current={settingsTab === 'accounts' ? 'page' : undefined} onclick={() => { clearCompletedFeedback(); settingsTab = 'accounts'; }}>Accounts</Button><Button disabled={instanceBusy} variant="ghost" class={settingsTab === 'instance' ? 'active' : ''} aria-current={settingsTab === 'instance' ? 'page' : undefined} onclick={() => { clearCompletedFeedback(); settingsTab = 'instance'; }}>Instance</Button><Button disabled={instanceBusy} variant="ghost" class={settingsTab === 'appearance' ? 'active' : ''} aria-current={settingsTab === 'appearance' ? 'page' : undefined} onclick={() => { clearCompletedFeedback(); settingsTab = 'appearance'; }}>Appearance</Button></nav>
            {#if settingsTab === 'accounts'}
              <section class="settings-section"><div class="section-heading"><div><h2 id="accounts-heading" tabindex="-1">Accounts</h2><p class="volume-space" aria-busy={allUsageState === 'loading'}>Data volume: <span class="volume-value">{allUsageState === 'loading' ? 'Loading…' : allUsage?.volume ? `${formatSize(allUsage.volume.available_bytes)} free of ${formatSize(allUsage.volume.total_bytes)}` : 'Unknown'}</span></p></div><div class="flex flex-wrap gap-2"><RefreshButton label="Refresh" busyLabel="Refreshing…" busy={accountsLoading} disabled={accountBusy} onclick={() => { if (accountsLoading || accountBusy) return; clearCompletedFeedback(); accountNotice = ''; void loadAccounts(); }} /><Button size="sm" disabled={accountBusy} onclick={(event) => { accountDialogTrigger = event.currentTarget; accountNotice = ''; createAccountOpen = true; }}>Create account</Button></div></div>
                {#if accountsLoading}<p role="status">Loading accounts…</p>{:else if accountsError}<p role="alert">{accountsError}</p>{:else if accounts.length === 0}<p>No accounts to show.</p>{/if}
                <div role="table" aria-label="Accounts"><div class="account-sort account-head" role="row"><SortHeader label="Name" column="name" sort={accountSort} disabled={accountBusy || accountsLoading} onclick={() => setAccountSort('name')} /><SortHeader label="Role" column="role" sort={accountSort} disabled={accountBusy || accountsLoading} onclick={() => setAccountSort('role')} /><SortHeader label="Status" column="status" sort={accountSort} disabled={accountBusy || accountsLoading} onclick={() => setAccountSort('status')} /><span role="columnheader" class="account-usage">Files</span><span role="columnheader" class="account-usage">Trash</span><span role="columnheader"><span class="sr-only">Actions</span></span></div>
                <div role="rowgroup" class="account-list">{#each sortedAccounts as account (account.account_id)}
                  <div class="account-row" role="row" aria-label={account.handle}>
                    <div class="account-identity" role="cell"><strong>{account.handle}</strong>{#if account.account_id === user?.id}<span class="account-you">You</span>{/if}<span class="account-usage-summary" aria-hidden="true">Files {usageLabel(allUsageState, usageByAccount.get(account.account_id)?.files_bytes)} · Trash {usageLabel(allUsageState, usageByAccount.get(account.account_id)?.trash_bytes)}</span></div><span role="cell" class="account-role">{account.role === 'administrator' ? 'Administrator' : 'User'}</span>
                    <span role="cell"><AccountStatus enabled={account.enabled} /></span>
                    <span role="cell" class="account-usage">{usageLabel(allUsageState, usageByAccount.get(account.account_id)?.files_bytes)}</span><span role="cell" class="account-usage">{usageLabel(allUsageState, usageByAccount.get(account.account_id)?.trash_bytes)}</span>
                    <div role="cell" class="account-actions"><AccountOptions {account} self={account.account_id === user?.id} lastAdministrator={account.enabled && account.role === 'administrator' && enabledAdministratorCount === 1} disabled={accountBusy} onaction={async (action, trigger) => {
                      accountDialogTrigger = trigger; accountNotice = '';
                      if (action === 'personal') { await navigate('account'); }
                      else if (action === 'reset') { resetId = account.account_id; resetPassword = ''; }
                      else if (action === 'disable') disableTarget = account;
                      else authorityTarget = { account, action };
                    }} /></div>
                  </div>
                {/each}</div></div>
              </section>
              {#if accountNotice && !createAccountOpen && !resetId && !disableTarget && !authorityTarget}<p role="status" class="form-notice">{accountNotice}</p>{/if}
              <OidcApprovals {session} {accounts} enabled={oidcEnabled} onrevoked={loadAccounts} />
            {:else if settingsTab === 'appearance'}
              <AppearanceSettings {client} {instanceName} dark={darkMode} bind:busy={appearanceBusy} bind:error={appearanceError} onsaved={value => { appearanceRequest++; appearanceLoading = false; appearance = value; appearanceLoadError = ''; }} />
            {:else}
              <section class="settings-section"><h2>Instance</h2><form class="settings-form grid gap-3" onsubmit={saveInstance}><Label for="instance-name">Instance name</Label><Input id="instance-name" bind:value={instanceDraft} maxlength={40} disabled={instanceBusy || instanceLoading} required /><Button type="submit" class="justify-self-start mt-2" disabled={instanceBusy || instanceLoading || !instanceDraft.trim()}>{instanceBusy ? 'Saving…' : 'Save changes'}</Button></form>{#if instanceError}<p class="form-notice" role="alert">{instanceError}</p>{/if}{#if instanceError || instanceLoading}<RefreshButton variant="link" label="Reload instance name" busyLabel="Reloading…" busy={instanceLoading} class="mt-2" aria-label="Reload instance name" onclick={() => { if (instanceLoading) return; clearCompletedFeedback(); void loadInstance(); }} />{/if}{#if instanceLoading}<p role="status">Reloading instance name…</p>{/if}{#if instanceNotice}<p class="form-notice" role="status">{instanceNotice}</p>{/if}</section>
            {/if}
          </section>
        {:else if librariesView}
          <section class="libraries-content grid gap-4" aria-labelledby="libraries-title" inert={retainingRoute} aria-busy={restorationPending}>
            <LibraryHome items={libraries} {publicFolderIds} pending={restorationPending} error={librariesError}
              onopen={(folder) => void navigate('files', [folder])}
              onretry={() => void navigate('libraries')} oncreate={createLibrary}
              onshare={(folder, trigger) => directShare = { folder, trigger }}
              onpubliclink={(folder, trigger) => share = { entry: folder, trigger }} />
            {@render publicLinksStatus()}
            {#if restorationPending && !presentedRoute}{@render routePlaceholder()}{/if}
          </section>
        {:else if sharedView}
          <SharedWorkspace {client} destination={sharedView} location={sharedLocation} onnavigate={navigateShared} />
        {:else if showTrash}
            {@const trashEmpty = !restorationPending && !trashLoading && !trashError && deletedItems.length === 0}
            <section class="trash-content grid gap-4" aria-labelledby="trash-heading" inert={retainingRoute} aria-busy={trashLoading || restorationPending}>
              <div class="trash-heading-row"><h1 id="trash-heading" tabindex="-1">Trash</h1><div class="flex flex-wrap justify-end gap-2"><RefreshButton label="Refresh trash" busyLabel="Refreshing trash…" busy={trashLoading} disabled={restorationPending || trashBusy || removalSubmitting} onclick={refreshWorkspace} /><Button variant="outline" size="sm" disabled={removalBlocked || !deletedItems.length} aria-busy={removalPreparing === 'all'} onclick={(event) => prepareRemoval(true, event.currentTarget)}>{removalPreparing === 'all' ? 'Preparing…' : 'Empty Trash'}</Button></div></div>
              {#if removalError}<p role="alert">{removalError}</p>{/if}
              {#if deletedItems.length}<div class="flex flex-wrap items-center gap-3"><label class="flex items-center gap-2"><input type="checkbox" aria-label="Select all items on this page" disabled={removalBlocked} checked={!!deletedItems.length && removalSelection.length === deletedItems.length} onchange={(event) => removalSelection = event.currentTarget.checked ? deletedItems.map(item => item.id) : []} />Select page</label>{#if removalSelection.length}<span>{removalSelection.length} selected</span><Button variant="destructive" size="sm" disabled={removalBlocked} aria-busy={removalPreparing === 'selected'} onclick={(event) => prepareRemoval(false, event.currentTarget)}>{removalPreparing === 'selected' ? 'Preparing…' : 'Remove permanently'}</Button>{/if}</div>{/if}
              {#if trashNotice}<p role="status">{trashNotice}</p>{/if}
              {#if trashError}<p role="alert">{trashError}</p>{#if trashRetry}<Button id="trash-page-retry" variant="outline" class="justify-self-start" disabled={trashLoading || trashBusy} aria-busy={trashPaging === 'retry' && trashLoading} onclick={() => pageTrash('retry')}>{trashPaging === 'retry' && trashLoading ? 'Retrying…' : 'Try again'}</Button>{/if}{/if}
              {#if restorationPending && !presentedRoute}{@render routePlaceholder()}{:else if trashLoading && !restorationPending}<p role="status">Loading trash…</p>{/if}
              <p id="trash-empty-status" class="sr-only" aria-live="polite">{trashEmpty ? 'Trash is empty.' : ''}</p>
              <div class="narrow-sort"><Label for="trash-sort">Sort Trash by</Label><select id="trash-sort" aria-busy={trashLoading} value={trashSort} disabled={resultSortBlocked} onchange={(event) => setResultSort('trash', event.currentTarget.value as ResultSort)}>{@render resultSortOptions()}</select></div>
              <div role="table" aria-label="Trash items" aria-describedby={trashEmpty ? 'trash-empty-status' : undefined}>
                <div class="trash-sort" role="row">
                  <SortHeader label="Name" column="name" sort={trashSort} busy={trashLoading && trashSort.startsWith('name_')} disabled={resultSortBlocked} onclick={() => setResultSort('trash', nextSort(trashSort, 'name'))} />
                  <SortHeader label="Location" column="location" sort={trashSort} busy={trashLoading && trashSort.startsWith('location_')} disabled={resultSortBlocked} onclick={() => setResultSort('trash', nextSort(trashSort, 'location'))} />
                  <SortHeader label="Modified" column="modified" sort={trashSort} busy={trashLoading && trashSort.startsWith('modified_')} disabled={resultSortBlocked} onclick={() => setResultSort('trash', nextSort(trashSort, 'modified'))} />
                  <SortHeader label="Size" column="size" class="trash-size-sort" sort={trashSort} busy={trashLoading && trashSort.startsWith('size_')} disabled={resultSortBlocked} onclick={() => setResultSort('trash', nextSort(trashSort, 'size'))} />
                  <span role="columnheader" aria-label="Selection"></span>
                  <span role="columnheader"><span class="sr-only">Actions</span></span>
                </div>
                {#if deletedItems.length > 0}
                  <ul role="rowgroup">
                    {#each deletedItems as item (item.id)}
                      <li class="trash-row" role="row" aria-label={item.name}>
                        <span role="cell" class="trash-item">
                          <span class="trash-name"><span class="trash-kind" class:folder-icon={item.kind === 'folder'} role="img" aria-label={item.kind}>{#if item.kind === 'folder'}<FolderIcon />{:else}<FileIcon />{/if}</span><span>{item.name}</span></span>
                          <span class="trash-metadata" aria-hidden="true">{@render modifiedDateLabel(item)} · {typeof item.size === 'number' ? formatSize(item.size) : 'Size unknown'}</span>
                        </span>
                        <span role="cell" class="trash-location">{trashLocation(item)}{#if needsDeletedReference(item)}<span class="trash-location">Item reference: {item.id}</span>{/if}</span>
                        <span role="cell" class="file-modified">{@render modifiedDateLabel(item)}</span>
                        <span role="cell" class="file-size">{typeof item.size === 'number' ? formatSize(item.size) : 'Size unknown'}</span>
                        <span role="cell" class="trash-selection"><label><input type="checkbox" aria-label={`Select ${item.name}${needsDeletedReference(item) ? `, item reference ${item.id}` : ''}`} disabled={removalBlocked} checked={removalSelection.includes(item.id)} onchange={(event) => removalSelection = event.currentTarget.checked ? [...removalSelection, item.id] : removalSelection.filter(id => id !== item.id)} /></label></span>
                        <div role="cell" class="trash-actions"><Button variant="outline" size="sm" disabled={removalBlocked} aria-busy={trashBusy && restoringId === item.id} aria-label={item.needs_reference || deletedItems.some((other) => other.id !== item.id && other.name === item.name) ? `Restore ${item.name} from ${trashLocation(item)}${needsDeletedReference(item) ? `, item reference ${item.id}` : ''}` : `Restore ${item.name}`} onclick={(event) => restoreEntry(item, event.currentTarget)}>{trashBusy && restoringId === item.id ? 'Restoring…' : 'Restore'}</Button></div>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>
              {#if trashPageIndex > 0 || trashHasMore}
                <nav class="flex flex-wrap gap-2" aria-label="Trash pages">
                  {#if trashPageIndex > 0}<Button variant="outline" size="sm" disabled={trashLoading || trashBusy} aria-label="Previous page" aria-busy={trashLoading && trashPaging === 'previous'} onclick={() => pageTrash('previous')}>{trashLoading && trashPaging === 'previous' ? 'Loading…' : 'Previous page'}</Button>{/if}
                  {#if trashHasMore}<Button variant="outline" size="sm" disabled={trashLoading || trashBusy} aria-label="Next page" aria-busy={trashLoading && trashPaging === 'next'} onclick={() => pageTrash('next')}>{trashLoading && trashPaging === 'next' ? 'Loading…' : 'Next page'}</Button>{/if}
                </nav>
              {/if}
            </section>

        {:else}
          <section class="files-content" aria-labelledby="folder-title" inert={retainingRoute} aria-busy={restorationPending}>


            <div class="page-heading">
              {#if folderPath.length > 1}<nav class="breadcrumb" aria-label="Folder path">{#each folderPath.slice(0, -1) as folder (folder.id)}<Button variant="link" class="h-auto p-0 break-all whitespace-normal" onclick={() => openFolder(folder)}>{folder.name}</Button><span aria-hidden="true">/</span>{/each}</nav>{/if}
              <h1 id="folder-title" tabindex="-1">{currentFolder.name}</h1>
              {#if publicFolderIds.has(currentFolder.id)}
                <Button variant="link" size="sm" class="px-0" aria-label={`Public link active for ${currentFolder.name}. Manage public links`} onclick={(event) => { share = { entry: currentFolder, trigger: event.currentTarget }; }}>Public link active</Button>
              {/if}
            </div>
            <div class="search-toolbar">
              <div class="search-controls"><Input id="file-search" aria-label="Search files and folders" disabled={bulkBusy || (restorationPending && !searchDuringRoute) || restorationFailed} type="search" placeholder={folderPath.length > 1 ? 'Search this folder by name' : namedLibrary ? 'Search this library by name' : 'Search all libraries by name'} maxlength={255} bind:value={searchInput} oninput={(event) => { searchInput = event.currentTarget.value; searchChanged(); }} onkeydown={searchKeydown}
                  oncompositionstart={searchCompositionStarted} oncompositionend={(event) => { searchComposing = false; searchInput = event.currentTarget.value; searchChanged(); }} />
                {#if searchQuery}<Button variant="outline" type="button" disabled={bulkBusy || restorationPending || restorationFailed} onclick={clearSearchFromButton}>Clear search</Button>{/if}
              </div>
            </div>
            {#if searchQuery}
              <section aria-label="Search results" aria-busy={searchLoading} class="search-results">
                <div class="narrow-sort"><Label for="search-sort">Sort search by</Label><select id="search-sort" aria-busy={searchLoading} value={searchSort} disabled={resultSortBlocked} onchange={(event) => setResultSort('search', event.currentTarget.value as ResultSort)}>{@render resultSortOptions()}</select></div>
                {#if searchLoading && !restorationPending}<p role="status">Searching…</p>
                {:else if searchInputError}<p role="alert">{searchInputError}</p>
                {:else if searchError}<Alert.Root variant="destructive" role="alert"><Alert.Title>Search failed</Alert.Title><Alert.Description><p>{searchError}</p><Button id="search-page-retry" variant="outline" size="sm" class="mt-2" onclick={() => runSearch(searchRetryCursor, searchRetryPage)}>Try again</Button></Alert.Description></Alert.Root>
                {:else if searchItems.length === 0}<p>No matching files or folders.</p>
                {/if}
                <div role="table" aria-label="Search matches"><div role="rowgroup" class="file-list"><div role="row" class="list-head selectable-head modified-head search-head">{@render selectionHeader(searchLoading || !!searchInputError || !!searchError)}<SortHeader label="Name" column="name" sort={searchSort} busy={searchLoading && searchSort.startsWith('name_')} disabled={resultSortBlocked} onclick={() => setResultSort('search', nextSort(searchSort, 'name'))} /><SortHeader label="Location" column="location" class="file-location" sort={searchSort} busy={searchLoading && searchSort.startsWith('location_')} disabled={resultSortBlocked} onclick={() => setResultSort('search', nextSort(searchSort, 'location'))} /><SortHeader label="Modified" column="modified" sort={searchSort} busy={searchLoading && searchSort.startsWith('modified_')} class="file-modified" disabled={resultSortBlocked} onclick={() => setResultSort('search', nextSort(searchSort, 'modified'))} /><SortHeader label="Size" column="size" sort={searchSort} busy={searchLoading && searchSort.startsWith('size_')} class="file-size" disabled={resultSortBlocked} onclick={() => setResultSort('search', nextSort(searchSort, 'size'))} /><span role="columnheader"><span class="sr-only">Options</span></span></div></div>
                {#if selectedEntries.length}<div role="row"><div role="cell" aria-colspan="6">{@render selectionToolbar()}</div></div>{/if}
                <div role="rowgroup" class="file-list">
                {#if !searchLoading && !searchInputError && !searchError}
                  {#each searchItems as hit (hit.entry.id)}
                    <div role="row" aria-label={hit.entry.name} data-entry-id={hit.entry.id} class={`file-row search-row selectable-row modified-row ${selectedIds.includes(hit.entry.id) ? 'selected-row' : ''}`}>
                      <div role="cell" class="entry-selection"><label><input type="checkbox" aria-label={`Select ${hit.entry.name} in ${searchLocation(hit)}`} checked={selectedIds.includes(hit.entry.id)} disabled={fileMutationBusy} onclick={(event) => selectEntry(hit.entry.id, event.currentTarget.checked, event.shiftKey)} onkeydown={(event) => selectionKeydown(event, hit.entry.id)} /></label></div>
                      <div role="cell" class="search-item-main name-cell"><button type="button" class="item-name" disabled={bulkBusy} title={hit.entry.name} aria-label={`${hit.entry.kind === 'folder' ? 'Open' : 'Preview'} ${hit.entry.name}`} aria-describedby={publicFolderIds.has(hit.entry.id) ? `search-row-public-link-${hit.entry.id}` : undefined} onclick={(event) => hit.entry.kind === 'folder' ? openSearchFolder(hit) : openPreview(hit.entry, event.currentTarget)}><span class:folder-icon={hit.entry.kind === 'folder'} aria-hidden="true">{#if hit.entry.kind === 'folder'}<FolderIcon />{:else}<FileIcon />{/if}</span><span class="item-label"><span class="filename">{hit.entry.name}</span>{#if publicFolderIds.has(hit.entry.id)}<span id={`search-row-public-link-${hit.entry.id}`} class="text-xs text-muted-foreground">Public link active</span>{/if}<span class="mobile-file-size" aria-hidden="true">{typeof hit.entry.size === 'number' ? formatSize(hit.entry.size) : 'Size unknown'} · {@render modifiedDateLabel(hit.entry)}</span></span></button></div>
                        <div role="cell" class="search-location file-location"><span>{searchLocation(hit)}</span><Button variant="link" size="sm" disabled={bulkBusy} onclick={() => openSearchFolder(hit, true)} aria-label={`Open containing folder for ${hit.entry.name}`}>Open folder</Button></div>
                      <span role="cell" class="file-modified">{@render modifiedDateLabel(hit.entry)}</span><span role="cell" class="file-size">{hit.entry.kind === 'folder' ? '—' : typeof hit.entry.size === 'number' ? formatSize(hit.entry.size) : '—'}</span>
                      <div role="cell" class="options-cell"><EntryOptions entry={hit.entry} disabled={bulkBusy || bulkNeedsRefresh || !!dropPending || moveBusy} moveDisabled={moveNeedsRefresh} copyDisabled={moveNeedsRefresh || !!pendingCopy} downloadDisabled={archiveBusy} downloading={archiveBusy && archiveTargets.includes(hit.entry.id)} onaction={(action, trigger) => entryAction(action, hit.entry, trigger)} /></div>
                    </div>
                  {/each}
                {/if}</div></div>
                {#if searchPageIndex > 0 || searchHasMore}<div class="flex flex-wrap gap-2 mt-3">{#if searchPageIndex > 0}<Button variant="outline" size="sm" onclick={searchPrevious} disabled={searchLoading || bulkBusy}>Previous page</Button>{/if}{#if searchHasMore}<Button variant="outline" size="sm" onclick={searchNext} disabled={searchLoading || bulkBusy}>Next page</Button>{/if}</div>{/if}
                {#if !searchLoading && !searchError && !searchInputError}<p class="list-summary">{searchItems.length} {searchItems.length === 1 ? 'result' : 'results'}{searchHasMore || searchPageIndex > 0 ? ' on this page' : ''}</p>{/if}
              </section>
            {:else}
            <section class="upload-toolbar" aria-label="Upload files">
              <Button variant="outline" disabled={fileMutationBusy || !currentFolder.id} onclick={(event) => beginCreate(event.currentTarget as HTMLElement)}><PlusIcon />New folder</Button>
              <div class="upload-form">
                <input class="sr-only" id="upload-file" type="file" multiple tabindex={-1} aria-label="Choose files to upload" disabled={fileMutationBusy || !!unresolvedUploadId || !!replacementPrompt} onchange={chooseUploads} />
                <Button id="choose-file" variant="outline" type="button" disabled={fileMutationBusy || !!unresolvedUploadId || !!replacementPrompt} onclick={() => document.getElementById('upload-file')?.click()}>Choose files</Button>
                {#if folderChooserSupported}
                  <input class="sr-only" id="upload-folder" type="file" webkitdirectory multiple tabindex={-1} aria-label="Choose folder to upload" disabled={fileMutationBusy || !!unresolvedUploadId || !!replacementPrompt} onchange={chooseFolderUploads} />
                  <Button id="choose-folder" variant="outline" type="button" disabled={fileMutationBusy || !!unresolvedUploadId || !!replacementPrompt} onclick={() => document.getElementById('upload-folder')?.click()}>Choose folder</Button>
                {/if}
              </div>
              <RefreshButton size="default" label="Refresh" busyLabel="Refreshing…" busy={loading || searchLoading} disabled={restorationPending || bulkBusy || bulkNeedsRefresh} onclick={() => { if (restorationPending || loading || searchLoading || bulkBusy || bulkNeedsRefresh) return; void refreshWorkspace(); }} aria-label="Refresh folder" />
            </section>
            {#if !folderChooserSupported}<p id="folder-upload-help" class="text-xs text-muted-foreground mb-3">Folder selection is unavailable in this browser. Choose files or drop a folder instead.</p>{/if}
            {#if currentSortAttempt?.failed}<p class="mb-2 text-sm text-destructive" role="alert">Could not sort files. Choose a column to try again.</p>{/if}
            <section class="file-drop-region" role="table" aria-label="Files in current folder" ondragover={(event) => dragOver(event, currentFolder)} ondragleave={(event) => dragLeave(event, currentFolder)} ondrop={(event) => dropEntry(event, currentFolder)} data-drop-target="current-folder" data-drop-active={dropTarget === currentFolder.id}>
            {#if !listingError && entriesFolderId && entriesFolderId === currentFolder.id && (entries.length || !loading)}
              <div role="rowgroup" class="file-list">
                <div role="row" class="list-head selectable-head modified-head folder-sort-head">
                  {@render selectionHeader(loading)}
                  <SortHeader label="Name" column="name" sort={folderSort} pendingSort={pendingFolderSort} disabled={folderSortBlocked} onclick={() => sortFolderBy('name')} />
                  <SortHeader label="Modified" column="modified" sort={folderSort} pendingSort={pendingFolderSort} disabled={folderSortBlocked} onclick={() => sortFolderBy('modified')} />
                  <SortHeader label="Size" column="size" sort={folderSort} pendingSort={pendingFolderSort} disabled={folderSortBlocked} onclick={() => sortFolderBy('size')} />
                  <span role="columnheader"><span class="sr-only">Options</span></span>
                </div>
              </div>
            {/if}
            {#if selectedEntries.length}<div role="row"><div role="cell" aria-colspan="5">{@render selectionToolbar()}</div></div>{/if}
            {#if listingError || currentFolderRetryBusy}<Alert.Root variant="destructive" role={currentFolderRetryBusy ? undefined : 'alert'}><Alert.Title>Could not open this folder</Alert.Title><Alert.Description>{#if listingError}<p>{listingError}</p>{/if}{#if restorationUnavailable}<Button variant="outline" class="mt-2" onclick={() => navigate('files', folders[0] ? [folders[0]] : undefined)}>Open Files</Button>{:else}<RefreshButton id="folder-page-retry" label="Try again" busyLabel="Trying again…" busy={currentFolderRetryBusy || (restorationPending && !restorationFailed)} class="mt-2" onclick={retryFolder} />{/if}{#if currentFolderRetryBusy}<p role="status">Loading files…</p>{/if}</Alert.Description></Alert.Root>
            {:else if restorationPending && (!presentedRoute || searchDuringRoute)}{@render routePlaceholder()}
            {:else if loading && !restorationPending && !(workspaceRefreshing && refreshSourceFolder === currentFolder.id && entriesFolderId === currentFolder.id && entries.length)}<div class="flex flex-col items-center gap-1 rounded-lg border border-dashed px-4 py-10 text-center" role="status"><strong class="font-medium">Loading files…</strong></div>
            {:else if entries.length === 0}<div class="flex flex-col items-center gap-1 rounded-lg border border-dashed px-4 py-10 text-center"><strong class="font-medium">This folder is empty</strong><p class="text-muted-foreground text-sm">Upload a file or create a folder.</p></div>
            {:else}<div role="rowgroup" class="file-list">{#each entries as entry (entry.id)}
              <div role="row" tabindex="-1" aria-label={entry.name} data-entry-id={entry.id} draggable={!fileMutationBusy && !moveNeedsRefresh} ondragstart={(event) => startDrag(event, entry)} ondragend={endDrag} ondragover={(event) => entry.kind === 'folder' && dragOver(event, entry)} ondragleave={(event) => entry.kind === 'folder' && dragLeave(event, entry)} ondrop={(event) => entry.kind === 'folder' && dropEntry(event, entry)} data-drop-target={entry.kind === 'folder' ? entry.id : undefined} data-drop-active={dropTarget === entry.id} class={`file-row selectable-row modified-row ${selectedIds.includes(entry.id) ? 'selected-row' : ''} ${dropTarget === entry.id ? 'drop-active' : ''}`}>
                <div role="cell" class="entry-selection"><label><input type="checkbox" aria-label={`Select ${entry.name}`} checked={selectedIds.includes(entry.id)} disabled={fileMutationBusy} onclick={(event) => selectEntry(entry.id, event.currentTarget.checked, event.shiftKey)} onkeydown={(event) => selectionKeydown(event, entry.id)} /></label></div>
                <div role="cell" class="name-cell"><button type="button" class="item-name" title={entry.name} aria-label={`${entry.kind === 'folder' ? 'Open' : 'Preview'} ${entry.name}`} aria-describedby={publicFolderIds.has(entry.id) ? `folder-row-public-link-${entry.id}` : undefined} disabled={loading || bulkBusy} onclick={(event) => entry.kind === 'folder' ? openFolder(entry) : openPreview(entry, event.currentTarget)}><span class:folder-icon={entry.kind === 'folder'} aria-hidden="true">{#if entry.kind === 'folder'}<FolderIcon />{:else}<FileIcon />{/if}</span><span class="item-label"><span class="filename">{entry.name}</span>{#if publicFolderIds.has(entry.id)}<span id={`folder-row-public-link-${entry.id}`} class="text-xs text-muted-foreground">Public link active</span>{/if}<span class="mobile-file-size" aria-hidden="true">{typeof entry.size === 'number' && Number.isFinite(entry.size) ? formatSize(entry.size) : 'Size unknown'} · {@render modifiedDateLabel(entry)}</span></span></button></div><span role="cell" class="file-modified">{@render modifiedDateLabel(entry)}</span><span role="cell" class="file-size">{typeof entry.size === 'number' && Number.isFinite(entry.size) ? formatSize(entry.size) : 'Size unknown'}</span>
                <div role="cell" class="options-cell"><EntryOptions {entry} disabled={loading || bulkBusy || bulkNeedsRefresh || !!dropPending || moveBusy} moveDisabled={moveNeedsRefresh} copyDisabled={moveNeedsRefresh || !!pendingCopy} downloadDisabled={archiveBusy} downloading={archiveBusy && archiveTargets.includes(entry.id)} onaction={(action, trigger) => entryAction(action, entry, trigger)} /></div>
              </div>
            {/each}</div>{/if}
            </section>
            {#if dropPending}<p role="status">Moving {dropPending.entry.name} to {dropPending.destination.name}…</p>{/if}
            {#if dropError}<p role="alert">{dropError}</p>{/if}
            {#if pageIndex > 0 || hasMore}<div class="flex flex-wrap gap-2">{#if pageIndex > 0}<Button variant="outline" size="sm" onclick={loadPrevious} disabled={loading || bulkBusy} class="aria-disabled:opacity-50" aria-disabled={!!pendingFolderSort || undefined}>Previous page</Button>{/if}{#if hasMore}<Button variant="outline" size="sm" onclick={loadMore} disabled={loading || bulkBusy} class="aria-disabled:opacity-50" aria-disabled={!!pendingFolderSort || undefined}>Next page</Button>{/if}</div>{/if}
            {#if (presentedRoute || !restorationPending) && !searchDuringRoute && !listingError}
              <p class="list-summary" role={loading && workspaceRefreshing && entries.length ? 'status' : undefined}>{loading && !restorationPending ? 'Loading…' : `${entries.length} ${entries.length === 1 ? 'item' : 'items'}${hasMore || pageIndex > 0 ? ' on this page' : ''}`}</p>
            {/if}
            {/if}
            {#if notice}<Alert.Root role="status" class="workspace-notice"><Alert.Description>{notice}</Alert.Description></Alert.Root>{/if}
            {@render publicLinksStatus()}
          </section>
        {/if}
        </div>
        {@render betaNotice(!accountView && !settingsView && !showTrash && !sharedView)}
      </Sidebar.Inset>
    </Sidebar.Provider>
    <Dialog.Root open={createOpen} onOpenChange={(open) => { if (!folderBusy) createOpen = open; }}>
      <Dialog.Content showCloseButton={!folderBusy} onOpenAutoFocus={(event) => { event.preventDefault(); folderInput?.focus(); }} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(createTrigger); createTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Create folder</Dialog.Title><Dialog.Description>Add a folder to {currentFolder.name}.</Dialog.Description></Dialog.Header>
        <form class="grid gap-6" aria-busy={folderBusy} onsubmit={createFolder}><div class="grid gap-2"><Label for="new-folder">Folder name</Label><Input id="new-folder" bind:ref={folderInput} bind:value={folderName} disabled={folderBusy} maxlength={255} aria-invalid={!!folderError} aria-describedby={folderError ? 'folder-error' : undefined} required /></div><PublicFolderNotice status={createAudience.status} subject="The new folder" />{#if folderError}<p id="folder-error" role="alert">{folderError}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={folderBusy} onclick={() => createOpen = false}>Cancel</Button><Button type="submit" disabled={folderBusy || createAudience.pending} aria-busy={folderBusy || createAudience.pending}>{folderBusy ? 'Creating…' : 'Create folder'}</Button></Dialog.Footer></form>
      </Dialog.Content>
    </Dialog.Root>
    {#key user.id}
      <WebDavPasswords {client} handle={user.handle} active={accountView}
        bind:available={webdavAvailable} bind:open={webdavOpen}
        onrestorefocus={() => restoreFocus(webdavTrigger, 'account-title')} />
    {/key}
    {#if share}<ShareLinks {client} entry={share.entry} bind:busy={shareBusy} onclose={closeShare} onchanged={folderLinksChanged} />{/if}
    {#if directShare}<DirectShareDialog {client} folder={directShare.folder} bind:busy={directShareBusy} onclose={closeDirectShare} />{/if}

    <Dialog.Root open={!!moveTarget || bulkAction === 'move'} onOpenChange={(open) => { if (!open && !destinationBusy) closeMove(); }}>
      <Dialog.Content
        bind:ref={moveDialog}
        showCloseButton={!destinationBusy}
        escapeKeydownBehavior={destinationBusy ? 'ignore' : 'close'}
        interactOutsideBehavior={destinationBusy ? 'ignore' : 'close'}
        class="max-h-[85dvh] overflow-y-auto"
        {...(destinationAction === 'move' ? { 'aria-describedby': undefined } : {})}
        onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(bulkTrigger ?? moveTrigger); moveTrigger = null; bulkTrigger = null; }}
      >
        <Dialog.Header>
          <Dialog.Title class="pr-8 [overflow-wrap:anywhere]">{destinationAction === 'copy' ? 'Copy' : 'Move'} {bulkAction === 'move' ? `${bulkTargets.length} selected items` : moveTarget?.name}</Dialog.Title>
          {#if destinationAction === 'copy'}
            <Dialog.Description>Current folder: {folderPath.map((folder) => folder.name).join(' / ')}. Choose a destination in your private storage.</Dialog.Description>
          {/if}
        </Dialog.Header>
        {#if movePath.length}<Button variant="link" class="justify-self-start p-0" disabled={destinationBusy || moveLoading} onclick={loadMoveLibraries}>Choose a library</Button>{/if}
        {#if movePath.length}
          <nav aria-label={destinationAction === 'copy' ? 'Copy destination path' : 'Move destination path'} class="flex flex-wrap items-center gap-x-2 gap-y-1">
            {#each movePath as folder, index (folder.id)}
              {#if index > 0}<ChevronRightIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />{/if}
              {#if destinationAction === 'move' && index === movePath.length - 1}
                <span tabindex="-1" class="min-w-0 font-medium [overflow-wrap:anywhere]" aria-current="location">{folder.name}</span>
              {:else}
                <Button variant="link" class="h-auto min-h-[var(--control-height)] max-w-full min-w-0 p-0 text-left whitespace-normal [overflow-wrap:anywhere]" disabled={moveCloseRefreshBusy || moveLoading || destinationBusy} aria-current={index === movePath.length - 1 ? 'page' : undefined} onclick={() => navigateMoveFolder(movePath.slice(0, index + 1))}>{folder.name}</Button>
              {/if}
            {/each}
          </nav>
        {/if}
        <div class="grid gap-1 rounded-lg border p-2" role="group" aria-label={movePath.length ? 'Destination folders' : 'Destination libraries'} aria-busy={moveLoading}>
          {#if moveLoading}
            <p role="status" class="px-2 py-1">Loading destinations…</p>
          {:else if moveDestinationReady && !destinationFolders.length && !moveCursor}
            <p class="text-muted-foreground px-2 py-1">{destinationAction === 'move' && moveAlreadyHere ? 'No other folders here.' : 'No folders here.'}</p>
          {/if}
          {#each destinationFolders as folder (folder.id)}
            <Button variant="ghost" class="h-auto min-h-9 min-w-0 justify-start text-left whitespace-normal [overflow-wrap:anywhere]" disabled={moveCloseRefreshBusy || moveLoading || destinationBusy || (destinationAction === 'copy' && folder.id === moveTarget?.id)} onclick={() => navigateMoveFolder([...movePath, folder])}>
              <FolderIcon class="size-4 shrink-0" />{folder.name}{destinationAction === 'copy' && folder.id === moveTarget?.id ? ' (cannot copy into itself)' : ''}
            </Button>
          {/each}
          {#if moveCursor}<Button variant="outline" disabled={moveCloseRefreshBusy || moveLoading || destinationBusy} onclick={() => loadMoveFolder(movePath, moveCursor)}>More destination folders</Button>{/if}
        </div>
        {#if destinationAction === 'copy'}
          <PublicFolderNotice status={copyAudience.status} subject="The copy" />
          <p class="text-sm break-all">Destination: {movePath.map((folder) => folder.name).join(' / ') || 'Choose a folder'}</p>
          {#if moveAlreadyHere}<p class="text-muted-foreground text-sm">This item is already here. Open another folder or choose an ancestor above.</p>{/if}
        {:else if moveDestinationReady && moveAlreadyHere && (destinationFolders.length || moveCursor)}
          <p class="text-muted-foreground">{bulkAction === 'move' ? 'The selected items are already here.' : 'This item is already here.'}</p>
        {/if}
        {#if moveError}<p role="alert">{moveError}</p>{/if}{#if moveNeedsRefresh || moveCloseRefreshBusy}<RefreshButton label="Close and refresh files" busyLabel="Refreshing files…" busy={moveCloseRefreshBusy} disabled={moveBusy || moveLoading} onclick={closeMoveAndRefresh} />{:else if moveError || moveLoading}<RefreshButton label="Refresh destinations" busyLabel="Refreshing destinations…" busy={moveLoading} disabled={moveBusy} onclick={() => movePath.length ? loadMoveFolder(movePath) : loadMoveLibraries()} />{/if}
        <Dialog.Footer>
          <Button variant="outline" disabled={destinationBusy} onclick={closeMove}>Cancel</Button>
          <Button disabled={!canConfirmMove || copyAudience.pending} aria-disabled={destinationBusy || undefined} aria-busy={destinationBusy || copyAudience.pending} class="aria-disabled:cursor-default" onclick={() => {
            if (!destinationBusy) return bulkAction === 'move' ? runBulk() : destinationAction === 'copy' ? copyEntry() : moveEntry();
          }}>
            {destinationAction === 'copy' ? (moveBusy ? 'Copying…' : 'Copy here') : (destinationBusy ? 'Moving…' : 'Move here')}
          </Button>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!treeSharingPrompt} onOpenChange={(open) => { if (!open) settleTreeSharing(false); }}>
      <Dialog.Content
        class="max-h-[85dvh] overflow-y-auto"
        onOpenAutoFocus={(event) => { event.preventDefault(); document.getElementById('cancel-tree-sharing')?.focus(); }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          const trigger = treeSharingFocus;
          treeSharingFocus = null;
          if (trigger?.isConnected && !trigger.matches(':disabled')) trigger.focus();
        }}>
        <Dialog.Header>
          <Dialog.Title class="[overflow-wrap:anywhere]">{treeSharingPrompt?.action === 'upload' ? 'Upload to' : treeSharingPrompt?.action === 'restore' ? 'Restore' : 'Move'} {treeSharingPrompt?.item.name}?</Dialog.Title>
          <Dialog.Description>
            {#if treeSharingPrompt?.action === 'upload'}
              {#if !treeSharingPrompt.audienceKnown}Could not check for public folder links. {/if}Anyone with a public link to this location can browse and download items you add.
            {:else if treeSharingPrompt?.changed}Sharing changed. Check who will have access.{:else if treeSharingPrompt?.destination}Move to {treeSharingPrompt.destination}.{:else}This item will return to its original folder.{/if}
          </Dialog.Description>
        </Dialog.Header>
        {#if treeSharingPrompt?.audience?.exposes_to_all}
          <p>Everyone signed in now or in the future will be able to open and download {treeSharingPrompt.item.kind === 'folder' ? 'files in this folder' : 'this file'}{treeSharingPrompt.action === 'restore' ? ' after restore' : ' in the new location'}.</p>
        {/if}
        {#if treeSharingPrompt?.audience?.exposes_to_public_links}
          <p>Anyone with a public link to this location will be able to {treeSharingPrompt.item.kind === 'folder' ? 'browse this folder and download its files' : 'download this file'}.</p>
        {/if}
        {#if treeSharingPrompt?.audience?.removes_access_for_all}
          <p>Access through Shared with all will end. Separate direct shares may still apply.</p>
        {/if}
        {#if treeSharingPrompt?.audience?.new_recipients.length && !treeSharingPrompt.audience.exposes_to_all}
          <div class="grid gap-1"><p>New access</p><ul class="list-disc pl-5 [overflow-wrap:anywhere]">
            {#each treeSharingPrompt.audience.new_recipients as handle (handle)}<li>{handle}</li>{/each}
          </ul></div>
        {/if}
        {#if treeSharingPrompt?.audience?.carried_recipients.length}
          <div class="grid gap-1"><p>Access that moves with this item</p><ul class="list-disc pl-5 [overflow-wrap:anywhere]">
            {#each treeSharingPrompt.audience.carried_recipients as handle (handle)}<li>{handle}</li>{/each}
          </ul></div>
        {/if}
        {#if treeSharingPrompt?.audience?.lost_recipients.length}
          <div class="grid gap-1"><p>Access from the original folder ends</p><ul class="list-disc pl-5 [overflow-wrap:anywhere]">
            {#each treeSharingPrompt.audience.lost_recipients as handle (handle)}<li>{handle}</li>{/each}
          </ul></div>
        {/if}
        <Dialog.Footer>
          <Button id="cancel-tree-sharing" variant="outline" onclick={() => settleTreeSharing(false)}>Cancel</Button>
          <Button onclick={() => settleTreeSharing(true)}>{treeSharingPrompt?.action === 'upload' ? 'Upload' : treeSharingPrompt?.action === 'restore' ? 'Restore and share' : treeSharingPrompt?.audience?.exposes_to_all || treeSharingPrompt?.audience?.exposes_to_public_links ? 'Move and share' : 'Confirm move'}</Button>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={bulkAction === 'trash'} onOpenChange={(open) => { if (!open) closeBulk(); }}>
      <Dialog.Content showCloseButton={!bulkBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(bulkTrigger); bulkTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Move {bulkTargets.length} selected items to trash?</Dialog.Title><Dialog.Description>Folders include their contents. You can restore these items later. Public links and direct shares rooted in the selected items or their contents are revoked permanently. Each selected item is handled separately; some items may stay in place.</Dialog.Description></Dialog.Header>
        <ul class="bulk-confirm-items">{#each bulkTargets as entry (entry.id)}<li>{entry.name}</li>{/each}</ul>
        <Dialog.Footer><Button variant="outline" disabled={bulkBusy} onclick={closeBulk}>Cancel</Button><Button disabled={bulkBusy} onclick={runBulk}>{bulkBusy ? 'Moving to trash…' : 'Move to trash'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!replacementPrompt} onOpenChange={(open) => { if (!open) dismissReplacement(); }}>
      <Dialog.Content onCloseAutoFocus={(event) => { event.preventDefault(); const choose = document.getElementById('choose-file') as HTMLButtonElement | null; restoreFocus(choose && !choose.disabled ? choose : null); }}>
        <Dialog.Header><Dialog.Title>Replace {replacementPrompt?.target.name}?</Dialog.Title><Dialog.Description>A file with this name already exists in {replacementPrompt?.folder.name}. The previous file will move to trash and stay there until restored. Rename the current file before restoring it. If this upload fails or is canceled before saving, the previous file stays unchanged.</Dialog.Description></Dialog.Header>
        <PublicFolderNotice status={replacementAudience.status} subject="The replacement file" />
        <Dialog.Footer><Button variant="outline" onclick={dismissReplacement}>Keep existing file</Button><Button disabled={replacementAudience.pending} aria-busy={replacementAudience.pending} onclick={confirmReplacement}>Replace file</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!removalPreview} onOpenChange={(open) => { if (!open && !removalSubmitting) removalPreview = null; }}>
      <Dialog.Content showCloseButton={!removalSubmitting} aria-busy={removalSubmitting} onEscapeKeydown={(event) => { if (removalSubmitting) event.preventDefault(); }} onInteractOutside={(event) => { if (removalSubmitting) event.preventDefault(); }} onOpenAutoFocus={(event) => { event.preventDefault(); document.getElementById('cancel-removal')?.focus(); }} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(removalTrigger, 'trash-heading'); removalTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Remove {removalPreview?.count} {removalPreview?.count === 1 ? 'item' : 'items'} permanently?</Dialog.Title><Dialog.Description>The count is for Trash items. Folders include their contents, which are not counted separately. Removal cannot be undone. {#if !removalPreview?.all}Items already in Trash inside a folder stay there unless selected separately.{/if}</Dialog.Description></Dialog.Header>
        <Dialog.Footer><Button id="cancel-removal" variant="outline" disabled={removalSubmitting} onclick={() => removalPreview = null}>Cancel</Button><Button variant="destructive" disabled={removalSubmitting} aria-busy={removalSubmitting} onclick={confirmRemoval}>{removalSubmitting ? 'Removing…' : 'Remove permanently'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!trashTarget} onOpenChange={(open) => { if (!open && !trashBusy) trashTarget = null; }}>
      <Dialog.Content showCloseButton={!trashBusy} aria-busy={trashBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(trashTrigger); trashTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Delete {trashTarget?.name}?</Dialog.Title><Dialog.Description>{trashTarget?.kind === 'folder' ? 'Move this folder and its contents to trash. You can restore the folder later. Public links and direct shares rooted here or in its contents are permanently revoked.' : 'Move this file to trash. You can restore it later. Public links to this file are permanently revoked.'}</Dialog.Description></Dialog.Header>
        {#if trashError}<p role="alert">{trashError}</p>{/if}
        <Dialog.Footer><Button variant="outline" disabled={trashBusy} onclick={() => trashTarget = null}>Cancel</Button><Button disabled={trashBusy} aria-busy={trashBusy} onclick={deleteEntry}>{trashBusy ? 'Moving to trash…' : 'Move to trash'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root bind:open={shortcutsOpen}>
      <Dialog.Content onCloseAutoFocus={restoreShortcutFocus}>
        <Dialog.Header><Dialog.Title>Keyboard shortcuts</Dialog.Title><Dialog.Description>To use item shortcuts, press Down Arrow from the folder heading or Tab to an item. Shortcuts pause while you type or use a dialog.</Dialog.Description></Dialog.Header>
        <dl class="grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2">
          {#each shortcutHelp as shortcut (shortcut.keys)}
            <dt class="flex flex-wrap gap-1">{#each shortcut.keys.split(', ') as key (key)}<kbd class="rounded border bg-muted px-1.5 py-0.5 font-mono text-xs">{key}</kbd>{/each}</dt>
            <dd>{shortcut.action}</dd>
          {/each}
        </dl>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!renameId} onOpenChange={(open) => { if (!open && !renameBusy) closeRename(); }}>
      <Dialog.Content onOpenAutoFocus={(event) => { event.preventDefault(); renameInput?.focus(); }} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(renameTrigger); renameTrigger = null; }}>
        <Dialog.Header><Dialog.Title>Rename item</Dialog.Title><Dialog.Description>Choose a name that is not already used in this folder.</Dialog.Description></Dialog.Header>
        <form class="grid gap-6" onsubmit={renameEntry}><div class="grid gap-2"><Label for="rename-name">New name</Label><Input id="rename-name" bind:ref={renameInput} bind:value={renameName} disabled={renameBusy} maxlength={255} aria-invalid={!!renameError} aria-describedby={renameError ? 'rename-error' : undefined} required /></div>{#if renameError}<p id="rename-error" role="alert">{renameError}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={renameBusy} onclick={closeRename}>Cancel</Button><Button type="submit" disabled={renameBusy}>{renameBusy ? 'Saving…' : 'Save name'}</Button></Dialog.Footer></form>
      </Dialog.Content>
    </Dialog.Root>

    <Dialog.Root open={conversionOpen} onOpenChange={(open) => { if (!open) closeConversion(); }}>
      <Dialog.Content onEscapeKeydown={(event) => { if (oidcOpening) event.preventDefault(); }} onInteractOutside={(event) => { if (oidcOpening) event.preventDefault(); }} showCloseButton={!oidcOpening} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(conversionTrigger, 'account-title'); }}>
        <Dialog.Header><Dialog.Title>Switch to Keycloak</Dialog.Title><Dialog.Description>After you sign in with Keycloak, Sylux removes this account’s local password, signs out other sessions and revokes its WebDAV app passwords. Your files and sharing stay in place.</Dialog.Description></Dialog.Header>
        <form class="grid gap-4" onsubmit={startKeycloak}>
          <div class="grid gap-2"><Label for="conversion-password">Current Sylux password</Label><Input id="conversion-password" type="password" autocomplete="current-password" bind:value={conversionPassword} disabled={oidcOpening} required /></div>
          {#if conversionError}<p role="alert" class="form-notice">{conversionError}</p>{/if}
          <Dialog.Footer><Button type="button" variant="outline" disabled={oidcOpening} onclick={closeConversion}>Cancel</Button><Button type="submit" disabled={oidcOpening} aria-busy={oidcOpening}>{oidcOpening ? 'Opening Keycloak…' : 'Continue to Keycloak'}</Button></Dialog.Footer>
        </form>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={changePasswordOpen} onOpenChange={(open) => { if (!accountBusy) { changePasswordOpen = open; if (!open) { currentPassword = ''; newPassword = ''; accountNotice = ''; } } }}><Dialog.Content onEscapeKeydown={(event) => { if (accountBusy) event.preventDefault(); }} onInteractOutside={(event) => { if (accountBusy) event.preventDefault(); }} showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(passwordDialogTrigger, 'account-title'); }}><Dialog.Header><Dialog.Title>Change password</Dialog.Title><Dialog.Description>This signs out all your sessions. Use 15 to 128 characters.</Dialog.Description></Dialog.Header><form class="grid gap-4" onsubmit={changePassword}><div class="grid gap-2"><Label for="current-password">Current password</Label><Input id="current-password" type="password" autocomplete="current-password" bind:value={currentPassword} disabled={accountBusy} required /></div><div class="grid gap-2"><Label for="new-password">New password</Label><Input id="new-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={newPassword} disabled={accountBusy} required /></div>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={accountBusy} onclick={() => { changePasswordOpen = false; currentPassword = ''; newPassword = ''; accountNotice = ''; }}>Cancel</Button><Button type="submit" disabled={accountBusy}>{accountBusy ? 'Changing…' : 'Change password'}</Button></Dialog.Footer></form></Dialog.Content></Dialog.Root>
    <Dialog.Root open={createAccountOpen} onOpenChange={(open) => { if (!accountBusy) { createAccountOpen = open; if (!open) clearCreateAccountSecrets(); } }}>
      <Dialog.Content onEscapeKeydown={(event) => { if (accountBusy) event.preventDefault(); }} onInteractOutside={(event) => { if (accountBusy) event.preventDefault(); }} showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}><Dialog.Header><Dialog.Title>Create account</Dialog.Title><Dialog.Description>Use 15 to 128 characters for the password. Share it through a private channel.</Dialog.Description></Dialog.Header><form class="grid gap-4" aria-busy={accountBusy} onsubmit={createAccount}><div class="grid gap-2"><Label for="account-handle">Account name</Label><Input id="account-handle" bind:value={accountHandle} pattern="[a-z0-9][a-z0-9._-]*" maxlength={64} disabled={accountBusy} required /><p class="text-muted-foreground text-xs">Lowercase letters, numbers, dots, underscores or hyphens.</p></div><div class="grid gap-2"><Label for="account-password">Password</Label><div class="flex items-center gap-2"><Input id="account-password" class="min-w-0" type={accountPasswordVisible ? 'text' : 'password'} autocomplete="new-password" minlength={15} maxlength={128} bind:value={accountPassword} oninput={() => accountPasswordError = ''} disabled={accountBusy} required /><Button type="button" variant="outline" class="shrink-0" aria-controls="account-password" aria-pressed={accountPasswordVisible} disabled={accountBusy} onclick={() => accountPasswordVisible = !accountPasswordVisible}>{accountPasswordVisible ? 'Hide password' : 'Show password'}</Button></div></div><div class="grid gap-2"><Label for="account-password-confirmation">Confirm password</Label><div class="flex items-center gap-2"><Input id="account-password-confirmation" class="min-w-0" type={accountPasswordConfirmationVisible ? 'text' : 'password'} autocomplete="new-password" minlength={15} maxlength={128} bind:value={accountPasswordConfirmation} oninput={() => accountPasswordError = ''} aria-invalid={!!accountPasswordError} aria-describedby={accountPasswordError ? 'account-password-error' : undefined} disabled={accountBusy} required /><Button type="button" variant="outline" class="shrink-0" aria-controls="account-password-confirmation" aria-pressed={accountPasswordConfirmationVisible} disabled={accountBusy} onclick={() => accountPasswordConfirmationVisible = !accountPasswordConfirmationVisible}>{accountPasswordConfirmationVisible ? 'Hide confirmation' : 'Show confirmation'}</Button></div></div>{#if accountPasswordError}<p id="account-password-error" role="alert">{accountPasswordError}</p>{/if}{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={accountBusy} onclick={() => { createAccountOpen = false; clearCreateAccountSecrets(); }}>Cancel</Button><Button type="submit" disabled={accountBusy} aria-busy={accountBusy}>{accountBusy ? 'Creating…' : 'Create account'}</Button></Dialog.Footer></form></Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!resetId} onOpenChange={(open) => { if (!open && !accountBusy) { resetId = ''; resetPassword = ''; } }}><Dialog.Content onEscapeKeydown={(event) => { if (accountBusy) event.preventDefault(); }} onInteractOutside={(event) => { if (accountBusy) event.preventDefault(); }} showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}><Dialog.Header><Dialog.Title>Reset password for {accounts.find((account) => account.account_id === resetId)?.handle}</Dialog.Title><Dialog.Description>This signs out all their sessions. Share the new password through a private channel.</Dialog.Description></Dialog.Header><form class="grid gap-4" onsubmit={resetAccount}><div class="grid gap-2"><Label for="reset-password">New password</Label><Input id="reset-password" type="password" autocomplete="new-password" minlength={15} maxlength={128} bind:value={resetPassword} disabled={accountBusy} required /><p class="text-muted-foreground text-xs">Use 15 to 128 characters.</p></div>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button type="button" variant="outline" disabled={accountBusy} onclick={() => { resetId = ''; resetPassword = ''; }}>Cancel</Button><Button type="submit" disabled={accountBusy} aria-busy={accountBusy}>{accountBusy ? 'Resetting…' : 'Reset password'}</Button></Dialog.Footer></form></Dialog.Content></Dialog.Root>
    <Dialog.Root open={!!authorityTarget} onOpenChange={(open) => { if (!open && !accountBusy) authorityTarget = null; }}>
      <Dialog.Content onEscapeKeydown={(event) => { if (accountBusy) event.preventDefault(); }} onInteractOutside={(event) => { if (accountBusy) event.preventDefault(); }} showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}>
        <Dialog.Header>
          <Dialog.Title class="pr-8 break-words">{authorityTarget?.action === 'enable' ? `Enable ${authorityTarget.account.handle}?` : `Make ${authorityTarget?.account.handle} ${authorityTarget?.account.role === 'user' ? 'an administrator' : 'a user'}?`}</Dialog.Title>
          <Dialog.Description>
            {#if authorityTarget?.action === 'enable'}They can sign in {authorityTarget.account.credential_mode === 'external' ? 'with Keycloak' : 'with their current password'}. Old sessions stay signed out.
            {:else if authorityTarget?.account.role === 'user'}They can manage accounts and instance settings. Other accounts’ files stay private. This signs out all their sessions.
            {:else}They will lose access to account and instance settings. Their private files stay available. This signs out all their sessions.{/if}
          </Dialog.Description>
        </Dialog.Header>
        {#if accountNotice}<p role="alert">{accountNotice}</p>{/if}
        <Dialog.Footer><Button variant="outline" disabled={accountBusy} onclick={() => authorityTarget = null}>Cancel</Button><Button disabled={accountBusy} aria-busy={accountBusy} onclick={changeAccountAuthority}>{accountBusy ? 'Updating…' : authorityTarget?.action === 'enable' ? 'Enable account' : authorityTarget?.account.role === 'user' ? 'Make administrator' : 'Make user'}</Button></Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
    <Dialog.Root open={!!disableTarget} onOpenChange={(open) => { if (!open && !accountBusy) disableTarget = null; }}><Dialog.Content onEscapeKeydown={(event) => { if (accountBusy) event.preventDefault(); }} onInteractOutside={(event) => { if (accountBusy) event.preventDefault(); }} showCloseButton={!accountBusy} onCloseAutoFocus={(event) => { event.preventDefault(); restoreFocus(accountDialogTrigger, 'accounts-heading'); }}><Dialog.Header><Dialog.Title class="pr-8 break-words">Disable {disableTarget?.handle}?</Dialog.Title><Dialog.Description>This prevents sign in and signs out all their sessions. Their files stay stored. Their public links end. Direct shares to and from this account end, and their published libraries are withdrawn. Enabling the account again does not restore sharing.</Dialog.Description></Dialog.Header>{#if accountNotice}<p role="alert">{accountNotice}</p>{/if}<Dialog.Footer><Button variant="outline" disabled={accountBusy} onclick={() => disableTarget = null}>Cancel</Button><Button variant="destructive" disabled={accountBusy} aria-busy={accountBusy} onclick={() => disableTarget && disableAccount(disableTarget)}>{accountBusy ? 'Disabling…' : 'Disable account'}</Button></Dialog.Footer></Dialog.Content></Dialog.Root>
  {/if}
  {#if !user || loggingOut}{@render betaNotice(false)}{/if}
</div>

<style>
  #route-status { position: sticky; top: 8px; z-index: 1; width: fit-content; padding: 4px 8px; background: var(--background); }
  .route-placeholder { display: grid; gap: 14px; padding: 16px 0; }
  :is(#folder-title, #trash-heading, #account-title, #route-status, :global(#libraries-title)):focus-visible { outline: 2px solid var(--ring); outline-offset: 3px; }
  .route-placeholder span { height: var(--control-height); background: var(--muted); border-radius: var(--radius); }
  .upload-batch-report { display: grid; gap: 12px; padding-block: 12px; }
  .upload-pending { display: flex; justify-content: space-between; align-items: center; gap: 8px; color: var(--muted-foreground); }
  .upload-pending [role="status"] { display: grid; min-width: 0; gap: 4px; }
  .upload-pending .upload-current-path, .upload-pending .upload-batch-destination { white-space: normal; overflow-wrap: anywhere; }
  .upload-pending :global(svg) { width: var(--icon-file); height: var(--icon-file); }
  .upload-batch-progress { display: grid; gap: 6px; width: min(100%, 640px); min-width: 0; font-size: var(--type-ui); font-variant-numeric: tabular-nums; }
  .upload-batch-heading, .upload-batch-bytes { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .upload-batch-heading > span { font-weight: var(--weight-heading); }
  .upload-batch-current { display: flex; gap: 5px; min-width: 0; }
  .upload-batch-current > span:first-child { flex-shrink: 0; }
  .upload-current-path, .upload-batch-destination { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .upload-batch-destination, .upload-batch-bytes, .upload-waiting { color: var(--muted-foreground); font-size: var(--type-meta); }
  .upload-location-details { font-size: var(--type-meta); }
  .upload-location-details summary { min-height: var(--control-height); align-content: center; }
  @media (pointer: coarse) { .upload-location-details summary { min-height: var(--control-height-touch); } }
  .upload-location-details dl { display: grid; gap: 4px; margin-top: 8px; }
  .upload-location-details dt { color: var(--muted-foreground); }
  .upload-location-details dd { overflow-wrap: anywhere; margin: 0 0 4px; }
  .upload-byte-progress { height: 6px; overflow: hidden; border-radius: var(--radius); background: var(--muted); }
  .upload-byte-progress > span { display: block; height: 100%; background: var(--primary); transition: width 150ms linear; }
  @media (prefers-reduced-motion: reduce) { .upload-byte-progress > span { transition: none; } }
  .upload-batch-report summary { cursor: pointer; font-size: var(--type-ui); }
  .upload-batch-report li { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 16px; border-bottom: 1px solid var(--border); padding-block: 8px; font-size: var(--type-ui); }
  .upload-batch-report li > span:last-child { flex-shrink: 0; }
  .upload-feedback, .upload-batch-status, .upload-result-path { overflow-wrap: anywhere; min-width: 0; }
  .action-feedback { display: flex; align-items: center; justify-content: space-between; gap: 12px; min-width: 0; font-size: var(--type-ui); }
  .action-feedback p { display: flex; align-items: center; gap: 8px; min-width: 0; overflow-wrap: anywhere; }
  .action-feedback p :global(svg) { width: var(--icon-small); height: var(--icon-small); flex-shrink: 0; color: var(--primary); }
  .action-feedback-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .action-feedback-heading h2 { margin: 0; }
  .workspace-feedback { margin-block: 12px; }
  .file-drop-region { border-radius: var(--radius-lg); }
  .file-drop-region[data-drop-active="true"] { outline: 2px solid var(--primary); outline-offset: -2px; }
  :global(.drop-active) { border-radius: var(--radius-lg); background: var(--accent); outline: 2px solid var(--primary); outline-offset: -2px; }
  :global(.refreshing) { animation: refresh-turn 1s linear infinite; }
  @keyframes refresh-turn { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { :global(.refreshing) { animation: none; } }
</style>

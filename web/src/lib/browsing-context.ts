import type { FolderSort, ResultSort } from './listing';

export type Destination = 'files' | 'trash' | 'account' | 'settings';
export type AccountSort = 'name_asc' | 'name_desc' | 'role_asc' | 'role_desc' | 'status_asc' | 'status_desc';
export type Location = { destination: Destination; folder: string; query: string; sort: FolderSort; searchSort?: ResultSort; trashSort?: ResultSort; accountSort?: AccountSort };
export type BrowsingContext = Location & { version: 1; owner: string; cursors: (string | null)[]; page: number; searchCursors: (string | null)[]; searchPage: number; scroll: number };
const sorts: FolderSort[] = ['name_asc', 'name_desc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc'];
const resultSorts = [...sorts, 'location_asc', 'location_desc'];
const accountSorts = ['name_asc', 'name_desc', 'role_asc', 'role_desc', 'status_asc', 'status_desc'];
const destinations = ['files', 'trash', 'account', 'settings'];
const validQuery = (query: string) => new TextEncoder().encode(query).length <= 255 && !/\p{Cc}/u.test(query);

export function parseLocation(hash: string): Location {
  const fallback: Location = { destination: 'files', folder: '', query: '', sort: 'name_asc' };
  if (hash.length > 4096) return fallback;
  try {
    const [path, query = ''] = hash.replace(/^#/, '').split('?');
    const [destination, folder = '', ...rest] = path.split('/');
    if (!destinations.includes(destination) || rest.length) return fallback;
    const params = new URLSearchParams(query);
    const search = params.get('q') ?? '';
    const sort = params.get('sort') as FolderSort;
    const id = decodeURIComponent(folder);
    if (id.length > 256 || /[\p{Cc}/]/u.test(id)) return fallback;
    return { destination: destination as Destination, folder: destination === 'files' ? id : '', query: destination === 'files' && validQuery(search) ? search : '', sort: sorts.includes(sort) ? sort : 'name_asc',
      ...(resultSorts.includes(params.get('searchSort') ?? '') ? { searchSort: params.get('searchSort') as ResultSort } : {}),
      ...(resultSorts.includes(params.get('trashSort') ?? '') ? { trashSort: params.get('trashSort') as ResultSort } : {}),
      ...(accountSorts.includes(params.get('accountSort') ?? '') ? { accountSort: params.get('accountSort') as AccountSort } : {}),
    };
  } catch { return fallback; }
}

export function locationHash(location: Location): string {
  const params = new URLSearchParams();
  if (location.query && location.destination === 'files') params.set('q', location.query);
  if (location.sort !== 'name_asc') params.set('sort', location.sort);
  for (const key of ['searchSort', 'trashSort', 'accountSort'] as const) {
    const value = location[key];
    if (value && value !== 'name_asc') params.set(key, value);
  }
  return `#${location.destination}${location.destination === 'files' && location.folder ? '/' + encodeURIComponent(location.folder) : ''}${params.size ? '?' + params : ''}`;
}

function validStack(value: unknown, page: unknown): value is (string | null)[] {
  return Array.isArray(value) && value.length > 0 && value.length <= 2048 && value[0] === null && Number.isInteger(page) && (page as number) >= 0 && (page as number) < value.length && value.every((cursor, index) => index === 0 ? cursor === null : typeof cursor === 'string' && cursor.length > 0 && cursor.length <= 8192);
}

export function savedContext(value: unknown, owner: string, location: Location): BrowsingContext | null {
  if (!value || typeof value !== 'object') return null;
  const state = value as BrowsingContext;
  if (!destinations.includes(state.destination) || typeof state.folder !== 'string' || state.folder.length > 256 || /[\p{Cc}/]/u.test(state.folder) || typeof state.query !== 'string' || !validQuery(state.query) || !sorts.includes(state.sort)) return null;
  if ((state.searchSort !== undefined && !resultSorts.includes(state.searchSort)) ||
    (state.trashSort !== undefined && !resultSorts.includes(state.trashSort)) ||
    (state.accountSort !== undefined && !accountSorts.includes(state.accountSort))) return null;
  if (state.version !== 1 || state.owner !== owner || locationHash(state) !== locationHash(location)) return null;
  if (!validStack(state.cursors, state.page) || !validStack(state.searchCursors, state.searchPage) || !Number.isFinite(state.scroll) || state.scroll < 0 || state.scroll > 100_000_000) return null;
  return state;
}

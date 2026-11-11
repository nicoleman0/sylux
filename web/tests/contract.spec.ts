import { expect, test } from '@playwright/test';
import { ApiClient, HttpTransport, MutationApi, type ApiTransport, type RequestOptions } from '../src/lib/api';
import { ListingApi } from '../src/lib/listing';
import { TransferApi } from '../src/lib/transfers';

type RecordedRequest = { path: string; options: RequestOptions };

class RecordingTransport implements ApiTransport {
  readonly requests: RecordedRequest[] = [];

  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    this.requests.push({ path, options });
    if (path === '/mutation-tickets') {
      return { mutation_id: 'mutation-1', ticket: 'mock-mutation-ticket', expires_at: '2030-01-01T00:00:00Z' } as T;
    }
    if (path.startsWith('/folders/children')) {
      if (options.method === 'POST') return { id: 'folder-created', parent_id: 'root', name: 'Research', kind: 'folder', generation: 1, etag: '"g1"' } as T;
      return { items: [], root: { id: 'root', parent_id: null, name: 'My files', kind: 'folder', generation: 1, etag: '"root:1"' }, next_cursor: null, has_more: false } as T;
    }
    if (path === '/folders/root/children') {
      return { items: [], next_cursor: null, has_more: false } as T;
    }
    if (path === '/uploads') return { upload_id: 'mutation-1' } as T;
    throw new Error(`Unexpected request: ${options.method ?? 'GET'} ${path}`);
  }
}

function createHarness() {
  const transport = new RecordingTransport();
  const client = new ApiClient(transport);
  client.setCsrfToken('mock-csrf-token');
  const mutations = new MutationApi(client);
  return {
    transport,
    listing: new ListingApi(client),
    transfers: new TransferApi(client, mutations),
  };
}

test('folder creation uses the authenticated folder collection contract', async () => {
  const { transport, listing } = createHarness();
  await listing.createFolder('root', 'Research', '"g1"');

  const request = transport.requests.find(({ path }) => path === '/folders/children');
  expect(request).toBeDefined();
  expect(request?.options.method).toBe('POST');
  expect(request?.options.headers).toMatchObject({ 'X-CSRF-Token': 'mock-csrf-token' });
  expect(request?.options.headers).not.toHaveProperty('X-Mutation-ID');
  expect(request?.options.body).toEqual({ parent_id: 'root', name: 'Research' });
});

test('upload admission sends the parent ETag in JSON without If-Match', async () => {
  const { transport, transfers } = createHarness();
  await transfers.admit('root', 'fixture.txt', 12, '"root-1"');

  const ticketRequest = transport.requests.find(({ path }) => path === '/mutation-tickets');
  expect(ticketRequest?.options.method).toBe('POST');
  expect(ticketRequest?.options.body).toEqual({
    scope: 'upload_create', parent_id: 'root', parent_etag: '"root-1"', name: 'fixture.txt', length: 12,
  });
  const request = transport.requests.find(({ path }) => path === '/uploads');
  expect(request).toBeDefined();
  expect(request?.options.method).toBe('POST');
  expect(request?.options.headers).toMatchObject({
    'If-None-Match': '*',
    'X-Mutation-ID': 'mock-mutation-ticket',
    'X-CSRF-Token': 'mock-csrf-token',
  });
  expect(request?.options.body).toEqual({
    parent_id: 'root', name: 'fixture.txt', length: 12, parent_etag: '"root-1"',
  });
});

test('roots lists the private root through the backend collection route', async () => {
  const transport = new RecordingTransport();
  const client = new ApiClient(transport);
  const listing = new ListingApi(client);

  await expect(listing.roots()).resolves.toMatchObject({
    items: [],
    root: { id: 'root', parent_id: null, name: 'My files', kind: 'folder', generation: 1, etag: '"root:1"' },
    next_cursor: null,
    has_more: false,
  });
  expect(transport.requests[0]?.path).toBe('/folders/children?limit=100');
});

test('malformed error JSON preserves unauthorized status and expires the session', async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response('{', { status: 401, headers: { 'content-type': 'application/json' } });
  let expired = false;
  const client = new ApiClient(new HttpTransport());
  client.setSessionExpiredHandler(() => { expired = true; });
  try {
    await expect(client.request('/folders/root/children')).rejects.toMatchObject({ name: 'HttpError', status: 401 });
    expect(expired).toBe(true);
  } finally { globalThis.fetch = originalFetch; }
});

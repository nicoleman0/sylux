import { expect, test } from '@playwright/test';
import { ApiClient, type ApiTransport } from '../src/lib/api';
import { ListingApi, type DeletedEntry, type ResultSort } from '../src/lib/listing';

class ResponseTransport implements ApiTransport {
  constructor(private body: unknown) {}
  async request<T>(): Promise<T> { return this.body as T; }
  set(body: unknown) { this.body = body; }
}

const entry = (id: string, name: string, fields: Partial<DeletedEntry> = {}): DeletedEntry => ({
  id, name, kind: 'file', parent_id: 'parent', size: 1, original_path: [], etag: `${id}:1`, ...fields,
});
const response = (items: DeletedEntry[]) => ({ items, next_cursor: null, has_more: false });
const sorts: ResultSort[] = ['name_asc', 'name_desc', 'size_asc', 'size_desc', 'modified_asc', 'modified_desc', 'location_asc', 'location_desc'];

test('Trash validates numeric metadata and accepts unknown and epoch values', async () => {
  const transport = new ResponseTransport(response([entry('ok', 'ok')]));
  const listing = new ListingApi(new ApiClient(transport));
  for (const field of ['size', 'modified_at'] as const) {
    const invalid = field === 'size' ? [-1, NaN, Infinity, 1.5, '1'] : [NaN, Infinity, 1.5, '1', 8_640_000_000_001];
    for (const value of invalid) {
      transport.set(response([entry('bad', 'bad', { [field]: value } as Partial<DeletedEntry>)]));
      await expect(listing.deleted()).rejects.toThrow('Invalid Trash entry response');
    }
  }
  transport.set(response([entry('epoch', 'epoch', { size: 0, modified_at: 0 })]));
  await expect(listing.deleted()).resolves.toMatchObject({ items: [{ id: 'epoch' }] });
  for (const metadata of [{ size: null, modified_at: null }, { size: undefined, modified_at: undefined }, { modified_at: -1 }]) {
    transport.set(response([entry('valid', 'valid', metadata)]));
    await expect(listing.deleted()).resolves.toBeTruthy();
  }
});

test('Trash page ordering follows every sort and breaks ties by stable ID', async () => {
  const samples = [
    entry('b', 'same', { size: 2, modified_at: 20, original_path: ['Folder'] }),
    entry('a', 'same', { size: 2, modified_at: 20, original_path: ['Folder'] }),
    entry('c', 'Other', { size: null, modified_at: null, original_path: ['folder'] }),
    entry('d', 'Early', { size: 1, modified_at: 10, original_path: ['A'] }),
    entry('folder', 'Same folder', { kind: 'folder', parent_id: 'parent', size: null, original_path: [] }),
  ];
  const orderedIds: Record<ResultSort, string[]> = {
    name_asc: ['d', 'c', 'folder', 'a', 'b'], name_desc: ['a', 'b', 'folder', 'c', 'd'],
    size_asc: ['folder', 'd', 'a', 'b', 'c'], size_desc: ['folder', 'a', 'b', 'd', 'c'],
    modified_asc: ['folder', 'd', 'a', 'b', 'c'], modified_desc: ['folder', 'a', 'b', 'd', 'c'],
    location_asc: ['folder', 'd', 'a', 'b', 'c'], location_desc: ['c', 'a', 'b', 'd', 'folder'],
  };
  for (const sort of sorts) {
    const expected = orderedIds[sort].map(id => samples.find(item => item.id === id)!);
    const transport = new ResponseTransport(response(expected));
    const listing = new ListingApi(new ApiClient(transport));
    await expect(listing.deleted(undefined, sort)).resolves.toMatchObject({ items: expected });
    transport.set(response([...expected].reverse()));
    await expect(listing.deleted(undefined, sort)).rejects.toThrow('Trash pagination did not advance');
    transport.set(response([expected[0], expected[0]]));
    await expect(listing.deleted(undefined, sort)).rejects.toThrow('Trash pagination repeated an item');
  }
});

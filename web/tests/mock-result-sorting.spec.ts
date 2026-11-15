import { expect, test } from '@playwright/test';
import type { Entry, ResultSort, SearchListing, TrashPage } from '../src/lib/listing';
import { MockTransport } from '../src/lib/mock-transport';

const entries: Entry[] = [
  { id: 'alpha', parent_id: 'root', name: 'Sort alpha', kind: 'folder', etag: 'a' },
  { id: 'zeta', parent_id: 'root', name: 'Sort zeta', kind: 'folder', etag: 'z' },
  { id: 'one', parent_id: 'zeta', name: 'Sort 01', kind: 'file', size: 10, modified_at: 0, etag: '1' },
  { id: 'two', parent_id: 'root', name: 'Sort 02', kind: 'file', size: 0, modified_at: 50, etag: '2' },
  { id: 'three', parent_id: 'alpha', name: 'Sort 03', kind: 'file', size: 10, modified_at: 50, etag: '3' },
  { id: 'unknown', parent_id: 'alpha', name: 'Sort 04', kind: 'file', size: Number.NaN, modified_at: null, etag: '4' },
];
const orders: Record<ResultSort, string[]> = {
  name_asc: ['one', 'two', 'three', 'unknown', 'alpha', 'zeta'],
  name_desc: ['zeta', 'alpha', 'unknown', 'three', 'two', 'one'],
  size_asc: ['alpha', 'zeta', 'two', 'one', 'three', 'unknown'],
  size_desc: ['alpha', 'zeta', 'one', 'three', 'two', 'unknown'],
  modified_asc: ['alpha', 'zeta', 'one', 'two', 'three', 'unknown'],
  modified_desc: ['alpha', 'zeta', 'two', 'three', 'one', 'unknown'],
  location_asc: ['two', 'alpha', 'zeta', 'three', 'unknown', 'one'],
  location_desc: ['one', 'three', 'unknown', 'two', 'alpha', 'zeta'],
};

for (const search of [true, false]) {
  test(`mock ${search ? 'search' : 'Trash'} orders all results and binds cursor sort and metadata`, async () => {
    const mock = new MockTransport();
    await mock.request('/auth/local', { method: 'POST', body: { handle: 'owner' } });
    const fixture = entries.map(entry => ({ ...entry }));
    Object.assign(mock, { entries: search ? fixture : [], deletedEntries: search ? [] : fixture });
    const base = search ? '/entries/search?q=sort&' : '/trash?';
    for (const [sort, expected] of Object.entries(orders)) {
      const ids: string[] = [];
      let cursor: string | null = null;
      do {
        const page: SearchListing | TrashPage = await mock.request(`${base}sort=${sort}&limit=2${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`);
        expect(page.items.length).toBeLessThanOrEqual(2);
        for (const item of page.items) {
          const entry = 'entry' in item ? item.entry : item;
          ids.push(entry.id);
          if (entry.kind === 'folder' || entry.id === 'unknown') expect(entry).toMatchObject({ size: null, modified_at: null });
          if (entry.id === 'one') expect(entry).toMatchObject({ size: 10, modified_at: 0 });
        }
        cursor = page.next_cursor;
        expect(page.has_more).toBe(cursor !== null);
        if (cursor) expect(ids.length).toBeLessThan(entries.length);
      } while (cursor);
      expect(ids).toEqual(expected);
    }
    for (const invalid of ['', 'NAME_ASC', 'unsupported']) {
      await expect(mock.request(`${base}sort=${invalid}`)).rejects.toMatchObject({ status: 400, code: 'invalid_sort' });
    }
    const first = await mock.request<TrashPage>(`${base}sort=size_asc&limit=1`);
    const cursor = encodeURIComponent(first.next_cursor!);
    for (const sort of Object.keys(orders).filter(sort => sort !== 'size_asc')) {
      await expect(mock.request(`${base}sort=${sort}&cursor=${cursor}`)).rejects.toMatchObject({ status: 400, code: 'invalid_cursor' });
    }
    await expect(mock.request(`${base}sort=size_asc&cursor=${'A'.repeat(4097)}`)).rejects.toMatchObject({ status: 400, code: 'invalid_cursor' });
    const file = fixture.find(entry => entry.id === 'one')!;
    if (file.kind !== 'file') throw new Error('Missing fixture file');
    file.modified_at = 10;
    await expect(mock.request(`${base}sort=size_asc&cursor=${cursor}`)).rejects.toMatchObject({ status: 409, code: 'listing_changed' });
  });
}

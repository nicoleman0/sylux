import type { Entry } from './listing';

const compact = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
const full = new Intl.DateTimeFormat(undefined, { dateStyle: 'full', timeStyle: 'long' });

export function modifiedTime(entry: Pick<Entry, 'kind' | 'modified_at'>): { datetime: string; compact: string; full: string } | null {
  if (entry.kind !== 'file' || typeof entry.modified_at !== 'number') return null;
  const date = new Date(entry.modified_at * 1000);
  if (!Number.isFinite(date.getTime())) return null;
  return { datetime: date.toISOString(), compact: compact.format(date), full: full.format(date) };
}

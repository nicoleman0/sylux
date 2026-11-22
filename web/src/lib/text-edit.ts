import { HttpError, type ApiClient, type MutationTicket, type RequestOptions } from './api';
import type { FileEntry } from './listing';
import type { UploadContentResult, UploadStatus } from './transfers';

export const TEXT_EDIT_LIMIT = 256 * 1024;
export type TextSource = { revision: string; bom: boolean };
export type TextEditState = {
  draft: { entry: FileEntry; source: TextSource; original: string; text: string; wire: string } | null;
  operation: TextSave | null;
  stale: boolean;
};

export function editableTextName(name: string): boolean {
  return /\.(md|txt)$/i.test(name);
}

export function editorText(text: string): string {
  return text.replace(/\r\n?/g, '\n');
}

export function preserveLineEndings(wire: string, previous: string, next: string): string {
  // The textarea exposes LF. Keep separators in the unchanged prefix and suffix.
  let start = 0;
  while (start < previous.length && previous[start] === next[start]) start++;
  let end = 0;
  while (end < previous.length - start && end < next.length - start &&
    previous[previous.length - end - 1] === next[next.length - end - 1]) end++;
  const endings = wire.match(/\r\n|\r|\n/g) ?? [];
  let line = 0;
  const prefix = previous.slice(0, start).split('\n').length - 1;
  const middleEnd = previous.length - end;
  const removed = previous.slice(start, middleEnd).split('\n').length - 1;
  const reusable = endings.slice(prefix, prefix + removed);
  const tail = endings.slice(prefix + removed);
  const inserted = next.slice(start, next.length - end).split('\n').length - 1;
  const nearby = reusable[0] ?? tail[0] ?? endings[prefix - 1] ?? '\n';
  const result = [
    ...endings.slice(0, prefix),
    ...Array.from({ length: inserted }, (_, index) => reusable[index] ?? nearby),
    ...tail,
  ];
  return next.replace(/\n/g, (separator, offset: number) => {
    const ending = result[line++];
    // Adjacent bare CR and LF would otherwise collapse into a single break.
    return ending === '\n' && result[line - 2] === '\r' && next[offset - 1] === separator
      ? '\r\n' : ending;
  });
}

export function draftBytes(wire: string, bom: boolean): Blob {
  return new Blob([bom ? '\uFEFF' : '', wire], { type: 'application/octet-stream' });
}

type SaveOutcome = 'saved' | 'stale' | 'retry' | 'unknown';
type EditStatus = UploadStatus & { revision_id?: string };
type EditCommit = { status: string; mutation_id: string; replaced_entry_id: string; size: number; sha256: string; revision_id: string };

export class TextSave {
  private issued: MutationTicket | undefined;
  private admitted = false;
  private inspect = false;

  constructor(
    private readonly entry: FileEntry,
    private readonly revision: string,
    private readonly parentEtag: string,
    private readonly bytes: Blob,
  ) {}

  get inProgress(): boolean { return this.issued !== undefined; }

  async run(client: ApiClient, signal: AbortSignal): Promise<SaveOutcome> {
    const { entry, parentEtag, bytes } = this;
    async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
      signal.throwIfAborted();
      const result = await client.request<T>(path, { ...options, signal });
      signal.throwIfAborted();
      return result;
    }
    signal.throwIfAborted();
    const content = await bytes.arrayBuffer();
    signal.throwIfAborted();
    const sha256 = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', content)),
      (byte) => byte.toString(16).padStart(2, '0')).join('');
    const admission = {
      parent_id: entry.parent_id, parent_etag: parentEtag, name: entry.name,
      length: bytes.size, sha256, replacement: { entry_id: entry.id, etag: entry.etag, text_revision: this.revision },
    };
    const finished = (state: EditStatus) => state.status === 'committed' && state.state === 'committed' &&
      !!state.revision_id && state.length === bytes.size && state.sha256 === sha256;
    try {
      this.issued ??= await request<MutationTicket>('/mutation-tickets', {
        method: 'POST', body: { scope: 'upload_create', ...admission },
      });
    } catch (error) {
      return error instanceof HttpError && [404, 412].includes(error.status) ? 'stale' : 'retry';
    }
    const id = this.issued.mutation_id;
    const path = `/uploads/${encodeURIComponent(id)}`;
    const headers = { 'X-Mutation-ID': this.issued.ticket, 'If-Match': entry.etag };
    const status = async () => {
      const state = await request<EditStatus>(path);
      if (state.upload_id !== id || state.mutation_id !== id) throw new Error('Unexpected edit save status');
      return state;
    };
    try {
      signal.throwIfAborted();
      if (!this.admitted) {
        try {
          await request('/uploads', { method: 'POST', body: admission, headers });
          signal.throwIfAborted();
          this.admitted = true;
        } catch (error) {
          if (!signal.aborted && error instanceof HttpError && [404, 412].includes(error.status)) {
            this.issued = undefined;
            return 'stale';
          }
          throw error;
        }
      }
      let staged = false;
      if (this.inspect) {
        const state = await status();
        signal.throwIfAborted();
        if (finished(state)) return 'saved';
        if (state.state === 'rejected') {
          this.issued = undefined;
          return 'retry';
        }
        staged = state.status === 'staged' && state.state === 'publishing' && !state.writer_active &&
          state.length === bytes.size && state.sha256 === sha256;
        if (!staged && (state.status !== 'admitted' || state.writer_active)) return 'unknown';
      }
      this.inspect = true;
      if (!staged) {
        const result = await request<UploadContentResult>(`${path}/content`, {
          method: 'PUT', body: bytes, headers,
        });
        if (result.upload_id !== id || result.status !== 'staged' || result.length !== bytes.size ||
          result.sha256 !== sha256) return 'unknown';
      }
      const result = await request<EditCommit>(`${path}/commit`, {
        method: 'POST', body: { parent_etag: parentEtag, sha256 }, headers,
      });
      if (result?.status !== 'committed' || result.mutation_id !== id || result.replaced_entry_id !== entry.id ||
        result.size !== bytes.size || result.sha256 !== sha256 || !result.revision_id) throw new Error('Unconfirmed edit save');
      return 'saved';
    } catch (error) {
      try {
        if (finished(await status())) return 'saved';
        if (error instanceof HttpError && [404, 412].includes(error.status)) {
          await request(path, { method: 'DELETE' });
          signal.throwIfAborted();
          this.issued = undefined;
          return 'stale';
        }
      } catch {
        // Keep the same mutation until its outcome can be checked.
      }
      return 'unknown';
    }
  }

  async cancel(client: ApiClient, signal: AbortSignal): Promise<'saved' | 'canceled' | 'unknown'> {
    if (!this.issued) return 'canceled';
    const path = `/uploads/${encodeURIComponent(this.issued.mutation_id)}`;
    try {
      signal.throwIfAborted();
      await client.request(path, { method: 'DELETE', signal });
      return 'canceled';
    } catch (error) {
      if (error instanceof HttpError && error.code === 'upload_already_committed') return 'saved';
      if (error instanceof HttpError && error.code === 'upload_not_found' && !this.admitted) return 'canceled';
      return 'unknown';
    }
  }
}

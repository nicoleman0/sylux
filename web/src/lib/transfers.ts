import { HttpError, type ApiClient, type MutationApi } from './api';

export type UploadStatus = {
  upload_id: string;
  mutation_id: string;
  state: 'admitted' | 'publishing' | 'committed' | 'rejected';
  status: 'admitted' | 'uploading' | 'staged' | 'committed' | 'canceled' | 'rejected' | 'recovery_required';
  bytes_received: number;
  length: number;
  writer_active: boolean;
  sha256?: string | null;
  expires_at?: number;
};
export type UploadContentResult = { upload_id: string; mutation_id: string; status: 'staged'; length: number; sha256: string; published: false };
export type Replacement = { entry_id: string; etag: string };
export type UploadAdmission = { id: string; status_url: string };

const IDENTITY_CHUNK = 8 * 1024 * 1024;
const digestHex = (bytes: ArrayBuffer) => Array.from(new Uint8Array(bytes), byte => byte.toString(16).padStart(2, '0')).join('');
export type FileIdentity = { name: string; size: number; fingerprint: string };
type AdmissionBody = { parent_id: string; parent_etag: string; name: string; length: number; replacement?: Replacement };
export type UploadJournal = { version: 1; owner: string; id: string; ticket: string; body: AdmissionBody; identity: FileIdentity; expires: number; chunkSize: number; commitStarted?: boolean; stagedSha256?: string };

// Every byte participates, using at most one file slice and a bounded list of digests.
export async function identifyFile(file: File, signal?: AbortSignal, progress?: (read: number, total: number) => void): Promise<FileIdentity> {
  const hashes = new Uint8Array(Math.ceil(file.size / IDENTITY_CHUNK) * 32);
  for (let offset = 0; offset < file.size; offset += IDENTITY_CHUNK) {
    signal?.throwIfAborted();
    const hash = await crypto.subtle.digest('SHA-256', await file.slice(offset, offset + IDENTITY_CHUNK).arrayBuffer());
    new Uint8Array(hashes.buffer, offset / IDENTITY_CHUNK * 32, 32).set(new Uint8Array(hash));
    progress?.(Math.min(file.size, offset + IDENTITY_CHUNK), file.size);
  }
  signal?.throwIfAborted();
  return { name: file.name, size: file.size, fingerprint: digestHex(await crypto.subtle.digest('SHA-256', hashes)) };
}

// The commit handler returns these only after rolling back, so no save happened.
export function saveRefused(error: unknown): error is HttpError {
  return error instanceof HttpError && ((error.status === 409 && error.code === 'name_collision') ||
    (error.status === 412 && (error.code === 'replacement_changed' || error.code === 'stale_parent_etag')));
}

export class TransferApi {
  private active = true;
  private owner = '';
  private journal: UploadJournal | null = null;
  private key(): string { return `sylux-upload-v1:${this.owner}`; }
  restore(owner: string): UploadJournal | null {
    this.owner = owner;
    const raw = sessionStorage.getItem(this.key());
    if (!raw) return null;
    let value: UploadJournal;
    try { value = JSON.parse(raw) as UploadJournal; } catch { throw new Error('The retained upload record is unreadable.'); }
    if (value.version !== 1 || value.owner !== owner || !/^[a-f0-9]{32}$/.test(value.id) || typeof value.ticket !== 'string' ||
      !value.body || !value.identity || value.identity.name !== value.body.name || value.identity.size !== value.body.length ||
      !Number.isSafeInteger(value.body.length) || value.body.length < 0 || !/^[a-f0-9]{64}$/.test(value.identity.fingerprint) ||
      (value.commitStarted !== undefined && typeof value.commitStarted !== 'boolean') ||
      (value.stagedSha256 !== undefined && !/^[a-f0-9]{64}$/.test(value.stagedSha256)) || !Number.isSafeInteger(value.expires) || !Number.isSafeInteger(value.chunkSize) || value.chunkSize <= 0 || value.chunkSize > IDENTITY_CHUNK) {
      throw new Error('The retained upload record is invalid.');
    }
    this.journal = value;
    this.tickets.set(value.id, value.ticket); this.parentEtags.set(value.id, value.body.parent_etag);
    this.admissions.set(value.id, { body: value.body, ticket: value.ticket });
    return value;
  }
  pending(): UploadJournal | null { return this.journal; }
  canCommit(uploadId: string): boolean { return this.journal?.id !== uploadId || this.journal.commitStarted !== true; }
  expired(uploadId: string): boolean { return this.journal?.id === uploadId && this.journal.expires <= Date.now() / 1000; }
  assertUnexpired(uploadId: string): void {
    if (this.expired(uploadId)) throw new Error('The 24-hour upload window has expired. Check its status, then start a new upload after this attempt is resolved.');
  }
  private stage(uploadId: string, sha256: string): void {
    this.stagedDigests.set(uploadId, sha256);
    if (this.journal?.id === uploadId) this.retain({ ...this.journal, stagedSha256: sha256 });
  }
  private retain(value: UploadJournal): void {
    if (!this.owner) throw new Error('Upload recovery needs a signed-in account.');
    try { sessionStorage.setItem(this.key(), JSON.stringify(value)); }
    catch { throw new Error('Upload recovery could not be saved in this tab. Free browser storage and try again.'); }
    this.journal = value;
  }
  clearRecovery(): void {
    if (this.owner) sessionStorage.removeItem(this.key());
    this.journal = null;
  }
  private ensureActive(): void {
    if (!this.active) throw new DOMException('Session ended', 'AbortError');
  }
  invalidate(): void {
    this.active = false;
    this.tickets.clear(); this.parentEtags.clear(); this.stagedDigests.clear(); this.admissions.clear();
  }
  private readonly tickets = new Map<string, string>();
  private readonly stagedDigests = new Map<string, string>();
  private readonly admissions = new Map<string, { body: { parent_id: string; parent_etag: string; name: string; length: number; replacement?: Replacement }; ticket: string }>();
  constructor(private readonly client: ApiClient, private readonly mutations: MutationApi) {}

  async admit(parentId: string, name: string, size: number, etag: string, onIssued?: (uploadId: string) => void, replacement?: Replacement, identity?: FileIdentity): Promise<UploadAdmission> {
    this.ensureActive();
    const body = { parent_id: parentId, parent_etag: etag, name, length: size, ...(replacement ? { replacement } : {}) };
    const ticket = await this.mutations.issue(body);
    this.ensureActive();
    this.tickets.set(ticket.mutation_id, ticket.ticket);
    this.parentEtags.set(ticket.mutation_id, etag);
    this.admissions.set(ticket.mutation_id, { body, ticket: ticket.ticket });
    if (identity) this.retain({ version: 1, owner: this.owner, id: ticket.mutation_id, ticket: ticket.ticket, body, identity, expires: ticket.expires_at, chunkSize: ticket.max_chunk_length });
    onIssued?.(ticket.mutation_id);
    await this.client.request('/uploads', {
      method: 'POST', headers: { ...this.precondition(body.replacement), 'X-Mutation-ID': ticket.ticket }, body,
    });
    this.ensureActive();
    return { id: ticket.mutation_id, status_url: `/uploads/${encodeURIComponent(ticket.mutation_id)}` };
  }
  async send(uploadId: string, bytes: Blob, signal?: AbortSignal, onProgress?: (sent: number, total: number) => void, onPending?: (message: string) => void): Promise<UploadContentResult> {
    this.ensureActive();
    const ticket = this.tickets.get(uploadId);
    if (!ticket) throw new Error('No active mutation ticket for this upload');
    this.assertUnexpired(uploadId);
    let state = await this.waitForWriter(uploadId, signal, onPending);
    if (state.state === 'committed' || state.state === 'rejected') throw new Error('This upload is already finished. Check its status.');
    let offset = state.bytes_received;
    if (!Number.isSafeInteger(offset) || offset < 0 || offset > bytes.size) throw new Error('Invalid upload offset');
    onProgress?.(offset, bytes.size);
    if (state.status === 'staged' && state.sha256) {
      this.stage(uploadId, state.sha256);
      return { upload_id: uploadId, mutation_id: uploadId, status: 'staged', length: bytes.size, sha256: state.sha256, published: false };
    }
    const chunkSize = this.journal?.chunkSize;
    if (!chunkSize || chunkSize <= 0 || chunkSize > IDENTITY_CHUNK) throw new Error('Missing upload chunk limit');
    let result: UploadContentResult | undefined;
    do {
      signal?.throwIfAborted();
      const chunk = bytes.slice(offset, offset + chunkSize);
      const checksum = digestHex(await crypto.subtle.digest('SHA-256', await chunk.arrayBuffer()));
      this.ensureActive(); signal?.throwIfAborted();
      let retries = 0;
      while (true) {
        try {
          result = await this.client.request<UploadContentResult & { bytes_received: number }>(`/uploads/${encodeURIComponent(uploadId)}/content`, {
            method: 'PATCH', headers: { 'Content-Type': 'application/octet-stream', 'X-Mutation-ID': ticket, 'Upload-Offset': String(offset), 'Upload-Checksum': checksum }, body: chunk, signal,
            onUploadProgress: (sent) => onProgress?.(offset + Math.min(sent, chunk.size), bytes.size),
          });
          break;
        } catch (error) {
          signal?.throwIfAborted(); this.ensureActive();
          // Timeouts, shutdowns and conflicts leave the acknowledged offset in place; status decides where to continue.
          if (error instanceof HttpError && (error.code === 'upload_canceled' || (error.status !== 408 && error.status !== 409 && error.status < 500))) throw error;
          if (++retries > 3) throw error;
          await this.pause(500 * retries, signal);
          state = await this.waitForWriter(uploadId, signal, onPending);
          this.assertUnexpired(uploadId);
          if (state.bytes_received === offset + chunk.size) {
            if (state.status === 'staged' && state.sha256) result = { upload_id: uploadId, mutation_id: uploadId, status: 'staged', length: bytes.size, sha256: state.sha256, published: false };
            else if (state.state === 'admitted') result = undefined;
            else throw new Error('Upload state changed. Check its status before continuing.');
            break;
          }
          if (state.state !== 'admitted' || state.bytes_received !== offset) throw new Error('Upload state changed. Check its status before continuing.');
        }
      }
      this.ensureActive();
      if (result && (result.upload_id !== uploadId || result.mutation_id !== uploadId || result.published !== false)) throw new Error('Invalid upload checkpoint');
      offset += chunk.size;
      onProgress?.(offset, bytes.size);
    } while (offset < bytes.size || !result);
    if (!result || result.upload_id !== uploadId || result.mutation_id !== uploadId || result.length !== bytes.size || result.published !== false || !/^[a-f0-9]{64}$/.test(result.sha256) || result.status !== 'staged') throw new Error('Invalid staged upload response');
    this.stage(uploadId, result.sha256);
    return result;
  }
  private async waitForWriter(uploadId: string, signal?: AbortSignal, onPending?: (message: string) => void): Promise<UploadStatus> {
    let failures = 0;
    // Covers a proxy timeout during final verification without resending private bytes.
    for (let polls = 0; polls < 120; polls++) {
      this.ensureActive(); signal?.throwIfAborted();
      let state: UploadStatus;
      try { state = await this.status(uploadId, signal); }
      catch (error) {
        this.ensureActive(); signal?.throwIfAborted();
        if (error instanceof HttpError && error.status < 500 || ++failures > 3) throw error;
        onPending?.('Connection interrupted. Checking the saved upload offset…');
        await this.pause(500 * failures, signal);
        continue;
      }
      if (!state.writer_active) return state;
      onPending?.('The server is receiving or verifying this upload. Waiting for its saved status…');
      await this.pause(1000, signal);
    }
    throw new Error('The server is still receiving or verifying this upload. Check its status shortly; its bytes remain private.');
  }
  private async pause(milliseconds: number, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      const abort = () => { clearTimeout(timer); reject(signal?.reason ?? new DOMException('Upload aborted', 'AbortError')); };
      const timer = setTimeout(() => { signal?.removeEventListener('abort', abort); resolve(); }, milliseconds);
      signal?.addEventListener('abort', abort, { once: true });
    });
    this.ensureActive(); signal?.throwIfAborted();
  }
  async commit(uploadId: string, sha256 = this.stagedDigests.get(uploadId) ?? ''): Promise<unknown> {
    if (!this.canCommit(uploadId)) throw new Error('Saving is unconfirmed. Check the original upload status.');
    return this.save(uploadId, sha256);
  }
  // The same staged upload may be saved after status shows no writer; it never re-sends bytes or admits again.
  finishable(uploadId: string, state: UploadStatus): boolean {
    const length = this.admissions.get(uploadId)?.body.length;
    const known = this.journal?.id === uploadId ? this.journal.stagedSha256 : this.stagedDigests.get(uploadId);
    return length !== undefined && state.upload_id === uploadId && state.mutation_id === uploadId &&
      state.state === 'publishing' && state.status === 'staged' && !state.writer_active &&
      state.length === length && state.bytes_received === length && !!state.sha256 && /^[a-f0-9]{64}$/.test(state.sha256) &&
      (!known || known === state.sha256);
  }
  async finish(uploadId: string): Promise<unknown> {
    this.ensureActive(); this.assertUnexpired(uploadId);
    const state = await this.status(uploadId);
    this.ensureActive();
    if (!this.finishable(uploadId, state)) throw new Error('The original upload is no longer ready to save. Check its status before continuing.');
    this.stage(uploadId, state.sha256!);
    return this.save(uploadId, state.sha256!);
  }
  private async save(uploadId: string, sha256: string): Promise<unknown> {
    this.ensureActive(); this.assertUnexpired(uploadId);
    const ticket = this.tickets.get(uploadId);
    const parent_etag = this.parentEtags.get(uploadId);
    if (!ticket || !parent_etag) throw new Error('No active mutation ticket for this upload');
    if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error('Invalid staged content digest');
    if (this.journal?.id === uploadId) this.retain({ ...this.journal, commitStarted: true });
    let result: unknown;
    try {
      result = await this.client.request(`/uploads/${encodeURIComponent(uploadId)}/commit`, {
        method: 'POST', headers: { ...this.precondition(this.admissions.get(uploadId)?.body.replacement), 'X-Mutation-ID': ticket }, body: { sha256, parent_etag },
      });
    } catch (error) {
      if (saveRefused(error) && this.journal?.id === uploadId) {
        this.retain({ ...this.journal, commitStarted: false });
      }
      throw error;
    }
    this.ensureActive();
    if (this.journal?.id === uploadId) this.clearRecovery();
    this.tickets.delete(uploadId);
    this.parentEtags.delete(uploadId);
    this.stagedDigests.delete(uploadId);
    this.admissions.delete(uploadId);
    return result;
  }
  status(uploadId: string, signal?: AbortSignal): Promise<UploadStatus> {
    this.ensureActive();
    return this.client.request(`/uploads/${encodeURIComponent(uploadId)}`, { signal });
  }
  async retryAdmission(uploadId: string): Promise<void> {
    this.ensureActive();
    const admission = this.admissions.get(uploadId);
    if (!admission) throw new Error('No retained admission for this upload');
    await this.client.request('/uploads', {
      method: 'POST', headers: { ...this.precondition(admission.body.replacement), 'X-Mutation-ID': admission.ticket }, body: admission.body,
    });
  }
  forget(uploadId: string): void {
    if (this.journal?.id === uploadId) this.clearRecovery();
    this.tickets.delete(uploadId);
    this.parentEtags.delete(uploadId);
    this.stagedDigests.delete(uploadId);
    this.admissions.delete(uploadId);
  }
  cancel(uploadId: string): Promise<void> {
    this.ensureActive();
    const ticket = this.tickets.get(uploadId);
    if (!ticket) throw new Error('No active mutation ticket for this upload');
    return this.client.request<void>(`/uploads/${encodeURIComponent(uploadId)}`, {
      method: 'DELETE', headers: { 'X-Mutation-ID': ticket },
    }).then(() => {
      this.ensureActive();
      if (this.journal?.id === uploadId) this.clearRecovery();
      this.tickets.delete(uploadId);
      this.parentEtags.delete(uploadId);
      this.stagedDigests.delete(uploadId);
      this.admissions.delete(uploadId);
    });
  }
  private precondition(replacement?: Replacement): Record<string, string> {
    return replacement ? { 'If-Match': replacement.etag } : { 'If-None-Match': '*' };
  }
  private readonly parentEtags = new Map<string, string>();
}

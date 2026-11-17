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
export type UploadJournal = { version: 1; owner: string; id: string; ticket: string; body: AdmissionBody; identity: FileIdentity; expires: number; chunkSize: number; commitStarted?: boolean };

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
      (value.commitStarted !== undefined && typeof value.commitStarted !== 'boolean') || !Number.isSafeInteger(value.expires) || !Number.isSafeInteger(value.chunkSize) || value.chunkSize <= 0 || value.chunkSize > IDENTITY_CHUNK) {
      throw new Error('The retained upload record is invalid.');
    }
    this.journal = value;
    this.tickets.set(value.id, value.ticket); this.parentEtags.set(value.id, value.body.parent_etag);
    this.admissions.set(value.id, { body: value.body, ticket: value.ticket });
    return value;
  }
  pending(): UploadJournal | null { return this.journal; }
  canCommit(uploadId: string): boolean { return this.journal?.id !== uploadId || this.journal.commitStarted !== true; }
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
  async send(uploadId: string, bytes: Blob, signal?: AbortSignal, onProgress?: (sent: number, total: number) => void): Promise<UploadContentResult> {
    this.ensureActive();
    const ticket = this.tickets.get(uploadId);
    if (!ticket) throw new Error('No active mutation ticket for this upload');
    let state = await this.status(uploadId);
    if (state.writer_active) throw new Error('The server is still receiving this upload. Check again shortly.');
    if (state.state === 'committed' || state.state === 'rejected') throw new Error('This upload is already finished. Check its status.');
    let offset = state.bytes_received;
    if (!Number.isSafeInteger(offset) || offset < 0 || offset > bytes.size) throw new Error('Invalid upload offset');
    onProgress?.(offset, bytes.size);
    if (state.status === 'staged' && state.sha256) {
      this.stagedDigests.set(uploadId, state.sha256);
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
          if (error instanceof HttpError && error.status !== 409 && error.status < 500) throw error;
          if (++retries > 3) throw error;
          await new Promise(resolve => setTimeout(resolve, 500 * retries));
          state = await this.status(uploadId);
          if (state.writer_active) continue;
          if (state.bytes_received === offset + chunk.size) {
            if (state.status === 'staged' && state.sha256) result = { upload_id: uploadId, mutation_id: uploadId, status: 'staged', length: bytes.size, sha256: state.sha256, published: false };
            else result = undefined;
            break;
          }
          if (state.state !== 'admitted' || state.bytes_received !== offset) throw new Error('Upload state changed. Check its status before continuing.');
        }
      }
      this.ensureActive();
      if (result && (result.upload_id !== uploadId || result.mutation_id !== uploadId || result.published !== false)) throw new Error('Invalid upload checkpoint');
      offset += chunk.size;
      onProgress?.(offset, bytes.size);
    } while (offset < bytes.size);
    if (!result || result.upload_id !== uploadId || result.mutation_id !== uploadId || result.length !== bytes.size || result.published !== false || !/^[a-f0-9]{64}$/.test(result.sha256) || result.status !== 'staged') throw new Error('Invalid staged upload response');
    this.stagedDigests.set(uploadId, result.sha256);
    return result;
  }
  async commit(uploadId: string, sha256 = this.stagedDigests.get(uploadId) ?? ''): Promise<unknown> {
    this.ensureActive();
    const ticket = this.tickets.get(uploadId);
    const parent_etag = this.parentEtags.get(uploadId);
    if (!ticket || !parent_etag) throw new Error('No active mutation ticket for this upload');
    if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error('Invalid staged content digest');
    if (!this.canCommit(uploadId)) throw new Error('Saving is unconfirmed. Check the original upload status.');
    if (this.journal?.id === uploadId) this.retain({ ...this.journal, commitStarted: true });
    const result = await this.client.request(`/uploads/${encodeURIComponent(uploadId)}/commit`, {
      method: 'POST', headers: { ...this.precondition(this.admissions.get(uploadId)?.body.replacement), 'X-Mutation-ID': ticket }, body: { sha256, parent_etag },
    });
    this.ensureActive();
    if (this.journal?.id === uploadId) this.clearRecovery();
    this.tickets.delete(uploadId);
    this.parentEtags.delete(uploadId);
    this.stagedDigests.delete(uploadId);
    this.admissions.delete(uploadId);
    return result;
  }
  status(uploadId: string): Promise<UploadStatus> {
    this.ensureActive();
    return this.client.request(`/uploads/${encodeURIComponent(uploadId)}`);
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

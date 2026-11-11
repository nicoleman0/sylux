import type { ApiClient, MutationApi } from './api';

export type UploadStatus = {
  upload_id: string;
  mutation_id: string;
  state: 'admitted' | 'publishing' | 'committed' | 'rejected';
  status: 'admitted' | 'uploading' | 'staged' | 'committed' | 'canceled' | 'rejected';
  bytes_received: number;
  length: number;
  writer_active: boolean;
};
export type UploadContentResult = { upload_id: string; mutation_id: string; status: 'staged'; length: number; sha256: string; published: false };
export type Replacement = { entry_id: string; etag: string };
export type UploadAdmission = { id: string; status_url: string };

export class TransferApi {
  private active = true;
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

  async admit(parentId: string, name: string, size: number, etag: string, onIssued?: (uploadId: string) => void, replacement?: Replacement): Promise<UploadAdmission> {
    this.ensureActive();
    const body = { parent_id: parentId, parent_etag: etag, name, length: size, ...(replacement ? { replacement } : {}) };
    const ticket = await this.mutations.issue(body);
    this.ensureActive();
    this.tickets.set(ticket.mutation_id, ticket.ticket);
    this.parentEtags.set(ticket.mutation_id, etag);
    this.admissions.set(ticket.mutation_id, { body, ticket: ticket.ticket });
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
    const result = await this.client.request<UploadContentResult>(`/uploads/${encodeURIComponent(uploadId)}/content`, {
      method: 'PUT', headers: { 'Content-Type': 'application/octet-stream', 'X-Mutation-ID': ticket }, body: bytes, signal, onUploadProgress: onProgress,
    });
    this.ensureActive();
    if (result.upload_id !== uploadId || result.mutation_id !== uploadId || result.length !== bytes.size || result.published !== false || !/^[a-f0-9]{64}$/.test(result.sha256) || result.status !== 'staged') throw new Error('Invalid staged upload response');
    this.stagedDigests.set(uploadId, result.sha256);
    return result;
  }
  async commit(uploadId: string, sha256 = this.stagedDigests.get(uploadId) ?? ''): Promise<unknown> {
    this.ensureActive();
    const ticket = this.tickets.get(uploadId);
    const parent_etag = this.parentEtags.get(uploadId);
    if (!ticket || !parent_etag) throw new Error('No active mutation ticket for this upload');
    if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error('Invalid staged content digest');
    const result = await this.client.request(`/uploads/${encodeURIComponent(uploadId)}/commit`, {
      method: 'POST', headers: { ...this.precondition(this.admissions.get(uploadId)?.body.replacement), 'X-Mutation-ID': ticket }, body: { sha256, parent_etag },
    });
    this.ensureActive();
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

export type ApiError = { code: string; message: string; request_id: string; existing?: { id: string; name: string; kind: string; etag: string } };
export class HttpError extends Error {
  constructor(readonly status: number, readonly details?: ApiError) { super(`Request failed (${status})`); this.name = 'HttpError'; }
  get code(): string | undefined { return this.details?.code; }
}

export type RequestOptions = {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';
  body?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
  onUploadProgress?: (loaded: number, total: number) => void;
};

/** Transport seam used by the same-origin browser client and focused tests. */
export interface ApiTransport {
  request<T>(path: string, options?: RequestOptions): Promise<T>;
}

/** Same-origin transport: cookies remain browser-managed and are never persisted by JS. */
export class HttpTransport implements ApiTransport {
  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const method = options.method ?? 'GET';
    const headers = new Headers(options.headers);
    let body: BodyInit | undefined;
    if (options.body instanceof Blob) {
      body = options.body;
    } else if (options.body !== undefined) {
      headers.set('Content-Type', 'application/json');
      body = JSON.stringify(options.body);
    }
    if (method === 'PUT' && body instanceof Blob && options.onUploadProgress) {
      return new Promise<T>((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        const signal = options.signal;
        const abort = () => xhr.abort();
        let settled = false;
        const cleanup = () => {
          if (settled) return;
          settled = true;
          signal?.removeEventListener('abort', abort);
        };
        if (signal?.aborted) {
          reject(new DOMException('Upload aborted', 'AbortError'));
          return;
        }
        xhr.open(method, path);
        xhr.withCredentials = true;
        xhr.responseType = 'text';
        headers.forEach((value, key) => xhr.setRequestHeader(key, value));
        xhr.upload.onprogress = (event) => options.onUploadProgress?.(event.loaded, event.lengthComputable ? event.total : body!.size);
        xhr.onload = () => {
          cleanup();
          const expected = new URL(path, window.location.href);
          const finalUrl = new URL(xhr.responseURL || expected.href, window.location.href);
          if (finalUrl.origin !== expected.origin || finalUrl.pathname !== expected.pathname || finalUrl.search !== expected.search || finalUrl.hash) return reject(new Error('Unexpected upload response URL'));
          if (xhr.status < 200 || xhr.status >= 300) {
            let details: ApiError | undefined;
            if (xhr.getResponseHeader('content-type')?.includes('application/json')) {
              try { details = JSON.parse(xhr.responseText) as ApiError; }
              catch { /* Preserve the HTTP status when a proxy returns malformed JSON. */ }
            }
            return reject(new HttpError(xhr.status, details));
          }
          try { resolve((xhr.getResponseHeader('content-type')?.includes('application/json') ? JSON.parse(xhr.responseText) : xhr.responseText) as T); }
          catch (error) { reject(error); }
        };
        xhr.onerror = () => { cleanup(); reject(new Error('Network request failed')); };
        xhr.onabort = () => { cleanup(); reject(new DOMException('Upload aborted', 'AbortError')); };
        signal?.addEventListener('abort', abort, { once: true });
        xhr.send(body);
      });
    }
    const response = await fetch(path, {
      method, headers, body, signal: options.signal, credentials: 'same-origin',
      cache: 'no-store', redirect: 'error',
    });
    if (!response.ok) {
      let details: ApiError | undefined;
      if (response.headers.get('content-type')?.includes('application/json')) {
        try { details = await response.json() as ApiError; }
        catch { /* Authentication handling still needs the HTTP status if JSON is malformed. */ }
      }
      throw new HttpError(response.status, details);
    }
    if (response.status === 204) return undefined as T;
    const contentType = response.headers.get('content-type') ?? '';
    return (contentType.includes('application/json') ? await response.json() : await response.blob()) as T;
  }
}

export class ApiClient {
  private csrfToken: string | null = null;
  private onSessionExpired: (() => void) | null = null;

  constructor(private readonly transport: ApiTransport) {}

  setCsrfToken(token: string | null): void {
    this.csrfToken = token;
  }

  setSessionExpiredHandler(handler: (() => void) | null): void {
    this.onSessionExpired = handler;
  }

  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const method = options.method ?? 'GET';
    const headers = { ...options.headers };
    if (method !== 'GET') {
      if (!this.csrfToken) throw new Error('CSRF token is required for mutations');
      headers['X-CSRF-Token'] = this.csrfToken;
    }
    const onExpired = this.onSessionExpired;
    try { return await this.transport.request<T>(path, { ...options, headers }); }
    catch (error) {
      // Password verification and mutation tickets can fail without ending the session.
      if (error instanceof HttpError && error.status === 401 && path !== '/me/password' &&
        (!error.code || ['unauthorized', 'authentication_required'].includes(error.code))) onExpired?.();
      throw error;
    }
  }
}

export type MutationTicket = { mutation_id: string; ticket: string; expires_at: string };

export class MutationApi {
  constructor(private readonly client: ApiClient) {}
  issue(admission: { parent_id: string; parent_etag: string; name: string; length: number; replacement?: { entry_id: string; etag: string } }): Promise<MutationTicket> {
    return this.client.request('/mutation-tickets', {
      method: 'POST', body: { scope: 'upload_create', ...admission },
    });
  }
}


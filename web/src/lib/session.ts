import type { ApiClient } from './api';

export type User = { id: string; handle: string; role: 'administrator' | 'user'; external?: boolean };
export type Account = { account_id: string; handle: string; role: 'administrator' | 'user'; enabled: boolean; credential_mode?: 'local' | 'external' };
export type OidcLink = { id: string; handle: string; pending: boolean; expires_at: number | null; generation: number };
/** Logical file bytes; null means the server could not determine a category. */
export type Usage = { files_bytes: number | null; trash_bytes: number | null };
export type AllUsage = { volume: { available_bytes: number; total_bytes: number } | null; accounts: (Usage & { account_id: string })[] };
export type SessionState = { user: User | null; setupRequired: boolean; csrfToken: string };

type BackendUser = { account_id: string; handle: string; role: 'administrator' | 'user'; csrf_token?: string };

function isBytes(value: unknown): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0; }
function validUsage(value: Usage): Usage {
  const category = (bytes: unknown) => { if (bytes !== null && !isBytes(bytes)) throw new Error('Invalid usage response'); return bytes; };
  return { files_bytes: category(value?.files_bytes), trash_bytes: category(value?.trash_bytes) };
}

export class SessionApi {
  constructor(private readonly client: ApiClient) {}

  async state(setupAuthorization: string): Promise<SessionState> {
    const result = await this.client.request<{ initialized: boolean }>('/setup', {
      headers: { 'X-Setup-Authorization': setupAuthorization },
    });
    return { user: null, setupRequired: !result.initialized, csrfToken: '' };
  }
  async csrf(): Promise<{ csrfToken: string }> {
    const result = await this.client.request<{ csrf_token: string }>('/auth/csrf');
    return { csrfToken: result.csrf_token };
  }
  async setup(handle: string, password: string, setupAuthorization: string): Promise<User> {
    await this.client.request('/setup/admin', {
      method: 'POST', headers: { 'X-Setup-Authorization': setupAuthorization }, body: { handle, password },
    });
    return { id: '', handle, role: 'administrator' };
  }
  async login(handle: string, password: string): Promise<User> {
    const result = await this.client.request<BackendUser>('/auth/local', { method: 'POST', body: { handle, password } });
    if (result.csrf_token) this.client.setCsrfToken(result.csrf_token);
    return { id: result.account_id, handle: result.handle, role: result.role };
  }
  async oidcEnabled(): Promise<boolean> {
    return (await this.client.request<{ enabled: boolean }>('/auth/oidc/status')).enabled === true;
  }
  async externalSession(): Promise<{ user: User; csrfToken: string }> {
    const result = await this.client.request<BackendUser>('/auth/oidc/session');
    if (!result.csrf_token) throw new Error('Missing external session token');
    return { user: { id: result.account_id, handle: result.handle, role: result.role, external: true }, csrfToken: result.csrf_token };
  }
  async startOidc(handle: string, conversion_password?: string): Promise<string> {
    const result = await this.client.request<{ authorization_url: string }>('/auth/oidc/start', {
      method: 'POST', body: { handle, conversion_password },
    });
    const url = new URL(result.authorization_url);
    if (url.protocol !== 'https:') throw new Error('Invalid Keycloak sign-in URL');
    return url.href;
  }
  oidcLinks(): Promise<OidcLink[]> { return this.client.request('/auth/oidc/approvals'); }
  approveOidc(subject: string, handle: string, account_id?: string): Promise<void> {
    return this.client.request('/auth/oidc/approvals', { method: 'POST', body: { subject, handle, account_id } });
  }
  revokeOidc({ id, pending, generation }: OidcLink): Promise<void> {
    return this.client.request(`/auth/oidc/approvals/${id}/revoke`, { method: 'POST', body: { pending, generation } });
  }
  accounts(): Promise<Account[]> { return this.client.request('/accounts'); }
  createAccount(handle: string, password: string): Promise<void> { return this.client.request('/accounts', { method: 'POST', body: { handle, password } }); }
  disableAccount(id: string): Promise<void> { return this.client.request(`/accounts/${id}/disable`, { method: 'POST' }); }
  enableAccount(id: string): Promise<void> { return this.client.request(`/accounts/${id}/enable`, { method: 'POST' }); }
  changeRole(id: string, role: Account['role']): Promise<void> { return this.client.request(`/accounts/${id}/role`, { method: 'POST', body: { role } }); }
  resetAccount(id: string, password: string): Promise<void> { return this.client.request(`/accounts/${id}/reset`, { method: 'POST', body: { password } }); }
  async usage(signal?: AbortSignal): Promise<Usage> { return validUsage(await this.client.request('/me/usage', { signal })); }
  async accountsUsage(): Promise<AllUsage> {
    const result = await this.client.request<AllUsage>('/accounts/usage');
    const volume = result?.volume;
    if (!Array.isArray(result?.accounts) || (volume !== null && !(isBytes(volume?.available_bytes) && isBytes(volume?.total_bytes)))) throw new Error('Invalid usage response');
    return { volume, accounts: result.accounts.map((usage) => {
      if (typeof usage?.account_id !== 'string') throw new Error('Invalid usage response');
      return { account_id: usage.account_id, ...validUsage(usage) };
    }) };
  }
  changePassword(current_password: string, new_password: string): Promise<void> { return this.client.request('/me/password', { method: 'POST', body: { current_password, new_password } }); }
  logout(): Promise<void> { return this.client.request('/auth/logout', { method: 'POST' }); }
}

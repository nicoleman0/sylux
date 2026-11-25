// Invoked by qualify-keycloak.sh with a built production Sylux executable.
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { chmodSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { setTimeout as wait } from 'node:timers/promises';
import { DatabaseSync } from 'node:sqlite';
import { faultProvider } from './qualify-sylux-oidc-faults.mjs';

const deadline = 12_000;
const localPassword = randomBytes(24).toString('base64url');
const callbackPath = '/auth/oidc/callback';

export async function qualifySylux(fixture, executable, browser, provider) {
    const { check, issuer, clientId, clientSecret, createUser, admin, container, submit, submitFactor, step } = provider;
    const root = `${fixture}/sylux`;
    const config = `${fixture}/sylux-oidc.json`;
    const secretFile = `${fixture}/oidc-client-secret`;
    const env = { ...globalThis.process.env };
    delete env.KC_BOOTSTRAP_ADMIN_CLIENT_SECRET;
    mkdirSync(root, { mode: 0o700 });
    execFileSync(executable, ['init', '--root', root], { env, stdio: 'ignore' });
    const reservation = createServer().listen(0, '127.0.0.1');
    await once(reservation, 'listening');
    const port = reservation.address().port;
    await new Promise((resolve) => reservation.close(resolve));
    const origin = `https://localhost:${port}`;
    let activeIssuer = issuer;
    function writeConfig(overrides = {}) {
        writeFileSync(config, JSON.stringify({
            issuer: activeIssuer, client_id: clientId, callback: origin + callbackPath,
            client_secret_file: secretFile, ca_file: `${fixture}/ca.pem`, ...overrides,
        }), { mode: 0o600 });
    }
    writeFileSync(secretFile, clientSecret, { mode: 0o600 });
    writeConfig();
    let process;
    const contexts = [];
    async function start({ configured = true, reason } = {}) {
        const args = ['serve', '--root', root, '--listen', `127.0.0.1:${port}`,
            '--public-origin', origin, '--tls-cert', `${fixture}/tls/leaf.pem`,
            '--tls-key', `${fixture}/tls/leaf.key`, '--webdav'];
        if (configured) args.push('--oidc-config', config);
        process = spawn(executable, args, { env, stdio: ['ignore', 'ignore', 'pipe'] });
        let foundReason = false;
        process.stderr.on('data', (chunk) => {
            if (reason && chunk.toString().includes(`(${reason})`)) foundReason = true;
        });
        const limit = performance.now() + deadline;
        while (process.exitCode === null && performance.now() < limit) {
            const health = await fetch(origin + '/healthz', { signal: AbortSignal.timeout(500) }).catch(() => null);
            if (health?.ok) {
                check(!reason || foundReason, 'Expected redacted OIDC configuration diagnostic');
                return;
            }
            await wait(100);
        }
        check(false, 'Disposable production Sylux did not start');
    }
    function spawnRemoval() {
        return spawnSync(executable, ['admin', 'remove-oidc-provider', '--root', root],
            { env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    }
    function removeProvider() {
        const result = spawnRemoval();
        check(result.status === 0 && result.stdout.trim() === 'oidc_provider_removed', 'Offline provider removal was not acknowledged');
    }
    async function stop() {
        if (!process || process.exitCode !== null || process.signalCode !== null) return;
        const exit = once(process, 'exit', { signal: AbortSignal.timeout(deadline) });
        process.kill('SIGTERM');
        try {
            const [code, signal] = await exit;
            check(code === 0 && !signal, 'Disposable Sylux failed graceful shutdown');
        } catch (error) {
            process.kill('SIGKILL');
            throw error;
        }
    }
    async function context(options = {}) {
        const instance = await browser.newContext({ serviceWorkers: 'block', ...options });
        contexts.push(instance);
        instance.setDefaultTimeout(deadline);
        instance.setDefaultNavigationTimeout(deadline);
        await instance.route('**/*', (route) =>
            [origin, new URL(activeIssuer).origin].includes(new URL(route.request().url()).origin)
                ? route.continue() : route.abort());
        return instance;
    }
    async function api(browserContext, method, path, expected, { data, csrf, headers, redirect, ...options } = {}) {
        const response = await browserContext.request.fetch(origin + path, {
            method, data, maxRedirects: redirect ? undefined : 0, timeout: deadline,
            headers: { Origin: origin, ...(csrf ? { 'X-CSRF-Token': csrf } : {}), ...headers },
            ...options,
        });
        check(response.status() === expected, `${method} ${path.split('?')[0]} expected ${expected}, got ${response.status()}`);
        return response;
    }
    const json = async (...args) => (await api(...args)).json();
    async function local(context, handle, expected = 200, password = localPassword) {
        const { csrf_token: csrf } = await json(context, 'GET', '/auth/csrf', 200);
        const response = await api(context, 'POST', '/auth/local', expected, {
            csrf, data: { handle, password },
        });
        return expected === 200 ? response.json() : undefined;
    }
    async function begin(context, handle, credentials, expected = 200) {
        const csrf = credentials?.csrf_token ?? (await json(context, 'GET', '/auth/csrf', 200)).csrf_token;
        const response = await api(context, 'POST', '/auth/oidc/start', expected, {
            csrf, data: { handle, ...(credentials ? { conversion_password: localPassword } : {}) },
        });
        if (expected !== 200) return;
        const { authorization_url } = await response.json();
        const request = new URL(authorization_url);
        check(request.origin === new URL(activeIssuer).origin &&
            request.searchParams.get('redirect_uri') === origin + callbackPath &&
            request.searchParams.get('response_type') === 'code' &&
            request.searchParams.get('code_challenge_method') === 'S256' &&
            !!request.searchParams.get('nonce') && !!request.searchParams.get('state') &&
            request.searchParams.get('max_age') === '0' && request.searchParams.get('scope') === 'openid', 'Unexpected Sylux authorization request');
        return request.href;
    }
    async function proof(context, url, username) {
        const page = await context.newPage();
        let captured;
        const intercept = await context.newCDPSession(page);
        intercept.on('Fetch.requestPaused', ({ requestId, request }) => {
            captured = new URL(request.url);
            void intercept.send('Fetch.fulfillRequest', { requestId, responseCode: 200, body: '' });
        });
        await intercept.send('Fetch.enable', { patterns: [{ urlPattern: origin + callbackPath + '*', requestStage: 'Request' }] });
        try {
            await page.goto(url);
            await submit(page, username);
            await submitFactor(page, username);
            await page.waitForURL(`${origin}${callbackPath}?*`);
            const received = captured ?? new URL(page.url());
            const category = received.searchParams.get('error');
            check(received.searchParams.has('code'), 'Provider returned no authorization code' +
                (['access_denied', 'invalid_request', 'server_error', 'login_required', 'interaction_required'].includes(category)
                    ? ` (${category})` : ''));
            check(!!captured, 'Browser fault route did not intercept the callback');
            return captured;
        } finally {
            await page.close();
        }
    }
    async function browserSignIn(context, url, username) {
        const page = await context.newPage();
        try {
            await page.goto(url);
            const response = page.waitForResponse((item) => new URL(item.url()).pathname === callbackPath);
            await submit(page, username);
            await submitFactor(page, username);
            const completed = await response;
            check(completed.status() === 303 && completed.headers().location === '/', 'Browser callback failed');
            await page.waitForURL(origin + '/');
        } finally {
            await page.close();
        }
    }
    async function screenshot(page, name) {
        if (env.SYLUX_OIDC_EVIDENCE) await page.screenshot({ path: `${env.SYLUX_OIDC_EVIDENCE}/${name}.png`, animations: 'disabled' });
    }
    async function localUi(page, handle) {
        await page.goto(origin);
        await page.getByLabel('Account name').fill(handle);
        await page.getByLabel('Password', { exact: true }).fill(localPassword);
        await page.getByRole('button', { name: 'Sign in', exact: true }).click();
        await page.getByRole('button', { name: 'Toggle Sidebar' }).waitFor();
    }
    async function approveUi(page, subject, handle, existing = false) {
        await page.getByRole('region', { name: 'Keycloak sign-in' }).getByRole('button', { name: 'Approve sign-in', exact: true }).click();
        const dialog = page.getByRole('dialog', { name: 'Approve Keycloak sign-in' });
        await dialog.waitFor();
        if (existing) await screenshot(page, 'oidc-approval-desktop-light');
        check(typeof subject === 'string', 'Keycloak admin API did not return a stable subject');
        check(await dialog.getByLabel('Keycloak subject', { exact: true }).count() === 1, 'Keycloak subject label is missing or ambiguous');
        await dialog.getByLabel('Keycloak subject', { exact: true }).fill(subject);
        if (existing) await dialog.getByLabel('Sylux account').selectOption({ label: handle });
        else await dialog.getByLabel('New account name').fill(handle);
        await dialog.getByRole('checkbox', { name: 'I verified the person and their Keycloak subject.' }).check();
        await dialog.getByRole('button', { name: 'Approve sign-in' }).click();
        await page.getByText('Keycloak sign-in approved. The approval expires in 24 hours.').waitFor();
        await dialog.waitFor({ state: 'hidden' });
    }
    async function browserFinish(page, username) {
        await page.waitForURL((url) => url.origin === new URL(issuer).origin);
        const authorization = new URL(page.url());
        check(authorization.searchParams.get('max_age') === '0' &&
            authorization.searchParams.get('code_challenge_method') === 'S256', 'Browser sign-in used a different OIDC policy');
        const response = page.waitForResponse((item) => new URL(item.url()).pathname === callbackPath);
        await submit(page, username, true);
        await submitFactor(page, username);
        check((await response).headers().location === '/', 'Keycloak response did not establish the browser session');
        await page.getByRole('button', { name: 'Toggle Sidebar' }).waitFor();
    }
    async function keycloakUi(page, handle) {
        await page.goto(origin);
        await page.getByLabel('Account name').fill(handle);
        const keycloak = page.getByRole('button', { name: 'Sign in with Keycloak' });
        await keycloak.focus();
        await keycloak.press('Enter');
        await browserFinish(page, handle);
    }
    function withDb(operation) {
        const db = new DatabaseSync(root + '/metadata/sylux.sqlite3');
        try { return operation(db); }
        finally { db.close(); }
    }
    async function sessionCookie(context) {
        const cookies = await context.cookies(origin);
        const cookie = cookies.find(({ value }) => /^[a-f0-9]{32}\./.test(value));
        check(!!cookie, 'Sylux session cookie was unavailable');
        return cookie;
    }
    async function callback(context, address, status) {
        const response = await api(context, 'GET', callbackPath + address.search, status);
        check(response.headers()['cache-control'] === 'no-store' &&
            response.headers()['referrer-policy'] === 'no-referrer', 'Missing callback privacy headers');
        if (status === 303) {
            check(response.headers().location === '/' && !(await response.text()).length,
                'Callback must only redirect locally');
        }
        return response.status();
    }
    async function upload(context, csrf, name, library) {
        if (!library) {
            [library] = (await json(context, 'GET', '/libraries', 200)).items;
        }
        const bytes = Buffer.from(`disposable ${name} hello\n`);
        const sha256 = createHash('sha256').update(bytes).digest('hex');
        const data = { parent_id: library.id, parent_etag: library.etag, name, length: bytes.length, sha256 };
        const ticket = (await api(context, 'POST', '/mutation-tickets', 201, {
            csrf, data: { ...data, scope: 'upload_create' },
        })).headers()['x-mutation-id'];
        check(!!ticket, 'No upload mutation ticket');
        const headers = { 'X-Mutation-ID': ticket, 'If-None-Match': '*' };
        const attempt = await json(context, 'POST', '/uploads', 201, { csrf, data, headers });
        await api(context, 'PUT', `/uploads/${attempt.upload_id}/content`, 200, {
            csrf, data: bytes, headers: { 'X-Mutation-ID': ticket, 'Content-Type': 'application/octet-stream' },
        });
        const stored = await json(context, 'POST', `/uploads/${attempt.upload_id}/commit`, 201, {
            csrf, headers, data: { sha256, parent_etag: library.etag },
        });
        return { ...stored, bytes };
    }
    async function expectBytes(context, path, bytes, headers) {
        const response = await api(context, 'GET', path, 200, { headers });
        check((await response.body()).equals(bytes), 'Fixture download changed bytes');
    }
    async function shareForConversion(ownerContext, owner, recipient, handle) {
        const csrf = owner.csrf_token;
        const root = await json(ownerContext, 'POST', '/libraries', 201,
            { csrf, data: { name: `Direct grant fixture to ${handle}` } });
        const file = await upload(ownerContext, csrf, 'granted.txt', root);
        const grantPath = `/entries/${root.id}/direct-grants`;
        const grant = await json(ownerContext, 'POST', grantPath, 200, { csrf, data: { handle } });
        check(!!grant.id && grant.handle === handle, 'Fixture grant was not created');
        const library = await json(ownerContext, 'POST', '/libraries', 201,
            { csrf, data: { name: `Published fixture for ${handle}` } });
        const published = await upload(ownerContext, csrf, 'published.txt', library);
        const publication = await json(ownerContext, 'POST', `/libraries/${library.id}/publication`, 200, { csrf });
        check(!!publication.id, 'Fixture publication was not created');

        return async () => {
            const grants = await json(ownerContext, 'GET', grantPath, 200);
            check(grants.items.some((item) => item.id === grant.id && item.handle === handle), 'Owned grant changed');
            const incoming = await json(recipient, 'GET', '/shared', 200);
            check(incoming.items.some((item) => item.entry.id === root.id), 'Received grant disappeared');
            await expectBytes(recipient, `/shared/entries/${file.id}/content`, file.bytes);

            const managed = await json(ownerContext, 'GET', `/entries/${library.id}/direct-grants`, 200);
            check(managed.publication_id === publication.id, 'Managed publication changed');
            const visible = await json(recipient, 'GET', '/published', 200);
            check(visible.items.some((item) => item.entry.id === library.id), 'Published library disappeared');
            await expectBytes(recipient, `/shared/entries/${published.id}/content`, published.bytes);
        };
    }
    async function approve(context, csrf, subject, handle, account_id, expected = 201) {
        const response = await api(context, 'POST', '/auth/oidc/approvals', expected, {
            csrf, data: { subject, handle, ...(account_id ? { account_id } : {}) },
        });
        return expected === 201 ? (await response.json()).id : undefined;
    }
    async function revoke(context, csrf, id) {
        const link = (await json(context, 'GET', '/auth/oidc/approvals', 200)).find((item) => item.id === id);
        check(!!link, 'Cannot revoke a missing fixture link');
        await api(context, 'POST', `/auth/oidc/approvals/${id}/revoke`, 204,
            { csrf, data: { pending: link.pending, generation: link.generation } });
    }
    const me = (context) => json(context, 'GET', '/auth/oidc/session', 200);
    async function localFaults(adminContext, adminCsrf, { lastSession, lastOwner, lastFile, lastSubject, recoveryPassword }) {
        const mock = await faultProvider(fixture, clientId, readFileSync(secretFile, 'utf8'));
        try {
            step('L5 explicit replacement with a different issuer');
            check((await me(lastSession)).account_id === lastOwner, 'Replacement session was not valid before removal');
            await stop();
            removeProvider();
            activeIssuer = mock.issuer;
            writeConfig();
            await start();
            await api(lastSession, 'GET', '/me', 401);
            check((await json(adminContext, 'GET', '/auth/oidc/approvals', 200)).length === 0,
                'The replacement appeared to inherit links or approvals');
            await approve(adminContext, adminCsrf, lastSubject, 'bob', lastOwner, 409);
            check(withDb((db) => db.prepare('SELECT owner_account_id FROM entry WHERE entry_id=?').get(lastFile)).owner_account_id === lastOwner,
                'Replacement changed storage ownership');
            console.log('L5 explicit replacement: new registration could not inherit links or attach the original subject from another issuer. Earlier session was revoked and the file owner stayed unchanged.');

            step('local discovery and transport fault controls');
            const browser = await context();
            for (const fault of ['redirect', 'oversize', 'discovery_issuer', 'destination', 'duplicate_key']) {
                mock.fault(fault);
                await begin(browser, 'fault-user', null, 503);
            }
            check(mock.counts().redirected === 0 && mock.counts().exchanges === 0,
                'Untrusted discovery followed a redirect or touched the token endpoint');
            mock.fault(undefined);
            const identity = await approve(adminContext, adminCsrf, 'fault-subject', 'fault-user');

            step('L2 callback issuer and cross-attempt state controls');
            await approve(adminContext, adminCsrf, 'callback-subject', 'callback-user');
            async function callbackAttempt() {
                const instance = await context();
                const url = mock.proof(await begin(instance, 'callback-user'), 'callback-subject');
                return { instance, url };
            }
            async function validCallback() {
                const { instance, url } = await callbackAttempt();
                await callback(instance, url, 303);
                return (await me(instance)).account_id;
            }
            // Keep the request alive while all other fault cases run.
            const expiredContext = await context();
            const expiryStarted = performance.now();
            const expiring = await begin(expiredContext, 'callback-user');
            const binding = (await expiredContext.cookies(origin)).find((cookie) => cookie.name === '__Host-sylux-oidc');
            check(!!binding && binding.expires - Math.floor(Date.now() / 1000) > 290 &&
                binding.expires - Math.floor(Date.now() / 1000) <= 300, 'Attempt cookie has an unexpected deadline');

            const controlAccount = await validCallback();
            const exchanges = mock.counts().exchanges;
            const wrongIssuer = await callbackAttempt();
            wrongIssuer.url.searchParams.set('iss', mock.issuer + '/different');
            await callback(wrongIssuer.instance, wrongIssuer.url, 401);
            await api(wrongIssuer.instance, 'GET', '/me', 401);

            const firstAttempt = await callbackAttempt();
            const secondAttempt = await callbackAttempt();
            const firstState = firstAttempt.url.searchParams.get('state');
            const secondState = secondAttempt.url.searchParams.get('state');
            check(firstState !== secondState, 'State control requires distinct attempts');
            firstAttempt.url.searchParams.set('state', secondState);
            secondAttempt.url.searchParams.set('state', firstState);
            for (const { instance, url } of [firstAttempt, secondAttempt]) {
                await callback(instance, url, 401);
                await api(instance, 'GET', '/me', 401);
            }
            check(mock.counts().exchanges === exchanges, 'Invalid callback reached token exchange');

            step('L2 unsigned and other invalid token controls');
            for (const fault of ['nonce', 'issuer', 'audience', 'azp', 'expiry', 'future', 'kid', 'algorithm', 'unsigned', 'signature', 'auth_time', 'stale_auth', 'bad_json']) {
                mock.fault(fault);
                const before = mock.counts().exchanges;
                const proof = mock.proof(await begin(browser, 'callback-user'), 'callback-subject');
                await callback(browser, proof, 401);
                await api(browser, 'GET', '/me', 401);
                check(mock.counts().exchanges === before + 1, 'Token fault did not reach exchange: ' + fault);
            }
            mock.fault(undefined);
            check(await validCallback() === controlAccount, 'Valid callback control changed owner');
            mock.fault('profile');
            const profile = await context();
            await callback(profile, mock.proof(await begin(profile, 'callback-user'), 'different-key'), 401);
            await api(profile, 'GET', '/me', 401);
            const profileControl = await callbackAttempt();
            await callback(profileControl.instance, profileControl.url, 303);
            check((await me(profileControl.instance)).role === 'user', 'Provider profile claims changed the local role');
            mock.fault(undefined);

            const expiredApproval = await approve(adminContext, adminCsrf, 'expired-key', 'expired-user');
            withDb((db) => db.prepare('UPDATE external_identity SET approval_expires_at=? WHERE identity_id=?')
                .run(Math.floor(Date.now() / 1000) - 1, expiredApproval));
            check(!(await json(adminContext, 'GET', '/auth/oidc/approvals', 200)).some((link) =>
                link.id === expiredApproval), 'Expired approval remained in the current list');
            const approved = await context();
            await callback(approved, mock.proof(await begin(approved, 'expired-user'), 'expired-key'), 401);
            check(await approve(adminContext, adminCsrf, 'expired-key', 'expired-user') === expiredApproval,
                'Explicit reapproval lost the same-key reservation');
            await callback(approved, mock.proof(await begin(approved, 'expired-user'), 'expired-key'), 303);

            for (const skew of ['clock_behind', 'clock_ahead']) {
                mock.fault(skew);
                check(await validCallback() === controlAccount, 'Bounded provider clock skew changed the account');
            }
            mock.fault(undefined);
            const valid = mock.proof(await begin(browser, 'fault-user'), 'fault-subject');
            const malformed = new URL(valid);
            malformed.searchParams.append('state', malformed.searchParams.get('state'));
            await api(browser, 'HEAD', callbackPath + valid.search, 405);
            await callback(browser, malformed, 401);
            await callback(browser, valid, 401);

            step('local OIDC audit failure rolls back activation and session');
            const db = new DatabaseSync(`${fixture}/sylux/metadata/sylux.sqlite3`);
            try {
                db.exec("CREATE TRIGGER oidc_test_audit BEFORE INSERT ON event WHEN NEW.action='oidc.session_issued' BEGIN SELECT RAISE(ABORT,'fixture audit fault'); END");
                const interrupted = mock.proof(await begin(browser, 'fault-user'), 'fault-subject');
                await callback(browser, interrupted, 500);
                check(!db.prepare("SELECT 1 FROM account WHERE handle='fault-user'").get(), 'Failed issuance provisioned an account');
                check(db.prepare('SELECT state FROM external_identity WHERE identity_id=?').get(identity).state === 'pending',
                    'Failed issuance partially activated the link');
                check(!db.prepare("SELECT 1 FROM event WHERE action='oidc.link_activated' AND external_identity_id=?").get(identity),
                    'Failed issuance wrote a success audit event');
                db.exec('DROP TRIGGER oidc_test_audit');

                const first = await context();
                const second = await context();
                const one = mock.proof(await begin(first, 'fault-user'), 'fault-subject');
                const two = mock.proof(await begin(second, 'fault-user'), 'fault-subject');
                const send = async (ctx, proof) => (await ctx.request.get(origin + callbackPath + proof.search, { maxRedirects: 0, timeout: deadline })).status();
                const statuses = await Promise.all([send(first, one), send(second, two)]);
                check(statuses.sort().join(',') === '303,401', 'Concurrent activation failed to issue exactly one session');
                const winner = (await first.request.get(origin + '/me')).ok() ? first : second;
                const account = await me(winner);
                check(db.prepare('SELECT count(*) AS total FROM session WHERE external_identity_id=?').get(identity).total === 1,
                    'Concurrent activation inserted more than one session');

                step('L3 hold token exchange during local disable, then release');
                const race = await context();
                const attempt = mock.proof(await begin(race, 'fault-user'), 'fault-subject');
                const hold = mock.hold();
                const response = send(race, attempt);
                try {
                    await Promise.race([hold.entered, new Promise((_, reject) => {
                        const alarm = setTimeout(() => reject(new Error('Held exchange did not reach the token endpoint')), 3000);
                        alarm.unref();
                    })]);
                    await api(adminContext, 'POST', `/accounts/${account.account_id}/disable`, 204, { csrf: adminCsrf });
                    await api(winner, 'GET', '/me', 401);
                    await api(adminContext, 'POST', `/accounts/${account.account_id}/enable`, 204, { csrf: adminCsrf });
                } finally {
                    hold.release();
                }
                check(await response === 401, 'Disable and re-enable admitted a held callback');
                await api(winner, 'GET', '/me', 401);
                check(db.prepare('SELECT count(*) AS total FROM session WHERE external_identity_id=?').get(identity).total === 1,
                    'Held callback created a session');
                check(db.prepare("SELECT count(*) AS total FROM event WHERE action='oidc.session_issued' AND external_identity_id=?").get(identity).total === 1,
                    'Held callback reported a successful session');
                step('L3 approving administrator loses authority during token exchange');
                await api(adminContext, 'POST', '/accounts', 204, { csrf: adminCsrf,
                    data: { handle: 'second-admin', password: localPassword },
                });
                const secondAdminId = (await json(adminContext, 'GET', '/accounts', 200))
                    .find((item) => item.handle === 'second-admin').account_id;
                const changeRole = (role) => api(adminContext, 'POST', `/accounts/${secondAdminId}/role`, 204,
                    { csrf: adminCsrf, data: { role } });
                await changeRole('administrator');
                const secondary = await context();
                const credentials = await local(secondary, 'second-admin');
                const approval = await approve(secondary, credentials.csrf_token, 'held-subject', 'held-user');
                const alreadyApproved = await approve(secondary, credentials.csrf_token, 'first-subject', 'first-user');
                const completed = await context();
                await callback(completed, mock.proof(await begin(completed, 'first-user'), 'first-subject'), 303);
                const pending = await context();
                const held = mock.proof(await begin(pending, 'held-user'), 'held-subject');
                const gate = mock.hold();
                const verdict = send(pending, held);
                try {
                    await Promise.race([gate.entered, new Promise((_, reject) => {
                        const alarm = setTimeout(() => reject(new Error('Approver-race token exchange did not start')), 3000);
                        alarm.unref();
                    })]);
                    await changeRole('user');
                } finally {
                    gate.release();
                }
                check(await verdict === 401, 'Demoted administrator activated a pending link');
                check(db.prepare('SELECT state FROM external_identity WHERE identity_id=?').get(approval).state === 'pending',
                    'Demoted approver committed a link');
                check(db.prepare('SELECT state FROM external_identity WHERE identity_id=?').get(alreadyApproved).state === 'active',
                    'Demotion altered a previously completed link');
                await me(completed);
            } finally {
                db.close();
            }
            step('L6 provider saturation leaves local administrator authentication usable');
            const saturation = await Promise.all(Array.from({ length: 8 }, async () => {
                const instance = await context();
                return [instance, mock.proof(await begin(instance, 'callback-user'), 'callback-subject')];
            }));
            const gates = saturation.map(() => mock.hold());
            const active = Promise.allSettled(saturation.map(([ctx, proof]) => callback(ctx, proof, 303)));
            try {
                await Promise.race([Promise.all(gates.map((gate) => gate.entered)), wait(3000, false)]).then((reached) =>
                    check(reached !== false, 'Not all provider exchanges reached the saturation gate'));
                await begin(browser, 'callback-user', undefined, 503);
                const recovery = await context();
                const localControl = await local(recovery, 'rescue', 200, recoveryPassword);
                await api(recovery, 'POST', '/auth/logout', 204, { csrf: localControl.csrf_token });
            } finally {
                for (const gate of gates) gate.release();
            }
            check((await active).every((result) => result.status === 'fulfilled'), 'Admitted exchanges failed after release');

            step('L4 five-minute attempt boundary on the production listener');
            await wait(Math.max(0, expiryStarted + 301_000 - performance.now()));
            const overdue = mock.proof(expiring, 'callback-subject');
            const beforeExpiry = mock.counts().exchanges;
            await api(expiredContext, 'GET', callbackPath + overdue.search, 401,
                { headers: { Cookie: `${binding.name}=${binding.value}` } });
            check(mock.counts().exchanges === beforeExpiry, 'Expired attempt reached token exchange');
            check(await validCallback() === controlAccount, 'Timely proof failed after the expiry control');
            console.log('L4 real deadline: a retained browser binding was denied before exchange after five minutes. Timely signed proof still passed.');
            console.log('Local TLS faults: wrong callback issuer, state swapped between browsers, unsigned token and future iat denied. Valid signed callbacks passed before and after the faults.');
            console.log('Local fault provider: discovery limits and redirects, strict token validation, replay, atomic audit rollback, concurrent activation, and held exchange versus disable/re-enable and approver demotion.');
        } finally {
            await mock.close();
        }
    }
    try {
        step('disposable Sylux and provider initialization');
        await start();
        await provider.provision(origin + callbackPath, { mfa: true });
        const ownerPath = await createUser('alice', { mfa: true });
        const ownerSubject = (await admin(ownerPath)).id;
        const otherPath = await createUser('bob', { mfa: true });
        const otherSubject = (await admin(otherPath)).id;
        const adminContext = await context({ colorScheme: 'light', viewport: { width: 1280, height: 900 } });
        const setup = readFileSync(`${fixture}/sylux/secrets/setup.authorization`, 'utf8');
        await api(adminContext, 'POST', '/setup/admin', 201, {
            headers: { 'X-Setup-Authorization': setup },
            data: { handle: 'rescue', password: localPassword },
        });
        const administrator = await local(adminContext, 'rescue');
        let adminCsrf = administrator.csrf_token;
        await api(adminContext, 'POST', '/accounts', 204, {
            csrf: adminCsrf, data: { handle: 'alice', password: localPassword },
        });
        const ownerContext = await context({ colorScheme: 'dark', viewport: { width: 390, height: 844 } });
        const owner = await local(ownerContext, 'alice');
        const staleLocal = await context();
        await local(staleLocal, 'alice');
        const liveFile = await upload(ownerContext, owner.csrf_token, 'owner-live.txt');
        const trashFile = await upload(ownerContext, owner.csrf_token, 'owner-trash.txt');
        const files = await json(ownerContext, 'GET', '/folders/children', 200);
        const trashEntry = files.items.find((entry) => entry.id === trashFile.id);
        check(!!trashEntry, 'Uploaded Trash fixture missing from listing');
        await api(ownerContext, 'DELETE', `/entries/${trashFile.id}`, 204, {
            csrf: owner.csrf_token, headers: { 'If-Match': trashEntry.etag },
        });

        step('M4 sharing and WebDAV before local owner conversion');
        await api(adminContext, 'POST', '/accounts', 204, {
            csrf: adminCsrf, data: { handle: 'peer', password: localPassword },
        });
        const peerContext = await context();
        const peer = await local(peerContext, 'peer');
        const verifyOwned = await shareForConversion(ownerContext, owner, peerContext, 'peer');
        const verifyReceived = await shareForConversion(peerContext, peer, ownerContext, 'alice');
        await verifyOwned();
        await verifyReceived();
        const dav = await json(ownerContext, 'POST', '/me/webdav', 201, {
            csrf: owner.csrf_token, data: { name: 'Desktop', read_only: true },
        });
        const davClient = await context();
        const davPath = '/dav/Files/owner-live.txt';
        const davHeaders = { Authorization: 'Basic ' + Buffer.from('alice:' + dav.password).toString('base64') };
        await expectBytes(davClient, davPath, liveFile.bytes, davHeaders);

        step('L1 and L2 exact approval and conversion');
        const adminUi = await context({ colorScheme: 'light', viewport: { width: 1280, height: 900 } });
        const adminPage = await adminUi.newPage();
        await localUi(adminPage, 'rescue');
        await adminPage.getByRole('button', { name: 'Settings', exact: true }).click();
        await approveUi(adminPage, ownerSubject, 'alice', true);
        const links = await json(adminContext, 'GET', '/auth/oidc/approvals', 200);
        const identity = links.find((item) => item.handle === 'alice' && item.pending)?.id;
        check(!!identity && !JSON.stringify(links).includes(ownerSubject), 'Approval did not list pending state with redacted subjects');
        const wrongBrowser = await context();
        step('L2 linked account requires local password proof');
        const wrong = await proof(wrongBrowser, await begin(wrongBrowser, 'alice'), 'alice');
        await callback(wrongBrowser, wrong, 401);
        await api(wrongBrowser, 'GET', '/me', 401);
        step('L2 convert only with matching browser and credentials');
        await api(ownerContext, 'POST', '/auth/oidc/start', 401, {
            csrf: randomBytes(32).toString('base64url'),
            data: { handle: 'alice', conversion_password: localPassword },
        });
        const conversion = await proof(ownerContext, await begin(ownerContext, 'alice', owner), 'alice');
        await callback(wrongBrowser, conversion, 401);
        const ownerPage = await ownerContext.newPage();
        await ownerPage.goto(origin);
        await ownerPage.getByRole('button', { name: 'Sign in with Keycloak' }).waitFor();
        await screenshot(ownerPage, 'oidc-signin-narrow-dark');
        await localUi(ownerPage, 'alice');
        await ownerPage.getByRole('button', { name: 'Toggle Sidebar' }).click();
        await ownerPage.getByRole('button', { name: 'Account', exact: true }).click();
        await ownerPage.getByRole('button', { name: 'Switch to Keycloak' }).click();
        const switchDialog = ownerPage.getByRole('dialog', { name: 'Switch to Keycloak' });
        await switchDialog.waitFor();
        await screenshot(ownerPage, 'oidc-conversion-narrow-dark');
        await switchDialog.getByLabel('Current Sylux password').fill('an incorrect local password');
        await switchDialog.getByRole('button', { name: 'Continue to Keycloak' }).click();
        await switchDialog.getByRole('alert').waitFor();
        await switchDialog.getByLabel('Current Sylux password').and(ownerPage.locator(':focus')).waitFor();
        await switchDialog.getByLabel('Current Sylux password').fill(localPassword);
        await switchDialog.getByRole('button', { name: 'Continue to Keycloak' }).click();
        await browserFinish(ownerPage, 'alice');
        await callback(ownerContext, conversion, 401);
        await api(staleLocal, 'GET', '/me', 401);
        await local(staleLocal, 'alice', 401);
        const external = await me(ownerContext);
        const staleApproval = links.find((item) => item.id === identity);
        await api(adminContext, 'POST', `/auth/oidc/approvals/${identity}/revoke`, 409,
            { csrf: adminCsrf, data: { pending: true, generation: staleApproval.generation } });

        check(external.account_id === owner.account_id && external.role === 'user', 'Converted owner changed');
        await expectBytes(ownerContext, `/entries/${liveFile.id}/content`, liveFile.bytes);
        const trash = await json(ownerContext, 'GET', '/trash', 200);
        const trashed = trash.items.find((entry) => entry.id === trashFile.id);
        check(!!trashed, 'Converted owner Trash changed');
        await api(adminContext, 'POST', `/accounts/${owner.account_id}/reset`, 404, {
            csrf: adminCsrf, data: { password: localPassword },
        });
        await api(adminContext, 'POST', `/accounts/${owner.account_id}/role`, 409, {
            csrf: adminCsrf, data: { role: 'administrator' },
        });
        await api(ownerContext, 'POST', '/me/webdav', 409, {
            csrf: external.csrf_token, data: { name: 'Desktop', read_only: true },
        });
        step('M4 sharing and WebDAV after local owner conversion');
        await verifyOwned();
        await verifyReceived();
        await api(davClient, 'GET', davPath, 401, { headers: davHeaders });
        console.log('M4 real provider: owned and received direct grants and publications retained access in both directions. The pre-conversion WebDAV credential stopped working.');

        step('L1 and L7 new account proof and two-user isolation');
        const otherContext = await context();
        const unknown = await proof(otherContext, await begin(otherContext, 'bob'), 'bob');
        await callback(otherContext, unknown, 401);
        await approveUi(adminPage, otherSubject, 'bob');
        await approveUi(adminPage, 'fixture-canceled-approval', 'canceled');
        await adminPage.getByRole('button', { name: 'Cancel approval for canceled' }).click();
        await adminPage.getByRole('dialog', { name: 'Cancel approval for canceled?' }).getByRole('button', { name: 'Cancel approval', exact: true }).click();
        await adminPage.getByText('Approval canceled.', { exact: true }).waitFor();
        check(!(await json(adminContext, 'GET', '/auth/oidc/approvals', 200)).some((item) => item.handle === 'canceled'), 'Canceled approval remained in the list');
        const mismatch = await context();
        const wrongKey = await proof(mismatch, await begin(mismatch, 'bob'), 'alice');
        await callback(mismatch, wrongKey, 401);
        const bobBrowser = await context();
        const bobPage = await bobBrowser.newPage();
        await bobPage.goto(origin);
        await bobPage.getByRole('button', { name: 'Sign in with Keycloak' }).waitFor();
        await screenshot(bobPage, 'oidc-signin-desktop-light');
        await keycloakUi(bobPage, 'bob');
        await bobPage.reload();
        await bobPage.getByRole('navigation', { name: 'Files', exact: true }).waitFor();
        const other = await me(bobBrowser);
        const otherFile = await upload(bobBrowser, other.csrf_token, 'other-live.txt');
        await api(bobBrowser, 'GET', `/entries/${liveFile.id}/content`, 404);
        await api(ownerContext, 'GET', `/entries/${otherFile.id}/content`, 404);
        await api(adminContext, 'GET', `/entries/${liveFile.id}/content`, 404);
        const restore = `/trash/${trashFile.id}/restore`;
        const precondition = { 'If-Match': trashed.etag };
        await api(bobBrowser, 'POST', restore, 404, { csrf: other.csrf_token, headers: precondition });
        await api(adminContext, 'POST', restore, 404, { csrf: adminCsrf, headers: precondition });
        await api(ownerContext, 'POST', restore, 204, { csrf: external.csrf_token, headers: precondition });
        await expectBytes(ownerContext, `/entries/${trashFile.id}/content`, trashFile.bytes);
        console.log('L1/L2/L7 real provider: exact approval, password conversion, owner ID, bytes, Trash, cross-account denial.');

        step('L3 held callback after disable/re-enable');
        const racing = await context();
        const held = await proof(racing, await begin(racing, 'alice'), 'alice');
        await api(adminContext, 'POST', `/accounts/${owner.account_id}/disable`, 204, { csrf: adminCsrf });
        await api(adminContext, 'POST', `/accounts/${owner.account_id}/enable`, 204, { csrf: adminCsrf });
        await callback(racing, held, 401);
        await api(ownerContext, 'GET', '/me', 401);
        step('L2 provider rename preserves the external subject');
        await admin(ownerPath, { method: 'PUT', status: 204, json: { username: 'alice-renamed' } });
        check((await admin(ownerPath)).username === 'alice-renamed', 'Provider did not persist the rename');
        provider.renameFactor('alice', 'alice-renamed');
        const renamedBrowser = await context();
        const renamed = await proof(renamedBrowser, await begin(renamedBrowser, 'alice'), 'alice-renamed');
        await callback(renamedBrowser, renamed, 303);
        check((await me(renamedBrowser)).account_id === owner.account_id, 'Provider rename changed Sylux owner');

        step('L4 explicit restoration of the original subject after revoke');
        const oldCookies = await renamedBrowser.cookies(origin);
        const replay = await context();
        check(oldCookies.length > 0, 'Revocation control requires the original cookie');
        await replay.addCookies(oldCookies);
        check((await me(replay)).account_id === owner.account_id, 'Copied pre-revocation session did not authenticate as the owner');
        await revoke(adminContext, adminCsrf, identity);
        await api(renamedBrowser, 'GET', '/me', 401);
        await api(adminContext, 'POST', `/accounts/${owner.account_id}/enable`, 409, { csrf: adminCsrf });
        await approve(adminContext, adminCsrf, ownerSubject, 'different', undefined, 409);
        const unapproved = await context();
        await callback(unapproved, await proof(unapproved, await begin(unapproved, 'alice'), 'alice-renamed'), 401);
        const renewed = await approve(adminContext, adminCsrf, ownerSubject, 'alice', owner.account_id);
        check(renewed === identity, 'Restoration changed the reserved identity ID');
        const restoredOwner = await context();
        await browserSignIn(restoredOwner, await begin(restoredOwner, 'alice'), 'alice-renamed');
        check((await me(restoredOwner)).account_id === owner.account_id, 'Restoration changed the owner');
        await expectBytes(restoredOwner, `/entries/${liveFile.id}/content`, liveFile.bytes);
        await expectBytes(restoredOwner, `/entries/${trashFile.id}/content`, trashFile.bytes);
        await api(replay, 'GET', '/me', 401);
        console.log('L4 real provider: fresh approval restored the original subject, owner and exact bytes. Old sessions stayed revoked.');

        step('L5 revoked key and same-name recreation');
        await revoke(adminContext, adminCsrf, identity);
        await admin(ownerPath, { method: 'DELETE', status: 204 });
        await createUser('alice-renamed', { mfa: true });
        await approve(adminContext, adminCsrf, ownerSubject, 'alice', owner.account_id);
        const recreatedContext = await context();
        const recreated = await proof(recreatedContext, await begin(recreatedContext, 'alice'), 'alice-renamed');
        await callback(recreatedContext, recreated, 401);

        step('L8 MFA disallows password-only and wrong OTP');
        const unenrolledPath = await createUser('unenrolled');
        await approve(adminContext, adminCsrf, (await admin(unenrolledPath)).id, 'unenrolled');
        const factorTest = await context();
        const factorPage = await factorTest.newPage();
        await factorPage.goto(await begin(factorTest, 'unenrolled'));
        await submit(factorPage, 'unenrolled');
        await factorPage.locator('#kc-totp-settings').waitFor();
        await api(factorTest, 'GET', '/me', 401);
        check(!withDb((db) => db.prepare("SELECT 1 FROM account WHERE handle='unenrolled'").get()),
            'Password-only proof created the approved account');
        await factorPage.goto(await begin(factorTest, 'bob'));
        await submit(factorPage, 'bob');
        await submitFactor(factorPage, 'bob', false);
        await factorPage.getByText('Invalid authenticator code.', { exact: true }).waitFor();
        await api(factorTest, 'GET', '/me', 401);
        await browserSignIn(factorTest, await begin(factorTest, 'bob'), 'bob');
        check((await me(factorTest)).account_id === other.account_id, 'Enrolled OTP control changed owner');

        step('L4 fresh MFA after local logout with existing Keycloak SSO');
        const signedOut = await sessionCookie(bobBrowser);
        const providerCookies = (await bobBrowser.cookies(issuer + '/protocol/openid-connect/auth')).filter((cookie) => cookie.name.startsWith('KEYCLOAK'));
        check(providerCookies.length > 0, 'Fresh-proof control requires a prior Keycloak session');
        await bobPage.getByRole('button', { name: 'Log out', exact: true }).click();
        await bobPage.getByRole('button', { name: 'Sign in with Keycloak' }).waitFor();
        const replayedLogout = await context();
        await replayedLogout.addCookies([signedOut]);
        await api(replayedLogout, 'GET', '/me', 401);
        check((await bobBrowser.cookies(issuer + '/protocol/openid-connect/auth')).some((cookie) => providerCookies.some((old) =>
            old.name === cookie.name && old.value === cookie.value)), 'Local logout cleared the Keycloak login');
        await keycloakUi(bobPage, 'bob');

        step('L4 fixed session deadline from production attempt start');
        const expiryContext = await context();
        const earliest = Math.floor(Date.now() / 1000);
        const expiringStart = await begin(expiryContext, 'bob');
        const latest = Math.floor(Date.now() / 1000);
        await wait(2100);
        await callback(expiryContext, await proof(expiryContext, expiringStart, 'bob'), 303);
        const sessionId = (await sessionCookie(expiryContext)).value.split('.')[0];
        const deadlines = () => withDb((db) => db.prepare(
            'SELECT created_at, idle_expires_at, absolute_expires_at FROM session WHERE session_id=?'
        ).get(sessionId));
        const initial = deadlines();
        check(initial.absolute_expires_at >= earliest + 900 && initial.absolute_expires_at <= latest + 900 &&
            initial.idle_expires_at === initial.absolute_expires_at &&
            initial.absolute_expires_at - initial.created_at < 899, 'Deadline was computed from sign-in completion');
        await me(expiryContext);
        await expectBytes(expiryContext, `/entries/${otherFile.id}/content`, otherFile.bytes);
        check(JSON.stringify(deadlines()) === JSON.stringify(initial), 'Activity extended the fixed deadline');

        step('L4 upstream disablement fixture and preservation of active sharing');
        const activeBob = await me(bobBrowser);
        const verifyBobOutgoing = await shareForConversion(bobBrowser, activeBob, peerContext, 'peer');
        const verifyBobIncoming = await shareForConversion(peerContext, peer, bobBrowser, 'bob');
        await verifyBobOutgoing();
        await verifyBobIncoming();
        const upstreamHeld = await context();
        const heldCode = await proof(upstreamHeld, await begin(upstreamHeld, 'bob'), 'bob');
        const upstreamSso = { cookies: await bobBrowser.cookies(issuer + '/protocol/openid-connect/auth'), origins: [] };
        await admin(otherPath, { method: 'PUT', status: 204, json: { enabled: false } });
        const disabledAt = performance.now();
        await callback(upstreamHeld, heldCode, 401);
        const observed = await Promise.all([context(), context({ storageState: upstreamSso })]);
        await Promise.all(observed.map(async (attempt) => {
            const page = await attempt.newPage();
            await page.goto(await begin(attempt, 'bob'));
            await submit(page, 'bob', true);
            await page.getByText('Account is disabled, contact your administrator.', { exact: true }).waitFor();
            await api(attempt, 'GET', '/me', 401);
        }));
        console.log(`L4 pinned single-node MFA fixture: held exchange and fresh attempts with and without existing provider cookies denied by ${((performance.now() - disabledAt) / 1000).toFixed(2)} s after provider acknowledgement.`);
        await me(bobBrowser);
        await verifyBobOutgoing();
        await verifyBobIncoming();
        await admin(otherPath, { method: 'PUT', status: 204, json: { enabled: true } });

        step('L5 outbound trust failure with a reachable provider preserves sharing');
        await stop();
        writeConfig({ ca_file: fixture + '/untrusted.pem' });
        await start();
        const untrusted = await context();
        await begin(untrusted, 'bob', null, 503);
        await verifyBobOutgoing();
        await verifyBobIncoming();
        await stop();
        writeConfig();
        await start();

        step('L5 rotate the client secret without changing registration');
        const newSecret = await provider.rotateClientSecret();
        writeFileSync(secretFile, newSecret);
        await stop();
        await start();
        const retained = await context();
        await browserSignIn(retained, await begin(retained, 'bob'), 'bob');
        check((await me(retained)).account_id === other.account_id, 'Secret rotation changed the linked account');
        await verifyBobOutgoing();
        await verifyBobIncoming();

        step('L4 and L8 provider outage preserves shares, expires sessions and permits logout');
        execFileSync('docker', ['stop', '--time', '2', container], { stdio: 'ignore' });
        const unavailable = await context();
        await begin(unavailable, 'bob', null, 503);
        await me(expiryContext);
        await verifyBobOutgoing();
        await verifyBobIncoming();
        withDb((db) => {
            const expired = Math.floor(Date.now() / 1000) - 1;
            db.prepare('UPDATE session SET idle_expires_at=?,absolute_expires_at=? WHERE session_id=?')
                .run(expired, expired, sessionId);
        });
        await api(expiryContext, 'GET', '/me', 401);
        await api(expiryContext, 'GET', `/entries/${otherFile.id}/content`, 401);
        await begin(unavailable, 'bob', null, 503);
        const copiedOutage = await sessionCookie(factorTest);
        await api(factorTest, 'POST', '/auth/logout', 204, { csrf: (await me(factorTest)).csrf_token });
        const outageReplay = await context();
        await outageReplay.addCookies([copiedOutage]);
        await api(outageReplay, 'GET', '/me', 401);
        const recovery = await context();
        const rescued = await local(recovery, 'rescue');
        await api(recovery, 'POST', '/auth/logout', 204, { csrf: rescued.csrf_token });
        await api(recovery, 'GET', '/me', 401);
        console.log('L4/L8: 900-second non-sliding deadline checked in SQLite, expired request denied under outage with a disposable deadline injection, sharing preserved, both local and external logout successful.');

        step('L5 absent, partial, unreadable and mismatched configuration preserve access');
        await stop();
        await start({ configured: false });
        check(!(await json(unavailable, 'GET', '/auth/oidc/status', 200)).enabled, 'OIDC is not default-off');
        await begin(unavailable, 'bob', null, 404);
        await me(retained);
        await verifyBobOutgoing();
        for (const [reason, overrides] of [
            ['oidc_config_invalid', { callback: undefined }],
            ['oidc_config_invalid', { callback: origin + callbackPath + '?unexpected=1' }],
            ['oidc_client_secret_unavailable', { client_secret_file: secretFile + '-missing' }],
            ['oidc_ca_unavailable', { ca_file: fixture + '/not-present.pem' }],
            ['oidc_explicit_removal_required', { client_id: 'unexpected-client' }],
        ]) {
            await stop();
            writeConfig(overrides);
            await start({ reason });
            await begin(unavailable, 'bob', null, 404);
            await me(retained);
            await verifyBobOutgoing();
            await verifyBobIncoming();
        }
        await stop();
        writeConfig();
        chmodSync(secretFile, 0o644);
        await start({ reason: 'oidc_client_secret_unavailable' });
        await me(retained);
        await verifyBobOutgoing();
        await stop();
        chmodSync(secretFile, 0o600);
        console.log('L5: the production listener kept existing sessions and grants across missing settings, invalid callback, missing or exposed secret, missing CA and a mismatched registration.');

        step('L6 independent host administrator recovery while Keycloak stays stopped');
        execFileSync(executable, ['admin', 'backup', '--root', root, '--destination', fixture + '/backup'],
            { env, stdio: 'ignore' });
        const recoveryPassword = randomBytes(24).toString('base64url');
        execFileSync(executable, ['admin', 'reset-password', '--root', root,
            '--handle', 'rescue', '--password-stdin'], { env, input: recoveryPassword, stdio: ['pipe', 'ignore', 'ignore'] });
        await start();
        await api(adminContext, 'GET', '/me', 401);
        await local(adminContext, 'rescue', 401);
        const recoveryAdmin = await local(adminContext, 'rescue', 200, recoveryPassword);
        check(recoveryAdmin.account_id === administrator.account_id, 'Recovery changed the administrator ID');
        adminCsrf = recoveryAdmin.csrf_token;
        await api(adminContext, 'GET', '/setup', 404, {
            headers: { 'X-Setup-Authorization': setup },
        });
        await me(retained);
        await verifyBobOutgoing();

        step('L5 deliberate provider removal is atomic and revokes grants and publications');
        await stop();
        withDb((db) => db.exec("CREATE TRIGGER oidc_test_removal BEFORE INSERT ON event WHEN NEW.action='oidc.provider_removed' BEGIN SELECT RAISE(ABORT,'fixture audit fault'); END"));
        const refused = spawnRemoval();
        check(refused.status === 1, 'Provider removal unexpectedly committed after its audit failed');
        withDb((db) => db.exec('DROP TRIGGER oidc_test_removal'));
        await start({ configured: false });
        await me(retained);
        await verifyBobOutgoing();
        await verifyBobIncoming();
        await stop();
        removeProvider();
        await start({ configured: false });
        await api(adminContext, 'GET', '/me', 200);
        await api(retained, 'GET', '/me', 401);
        await api(adminContext, 'POST', `/accounts/${other.account_id}/enable`, 409, { csrf: adminCsrf });
        await begin(unavailable, 'bob', null, 404);
        const afterRemoval = withDb((db) => ({
            grants: db.prepare('SELECT count(*) AS n FROM direct_grant WHERE owner_account_id=? OR recipient_account_id=?').get(other.account_id, other.account_id).n,
            publications: db.prepare('SELECT count(*) AS n FROM library_publication WHERE owner_account_id=?').get(other.account_id).n,
            owner: db.prepare('SELECT owner_account_id FROM entry WHERE entry_id=?').get(otherFile.id)?.owner_account_id,
            enabled: db.prepare('SELECT enabled FROM account WHERE account_id=?').get(other.account_id)?.enabled,
            links: db.prepare("SELECT count(*) AS n FROM external_identity WHERE state<>'revoked'").get().n,
        }));
        check(afterRemoval.grants === 0 && afterRemoval.publications === 0 && afterRemoval.links === 0 &&
            afterRemoval.owner === other.account_id && afterRemoval.enabled === 0,
        'Provider removal failed to revoke access and sharing while preserving ownership');
        check((await json(adminContext, 'GET', '/auth/oidc/approvals', 200)).length === 0,
            'Old links or approvals survived provider removal');
        execFileSync('docker', ['start', container], { stdio: 'ignore' });
        await provider.ready();
        await stop();
        await start();
        const postRemoval = await context();
        await callback(postRemoval, await proof(postRemoval, await begin(postRemoval, 'bob'), 'bob'), 401);
        await approve(adminContext, adminCsrf, otherSubject, 'bob', other.account_id);
        await browserSignIn(postRemoval, await begin(postRemoval, 'bob'), 'bob');
        const restoredBob = await me(postRemoval);
        check(restoredBob.account_id === other.account_id, 'Reapproval changed the original account');
        await expectBytes(postRemoval, `/entries/${otherFile.id}/content`, otherFile.bytes);
        check(withDb((db) => db.prepare('SELECT count(*) AS n FROM direct_grant WHERE owner_account_id=? OR recipient_account_id=?')
            .get(other.account_id, other.account_id).n) === 0, 'Reapproval restored a revoked share');
        const shareRoot = (await json(postRemoval, 'GET', '/libraries', 200)).items.find((item) => item.name === 'Direct grant fixture to peer');
        const sharedAgain = await json(postRemoval, 'POST', `/entries/${shareRoot.id}/direct-grants`, 200, {
            csrf: restoredBob.csrf_token, data: { handle: 'peer' },
        });
        check(!!sharedAgain.id, 'Explicit re-sharing after restored proof failed');
        const sanitized = withDb((db) => db.prepare("SELECT * FROM event WHERE action LIKE 'oidc.%'").all());
        const audit = JSON.stringify(sanitized);
        check(sanitized.some((row) => row.action === 'oidc.provider_removed') &&
            sanitized.some((row) => row.action === 'oidc.link_activated') &&
            sanitized.some((row) => row.action === 'oidc.session_issued') &&
            sanitized.some((row) => row.action === 'oidc.session_revoked') &&
            [issuer, ownerSubject, otherSubject, clientSecret, newSecret, localPassword, recoveryPassword, fixture, otherFile.bytes.toString()]
                .every((sensitive) => !audit.includes(sensitive)), 'External audit leaked raw identity data or omitted a lifecycle event');
        console.log('L5/L6/L8: real host backup/recovery during outage, deliberate removal and audit rollback, revoked approvals, sessions, grants and publications, same owner and bytes recovered only with fresh approval and proof, explicit re-sharing, redacted lifecycle audit.');
        await localFaults(adminContext, adminCsrf, {
            lastSession: postRemoval, lastOwner: other.account_id, lastFile: otherFile.id,
            lastSubject: otherSubject, recoveryPassword,
        });
    } finally {
        try {
            await Promise.all(contexts.map((item) => item.close()));
        } finally {
            await stop();
        }
    }
}

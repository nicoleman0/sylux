// Invoked by qualify-keycloak.sh with a built test-harness fixture.
import { spawn, execFileSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import { DatabaseSync } from 'node:sqlite';
import { faultProvider } from './qualify-sylux-oidc-faults.mjs';

const deadline = 12_000;
const localPassword = randomBytes(24).toString('base64url');
const callbackPath = '/auth/oidc/callback';

export async function qualifySylux(fixture, executable, browser, provider) {
    const { check, issuer, clientId, clientSecret, createUser, admin, container, submit, step } = provider;
    mkdirSync(`${fixture}/sylux`, { mode: 0o700 });
    let activeIssuer = issuer;
    function writeConfig() {
        writeFileSync(`${fixture}/sylux-oidc.json`, JSON.stringify({
            issuer: activeIssuer, client_id: clientId, secret: clientSecret,
            attempt_seconds: 90, session_seconds: 180,
        }), { mode: 0o600 });
    }
    writeConfig();
    let process, origin;
    const contexts = [];
    async function start(mode) {
        const args = ['--fixture', fixture, mode];
        if (origin) args.push(new URL(origin).port);
        process = spawn(executable, args, { stdio: ['ignore', 'pipe', 'ignore'] });
        const lines = createInterface({ input: process.stdout });
        try {
            const [address] = await once(lines, 'line', { signal: AbortSignal.timeout(deadline) });
            check(/^https:\/\/localhost:\d+$/.test(address), 'Unexpected disposable Sylux address');
            check(!origin || origin === address, 'Disposable Sylux address changed');
            origin = address;
        } finally {
            lines.close();
        }
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
    async function context() {
        const instance = await browser.newContext({ serviceWorkers: 'block' });
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
    async function local(context, handle, expected = 200) {
        const { csrf_token: csrf } = await json(context, 'GET', '/auth/csrf', 200);
        const response = await api(context, 'POST', '/auth/local', expected, {
            csrf, data: { handle, password: localPassword },
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
            request.searchParams.get('scope') === 'openid', 'Unexpected Sylux authorization request');
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
            const completed = await response;
            check(completed.status() === 303 && completed.headers().location === '/', 'Browser callback failed');
            await page.waitForURL(origin + '/');
        } finally {
            await page.close();
        }
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
            { csrf, data: { name: 'Direct grant fixture' } });
        const file = await upload(ownerContext, csrf, 'granted.txt', root);
        const grantPath = `/entries/${root.id}/direct-grants`;
        const grant = await json(ownerContext, 'POST', grantPath, 200, { csrf, data: { handle } });
        check(!!grant.id && grant.handle === handle, 'Fixture grant was not created');
        const library = await json(ownerContext, 'POST', '/libraries', 201,
            { csrf, data: { name: 'Published fixture' } });
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
    const me = (context) => json(context, 'GET', '/auth/oidc/session', 200);
    async function localFaults(adminContext, adminCsrf) {
        const mock = await faultProvider(fixture, clientId, clientSecret);
        try {
            await stop();
            activeIssuer = mock.issuer;
            writeConfig();
            await start('enabled');

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
            for (const fault of ['nonce', 'issuer', 'audience', 'azp', 'expiry', 'future', 'kid', 'algorithm', 'unsigned', 'signature', 'auth_time', 'bad_json']) {
                mock.fault(fault);
                const before = mock.counts().exchanges;
                const proof = mock.proof(await begin(browser, 'callback-user'), 'callback-subject');
                await callback(browser, proof, 401);
                await api(browser, 'GET', '/me', 401);
                check(mock.counts().exchanges === before + 1, 'Token fault did not reach exchange: ' + fault);
            }
            mock.fault(undefined);
            check(await validCallback() === controlAccount, 'Valid callback control changed owner');
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
            console.log('Local TLS faults: wrong callback issuer, state swapped between browsers, unsigned token and future iat denied. Valid signed callbacks passed before and after the faults.');
            console.log('Local fault provider: discovery limits and redirects, strict token validation, replay, atomic audit rollback, concurrent activation, and held exchange versus disable/re-enable and approver demotion.');
        } finally {
            await mock.close();
        }
    }
    try {
        step('disposable Sylux and provider initialization');
        await start('enabled');
        await provider.provision(origin + callbackPath);
        const ownerPath = await createUser('alice');
        const ownerSubject = (await admin(ownerPath)).id;
        const otherPath = await createUser('bob');
        const otherSubject = (await admin(otherPath)).id;
        const adminContext = await context();
        const setup = readFileSync(`${fixture}/sylux/secrets/setup.authorization`, 'utf8');
        await api(adminContext, 'POST', '/setup/admin', 201, {
            headers: { 'X-Setup-Authorization': setup },
            data: { handle: 'rescue', password: localPassword },
        });
        const administrator = await local(adminContext, 'rescue');
        const adminCsrf = administrator.csrf_token;
        await api(adminContext, 'POST', '/accounts', 204, {
            csrf: adminCsrf, data: { handle: 'alice', password: localPassword },
        });
        const ownerContext = await context();
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
        const identity = await approve(adminContext, adminCsrf, ownerSubject, 'alice', owner.account_id);
        const wrongBrowser = await context();
        step('L2 linked account requires local password proof');
        const wrong = await proof(wrongBrowser, await begin(wrongBrowser, 'alice'), 'alice');
        await callback(wrongBrowser, wrong, 401);
        await api(wrongBrowser, 'GET', '/me', 401);
        step('L2 convert only with matching browser and credentials');
        const conversion = await proof(ownerContext, await begin(ownerContext, 'alice', owner), 'alice');
        await callback(wrongBrowser, conversion, 401);
        await callback(ownerContext, conversion, 303);
        await callback(ownerContext, conversion, 401);
        await api(staleLocal, 'GET', '/me', 401);
        await local(staleLocal, 'alice', 401);
        const external = await me(ownerContext);
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
        await approve(adminContext, adminCsrf, otherSubject, 'bob');
        const mismatch = await context();
        const wrongKey = await proof(mismatch, await begin(mismatch, 'bob'), 'alice');
        await callback(mismatch, wrongKey, 401);
        const bobBrowser = await context();
        await browserSignIn(bobBrowser, await begin(bobBrowser, 'bob'), 'bob');
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
        await api(adminContext, 'POST', `/auth/oidc/approvals/${identity}/revoke`, 204, { csrf: adminCsrf });
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
        await api(adminContext, 'POST', `/auth/oidc/approvals/${identity}/revoke`, 204, { csrf: adminCsrf });
        await admin(ownerPath, { method: 'DELETE', status: 204 });
        await createUser('alice-renamed');
        await approve(adminContext, adminCsrf, ownerSubject, 'alice', owner.account_id);
        const recreatedContext = await context();
        const recreated = await proof(recreatedContext, await begin(recreatedContext, 'alice'), 'alice-renamed');
        await callback(recreatedContext, recreated, 401);

        step('L6 outage and local login and L8 app logout');
        const retained = await context();
        await browserSignIn(retained, await begin(retained, 'bob'), 'bob');
        check((await me(retained)).account_id === other.account_id, 'Repeat sign-in changed account');
        execFileSync('docker', ['stop', '--time', '2', container], { stdio: 'ignore' });
        const unavailable = await context();
        await begin(unavailable, 'bob', null, 503);
        const recovery = await context();
        const rescued = await local(recovery, 'rescue');
        await api(recovery, 'POST', '/auth/logout', 204, { csrf: rescued.csrf_token });
        await api(recovery, 'GET', '/me', 401);
        await api(bobBrowser, 'POST', '/auth/logout', 204, { csrf: other.csrf_token });
        await api(bobBrowser, 'GET', '/me', 401);

        step('L5 provider configuration removed at restart');
        await stop();
        await start('disabled');
        await api(adminContext, 'GET', '/me', 200);
        await api(retained, 'GET', '/me', 401);
        await api(adminContext, 'POST', `/accounts/${other.account_id}/enable`, 409, { csrf: adminCsrf });
        await begin(unavailable, 'bob', null, 404);
        console.log('L3/L5/L6/L8 real provider: stale callback denial, rename, key reservation, recreated identity denial, provider removal, local login/logout during outage.');
        await localFaults(adminContext, adminCsrf);
    } finally {
        try {
            await Promise.all(contexts.map((item) => item.close()));
        } finally {
            await stop();
        }
    }
}

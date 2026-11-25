import { execFileSync } from 'node:child_process';
import { createHash, createPublicKey, randomBytes, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer, request as send } from 'node:https';
import { once } from 'node:events';
import { setTimeout as wait } from 'node:timers/promises';
import { chromium } from '../web/node_modules/playwright/index.mjs';

const [fixture, container, origin] = process.argv.slice(2);
const realm = 'qualification';
const issuer = `${origin}/realms/${realm}`;
const clientId = 'fixture-rp';
const random = () => randomBytes(32).toString('base64url');
const clientSecret = random();
const password = random();
const ca = readFileSync(`${fixture}/ca.pem`);
const cert = readFileSync(`${fixture}/tls/leaf.pem`);
const key = readFileSync(`${fixture}/tls/leaf.key`);
const now = () => Math.floor(Date.now() / 1000);
const deadline = 5_000;
const codeLifetime = 20;
const tokenLifetime = 60;
let step = 'initialization';

function check(condition, message) {
    if (!condition) throw new Error(message);
}

// Return only bounded responses and keep provider bodies out of diagnostics.
async function request(url, { status = 200, basic, form, bearer, json, method } = {}) {
    const endpoint = new URL(url);
    check(endpoint.origin === origin && !endpoint.username && !endpoint.password && !endpoint.hash,
        'Unexpected provider destination');
    const headers = {};
    if (basic) headers.Authorization = `Basic ${Buffer.from(basic.join(':')).toString('base64')}`;
    if (bearer) headers.Authorization = `Bearer ${bearer}`;
    if (form) headers['Content-Type'] = 'application/x-www-form-urlencoded';
    if (json) headers['Content-Type'] = 'application/json';
    const response = await new Promise((resolve, reject) => {
        const req = send(url, { ca, rejectUnauthorized: true, headers, method, signal: AbortSignal.timeout(deadline) }, (res) => {
            const chunks = [];
            let size = 0;
            res.on('data', (chunk) => {
                size += chunk.length;
                if (size > 1024 * 1024) req.destroy(new Error('Provider response exceeded 1 MiB'));
                else chunks.push(chunk);
            });
            res.on('error', reject);
            res.on('end', () => resolve({ status: res.statusCode, body: Buffer.concat(chunks) }));
        });
        req.on('error', reject);
        req.end(form ? new URLSearchParams(form).toString() : json && JSON.stringify(json));
    });
    check(response.status === status, `Expected provider HTTP ${status}, got ${response.status}`);
    return response.body.length && (status === 200 || status === 400) ? JSON.parse(response.body) : null;
}

function networkFailure(error) {
    return ['ECONNREFUSED', 'ECONNRESET', 'ABORT_ERR'].includes(error.code);
}

async function ready(url) {
    const end = performance.now() + 120_000;
    while (performance.now() < end) {
        try {
            return await request(url);
        } catch (error) {
            if (!networkFailure(error) && error.message !== 'Expected provider HTTP 200, got 503') throw error;
            await wait(500);
        }
    }
    throw new Error('Provider did not become ready');
}

const discoveryPath = '/.well-known/openid-configuration';
const adminTokenUrl = `${origin}/realms/master/protocol/openid-connect/token`;
async function admin(path, options) {
    const token = await request(adminTokenUrl, {
        method: 'POST', basic: ['fixture-admin', process.env.KC_BOOTSTRAP_ADMIN_CLIENT_SECRET],
        form: { grant_type: 'client_credentials' },
    });
    return request(`${origin}/admin/realms${path}`, { ...options, bearer: token.access_token });
}

const clientAttributes = {
    'pkce.code.challenge.method': 'S256',
    'client.secret.authentication.allowed.method': 'client_secret_basic',
    'id.token.signed.response.alg': 'RS256',
    'use.refresh.tokens': 'false',
};

async function provision(callback) {
    await ready(`${origin}/realms/master${discoveryPath}`);
    await admin('', {
        method: 'POST', status: 201, json: {
            realm, enabled: true, sslRequired: 'all', editUsernameAllowed: true,
            accessCodeLifespan: codeLifetime, accessTokenLifespan: tokenLifetime,
            clients: [{
                clientId, protocol: 'openid-connect', publicClient: false,
                clientAuthenticatorType: 'client-secret', secret: clientSecret,
                standardFlowEnabled: true, directAccessGrantsEnabled: false,
                implicitFlowEnabled: false, serviceAccountsEnabled: false,
                redirectUris: [callback], defaultClientScopes: ['basic'], optionalClientScopes: [],
                attributes: clientAttributes,
            }],
        },
    });
    const effectiveRealm = await admin(`/${realm}`);
    check(effectiveRealm.accessCodeLifespan === codeLifetime &&
        effectiveRealm.accessTokenLifespan === tokenLifetime, 'Unexpected effective realm lifetimes');
    const [client] = await admin(`/${realm}/clients?clientId=${clientId}`);
    check(client?.standardFlowEnabled && client.publicClient === false &&
        client.clientAuthenticatorType === 'client-secret' && !client.directAccessGrantsEnabled &&
        !client.implicitFlowEnabled && !client.serviceAccountsEnabled &&
        client.redirectUris.length === 1 && client.redirectUris[0] === callback &&
        client.defaultClientScopes.length === 1 && client.defaultClientScopes[0] === 'basic' &&
        client.optionalClientScopes.length === 0 &&
        Object.entries(clientAttributes).every(([name, value]) => client.attributes[name] === value),
    'Unexpected effective relying-party configuration');
}

async function createUser(username) {
    const usersPath = `/${realm}/users`;
    await admin(usersPath, {
        method: 'POST', status: 201, json: {
            username, enabled: true, firstName: 'Fixture', lastName: 'User',
            email: `${username}@example.invalid`, emailVerified: true,
            credentials: [{ type: 'password', value: password, temporary: false }],
        },
    });
    const users = await admin(`${usersPath}?exact=true&username=${username}`);
    check(users.length === 1 && users[0].enabled, 'Fixture user unavailable');
    return `${usersPath}/${encodeURIComponent(users[0].id)}`;
}

async function discover() {
    const metadata = await ready(issuer + discoveryPath);
    check(metadata.issuer === issuer && metadata.response_types_supported.includes('code') &&
        metadata.grant_types_supported.includes('authorization_code') &&
        metadata.subject_types_supported.includes('public') &&
        metadata.code_challenge_methods_supported.includes('S256') &&
        metadata.token_endpoint_auth_methods_supported.includes('client_secret_basic') &&
        metadata.id_token_signing_alg_values_supported.includes('RS256'), 'Unexpected discovery profile');
    const protocol = `${issuer}/protocol/openid-connect`;
    check(metadata.authorization_endpoint === `${protocol}/auth` &&
        metadata.token_endpoint === `${protocol}/token` &&
        metadata.jwks_uri === `${protocol}/certs`, 'Unexpected discovery endpoints');
    const { keys } = await request(metadata.jwks_uri);
    return { metadata, keys };
}

function attempt(provider, callback, fresh) {
    const verifier = random();
    const state = random();
    const nonce = random();
    const parameters = new URLSearchParams({
        client_id: clientId, response_type: 'code', redirect_uri: callback,
        scope: 'openid', state, nonce, code_challenge_method: 'S256',
        code_challenge: createHash('sha256').update(verifier).digest('base64url'),
        ...(fresh ? { max_age: '0' } : { prompt: 'none' }),
    });
    return { verifier, state, nonce, started: now(), deadline: performance.now() + 60_000,
        url: `${provider.metadata.authorization_endpoint}?${parameters}` };
}

function callbackCode(page, callback, transaction) {
    const url = new URL(page.url());
    check(`${url.origin}${url.pathname}` === callback && !url.hash &&
        url.searchParams.getAll('iss').length === 1 && url.searchParams.get('iss') === issuer &&
        url.searchParams.getAll('state').length === 1 && url.searchParams.get('state') === transaction.state &&
        url.searchParams.getAll('code').length === 1 && url.searchParams.get('code') &&
        !url.searchParams.has('error'), 'Unexpected authorization callback');
    return url.searchParams.get('code');
}

function signedClaims(token, keys) {
    const segments = token.split('.');
    check(segments.length === 3, 'Unexpected ID token shape');
    const [head, body, signature] = segments;
    const header = JSON.parse(Buffer.from(head, 'base64url'));
    const matching = keys.filter((jwk) => jwk.kid === header.kid &&
        jwk.kty === 'RSA' && jwk.use === 'sig' && jwk.alg === 'RS256');
    check(header.alg === 'RS256' && typeof header.kid === 'string' && matching.length === 1,
        'Unexpected ID token signing key');
    check(verify('RSA-SHA256', Buffer.from(`${head}.${body}`),
        createPublicKey({ key: matching[0], format: 'jwk' }), Buffer.from(signature, 'base64url')),
    'ID token signature invalid');
    return JSON.parse(Buffer.from(body, 'base64url'));
}

async function exchange(provider, callback, transaction, code, { status = 200, fresh = true } = {}) {
    check(!transaction.consumed && performance.now() < transaction.deadline, 'Authorization attempt spent or expired');
    transaction.consumed = true;
    const tokens = await request(provider.metadata.token_endpoint, {
        status, method: 'POST', basic: [clientId, clientSecret],
        form: { grant_type: 'authorization_code', redirect_uri: callback,
            code_verifier: transaction.verifier, code },
    });
    if (status !== 200) {
        check(tokens?.error === 'invalid_grant', 'Expected authorization-code grant denial');
        return null;
    }
    check(typeof tokens.id_token === 'string' && !tokens.refresh_token &&
        !tokens.scope?.split(' ').includes('offline_access'), 'Unexpected token response');
    const claims = signedClaims(tokens.id_token, provider.keys);
    const audiences = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    check(claims.iss === issuer && typeof claims.sub === 'string' && claims.sub.length > 0 &&
        audiences.length === 1 && audiences[0] === clientId &&
        (claims.azp === undefined || claims.azp === clientId) && claims.nonce === transaction.nonce &&
        Number.isInteger(claims.exp) && claims.exp > now() &&
        Number.isInteger(claims.iat) && claims.iat >= transaction.started && claims.iat <= now() &&
        claims.exp - claims.iat === tokenLifetime &&
        (claims.nbf === undefined || (Number.isInteger(claims.nbf) && claims.nbf <= now())),
    'ID token claims invalid');
    if (fresh) {
        check(Number.isInteger(claims.auth_time) && claims.auth_time >= transaction.started &&
            claims.auth_time <= claims.iat, 'ID token did not prove fresh authentication');
    }
    return { claims, idToken: tokens.id_token };
}

const loginForm = (page) => page.locator('#password');
async function submit(page, username) {
    const action = await page.locator('#kc-form-login').getAttribute('action');
    check(new URL(action, page.url()).origin === origin, 'Unexpected login form destination');
    const usernameField = page.locator('#username');
    if (await usernameField.isVisible()) await usernameField.fill(username);
    await loginForm(page).fill(password);
    await page.locator('#kc-login').click();
}

async function start(page, provider, callback, fresh = true) {
    const transaction = attempt(provider, callback, fresh);
    await page.goto(transaction.url);
    return transaction;
}

async function authorize(page, provider, callback, username) {
    const transaction = await start(page, provider, callback);
    await loginForm(page).waitFor();
    const beforeSubmit = performance.now();
    await submit(page, username);
    await page.waitForURL(`${callback}?*`);
    return { transaction, beforeSubmit, code: callbackCode(page, callback, transaction) };
}

async function signIn(page, provider, callback, username) {
    const { transaction, code } = await authorize(page, provider, callback, username);
    return exchange(provider, callback, transaction, code);
}

async function denied(page, callback, transaction, username) {
    await loginForm(page).waitFor();
    await submit(page, username);
    // Require a provider-rendered credential denial. A transport failure cannot pass.
    await page.getByText(/^(Invalid username or password\.|Account is disabled.*)$/i).first().waitFor();
    check(new URL(page.url()).origin === origin && !transaction.consumed &&
        !page.url().startsWith(callback), 'Unexpected browser outcome after disablement');
}

async function browserContext(browser, callback, storageState) {
    const context = await browser.newContext({ storageState, serviceWorkers: 'block' });
    context.setDefaultTimeout(deadline);
    context.setDefaultNavigationTimeout(deadline);
    await context.route('**/*', (route) => {
        const destination = new URL(route.request().url());
        return [origin, new URL(callback).origin].includes(destination.origin)
            ? route.continue() : route.abort();
    });
    return context;
}

async function run(browser, callback) {
    step = 'provisioning and discovery';
    await provision(callback);
    let provider = await discover();
    console.log('TLS, discovery, JWKS and effective client and realm settings passed.');

    const context = await browserContext(browser, callback);
    const page = await context.newPage();
    const originalName = 'fixture-original';
    const renamed = 'fixture-renamed';
    let user = await createUser(originalName);

    step = 'fresh proof and SSO control';
    const first = await signIn(page, provider, callback, originalName);
    await wait(1100);
    const silent = await start(page, provider, callback, false);
    const sso = await exchange(provider, callback, silent, callbackCode(page, callback, silent), { fresh: false });
    check(sso.claims.sub === first.claims.sub && Number.isInteger(sso.claims.auth_time) &&
        sso.claims.auth_time < silent.started, 'Silent SSO control did not use earlier authentication');
    const fresh = await signIn(page, provider, callback, originalName);
    check(fresh.claims.sub === first.claims.sub && fresh.claims.auth_time > sso.claims.auth_time,
        'Existing SSO did not yield a distinct fresh proof');
    console.log('Silent SSO used earlier authentication. max_age=0 displayed the form and produced a newer signed auth_time.');

    step = 'rename continuity';
    await admin(user, { method: 'PUT', status: 204, json: { username: renamed } });
    await wait(1100);
    const beforeDisable = await signIn(page, provider, callback, renamed);
    check(first.claims.iss === beforeDisable.claims.iss && first.claims.sub === beforeDisable.claims.sub,
        'Rename changed the verified identity key');
    console.log('Rename preserved the verified issuer and subject.');

    step = 'disablement';
    const existingSso = await context.storageState();
    await wait(1100);
    const held = await authorize(await context.newPage(), provider, callback, renamed);
    const heldForm = await context.newPage();
    await wait(1100);
    const pending = await start(heldForm, provider, callback);
    await loginForm(heldForm).waitFor();
    const writeStarted = performance.now();
    await admin(user, { method: 'PUT', status: 204, json: { enabled: false } });
    const acknowledged = performance.now();
    await exchange(provider, callback, held.transaction, held.code, { status: 400 });
    const heldAge = (performance.now() - held.beforeSubmit) / 1000;
    check(heldAge < codeLifetime, 'Held code could have expired during disablement probe');
    await denied(heldForm, callback, pending, renamed);
    check((await admin(user)).enabled === false, 'Disable acknowledgement did not persist');
    const old = signedClaims(beforeDisable.idToken, provider.keys);
    check(old.exp > now(), 'Previously issued ID token expired before observation');
    console.log(`Disable write and acknowledgement window ${((acknowledged - writeStarted) / 1000).toFixed(2)} s.`);
    console.log(`Held code (age at most ${heldAge.toFixed(2)} s) got invalid_grant. Pending form denied.`);
    console.log(`Previously issued ID token signature still verified offline with ${old.exp - now()} s to expiry.`);

    for (const offset of [0, 5, 30, 60, 120, 180, 240, 290, 300]) {
        await wait(Math.max(0, acknowledged + offset * 1000 - performance.now()));
        const sampleStart = performance.now();
        await Promise.all([undefined, existingSso].map(async (storageState) => {
            const probe = await browserContext(browser, callback, storageState);
            try {
                const probePage = await probe.newPage();
                const transaction = await start(probePage, provider, callback);
                await denied(probePage, callback, transaction, renamed);
            } finally {
                await probe.close();
            }
        }));
        const elapsed = (time) => ((time - acknowledged) / 1000).toFixed(2);
        console.log(`Both fresh probes denied: +${elapsed(sampleStart)}..+${elapsed(performance.now())} s from acknowledgement.`);
    }
    check(old.exp <= now(), 'Prior token still inside nominal lifetime after observation window');
    console.log('Prior ID token passed its signed expiry during the sampling window.');

    step = 'same-name replacement and code expiry control';
    await admin(user, { method: 'DELETE', status: 204 });
    user = await createUser(renamed);
    const replacement = await signIn(page, provider, callback, renamed);
    check(replacement.claims.iss === first.claims.iss && replacement.claims.sub !== first.claims.sub,
        'Replacement reused the verified identity key');
    console.log('Deleting and recreating the same name preserved issuer and produced a different verified subject.');
    await wait(1100);
    const expired = await authorize(page, provider, callback, renamed);
    await wait((codeLifetime + 2) * 1000);
    await exchange(provider, callback, expired.transaction, expired.code, { status: 400 });
    console.log(`Enabled-user code age at least ${codeLifetime + 2} s got HTTP 400 (configured ${codeLifetime} s).`);

    step = 'actual provider outage';
    const interrupted = await authorize(page, provider, callback, renamed);
    execFileSync('docker', ['stop', '--time', '5', container], { stdio: 'ignore', timeout: 20_000 });
    const outageStart = performance.now();
    const outage = await Promise.allSettled([
        start(await context.newPage(), provider, callback),
        exchange(provider, callback, interrupted.transaction, interrupted.code),
    ]);
    check(outage[0].status === 'rejected' && /net::ERR_(CONNECTION_REFUSED|CONNECTION_RESET)/.test(outage[0].reason.message),
        'Stopped provider did not give a browser connection failure');
    check(outage[1].status === 'rejected' && networkFailure(outage[1].reason),
        'Stopped provider did not give a bounded token connection failure');
    console.log(`Stopped-provider browser and token requests failed in ${((performance.now() - outageStart) / 1000).toFixed(2)} s (5 s per request limit).`);

    step = 'recovery';
    const recoveryStart = performance.now();
    execFileSync('docker', ['start', container], { stdio: 'ignore', timeout: 20_000 });
    provider = await discover();
    const recovered = await signIn(page, provider, callback, renamed);
    check(recovered.claims.iss === replacement.claims.iss && recovered.claims.sub === replacement.claims.sub,
        'Recovery changed the verified identity key');
    console.log(`Restart, validated discovery and a new fresh proof passed in ${((performance.now() - recoveryStart) / 1000).toFixed(2)} s.`);
}

async function main() {
    const server = createServer({ cert, key }, (req, res) => {
        res.writeHead(req.method === 'GET' && req.url.startsWith('/callback?') ? 200 : 404,
            { 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer' });
        res.end();
    }).listen(0, '127.0.0.1');
    let browser;
    try {
        await once(server, 'listening');
        const callback = `https://127.0.0.1:${server.address().port}/callback`;
        const pin = createHash('sha256').update(createPublicKey(cert).export({ type: 'spki', format: 'der' })).digest('base64');
        const env = { ...process.env };
        delete env.KC_BOOTSTRAP_ADMIN_CLIENT_SECRET;
        browser = await chromium.launch({ env, args: [`--ignore-certificate-errors-spki-list=${pin}`] });
        console.log(`Node ${process.version}, Chromium ${browser.version()}.`);
        await run(browser, callback);
    } finally {
        try {
            await browser?.close();
        } finally {
            server.close();
        }
    }
}

try {
    await main();
} catch (error) {
    // Playwright errors can contain callback URLs or form input. Emit only fixture-owned diagnostics.
    const detail = error.constructor === Error && !error.stack?.includes('node_modules') &&
        error.stack?.split('\n')[1]?.includes('at check (') ? ` ${error.message}` : '';
    console.error(`Qualification failed during ${step}.${detail}`);
    process.exitCode = 1;
}

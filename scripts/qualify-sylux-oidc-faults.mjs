import { createHash, createPrivateKey, createPublicKey, randomBytes, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:https';
import { once } from 'node:events';

const random = () => randomBytes(32).toString('base64url');
const epoch = () => Math.floor(Date.now() / 1000);
const digest = (text) => createHash('sha256').update(text).digest('base64url');
const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');

export async function faultProvider(fixture, clientId, clientSecret) {
    const key = readFileSync(`${fixture}/tls/leaf.key`);
    const cert = readFileSync(`${fixture}/tls/leaf.pem`);
    const privateKey = createPrivateKey(key);
    const jwk = { ...createPublicKey(privateKey).export({ format: 'jwk' }), alg: 'RS256', use: 'sig', kid: 'fixture-key' };
    const attempts = new Map();
    let issuer, fault, gate;
    let redirected = 0, exchanges = 0;
    const server = createServer({ cert, key }, async (request, response) => {
        try {
            await handle(request, response);
        } catch {
            response.writeHead(500).end();
        }
    });

    async function handle(request, response) {
        const json = (value, status = 200) =>
            response.writeHead(status, { 'Content-Type': 'application/json' }).end(JSON.stringify(value));
        switch (request.url) {
        case '/.well-known/openid-configuration':
            if (fault === 'redirect') return response.writeHead(302, { location: issuer + '/redirected' }).end();
            if (fault === 'oversize') return response.writeHead(200, { 'Content-Type': 'application/json' }).end(Buffer.alloc(1024 * 1024 + 1));
            return json({
                issuer: fault === 'discovery_issuer' ? issuer + '/different' : issuer,
                authorization_endpoint: issuer + '/authorize',
                token_endpoint: (fault === 'destination' ? issuer.replace('127.0.0.1', 'localhost') : issuer) + '/token',
                jwks_uri: issuer + '/jwks',
                response_types_supported: ['code'],
                response_modes_supported: ['query'],
                subject_types_supported: ['public'],
                grant_types_supported: ['authorization_code'],
                code_challenge_methods_supported: ['S256'],
                token_endpoint_auth_methods_supported: ['client_secret_basic'],
                id_token_signing_alg_values_supported: ['RS256'],
                authorization_response_iss_parameter_supported: true,
            });
        case '/redirected':
            redirected++;
            return response.writeHead(500).end();
        case '/jwks':
            return json({ keys: fault === 'duplicate_key' ? [jwk, jwk] : [jwk] });
        case '/token': {
            exchanges++;
            let body = '';
            for await (const chunk of request) {
                body += chunk;
                if (body.length > 32768) return response.writeHead(413).end();
            }
            const form = new URLSearchParams(body);
            const code = form.get('code');
            const attempt = attempts.get(code);
            attempts.delete(code);
            if (request.method !== 'POST' || !attempt ||
                request.headers.authorization !== 'Basic ' + Buffer.from(clientId + ':' + clientSecret).toString('base64') ||
                form.get('grant_type') !== 'authorization_code' ||
                !form.get('code_verifier') || digest(form.get('code_verifier')) !== attempt.challenge ||
                form.get('redirect_uri') !== attempt.callback) {
                return json({ error: 'invalid_grant' }, 400);
            }
            if (gate) {
                const held = gate;
                gate = undefined;
                held.enter();
                await held.release;
            }
            if (fault === 'bad_json') return response.writeHead(200, { 'Content-Type': 'application/json' }).end('{');
            const header = {
                alg: fault === 'algorithm' ? 'RS512' : 'RS256',
                kid: fault === 'kid' ? 'wrong-kid' : jwk.kid,
                typ: 'JWT',
            };
            const now = epoch();
            const claims = {
                iss: fault === 'issuer' ? issuer + '/different' : issuer,
                aud: fault === 'audience' ? 'wrong-client' : clientId,
                azp: fault === 'azp' ? 'wrong-client' : clientId,
                sub: attempt.subject,
                nonce: fault === 'nonce' ? random() : attempt.nonce,
                iat: fault === 'future' ? now + 3600 : now,
                exp: fault === 'expiry' ? now - 1 : now + 30,
                auth_time: fault === 'auth_time' ? undefined : now,
            };
            const signed = encode(header) + '.' + encode(claims);
            const signature = sign('RSA-SHA256', Buffer.from(signed), privateKey);
            if (fault === 'signature') signature[0] ^= 1;
            return json({
                token_type: 'Bearer', access_token: random(), expires_in: 30, scope: 'openid',
                id_token: signed + '.' + signature.toString('base64url'),
                refresh_token: 'disposable-provider-value',
            });
        }
        default:
            return response.writeHead(404).end();
        }
    }

    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    issuer = 'https://127.0.0.1:' + server.address().port;

    return {
        issuer,
        fault(value) { fault = value; },
        counts() { return { redirected, exchanges }; },
        hold() {
            const { promise: entered, resolve: enter } = Promise.withResolvers();
            const { promise: release, resolve } = Promise.withResolvers();
            gate = { enter, release };
            return { entered, release: resolve };
        },
        proof(authorization, subject) {
            const query = new URL(authorization).searchParams;
            const callback = new URL(query.get('redirect_uri'));
            const code = random();
            attempts.set(code, { subject, nonce: query.get('nonce'), challenge: query.get('code_challenge'), callback: callback.href });
            callback.searchParams.set('code', code);
            callback.searchParams.set('state', query.get('state'));
            callback.searchParams.set('iss', issuer);
            return callback;
        },
        async close() {
            const closed = once(server, 'close');
            server.close();
            server.closeAllConnections();
            await closed;
        },
    };
}

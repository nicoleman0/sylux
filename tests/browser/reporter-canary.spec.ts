import path from 'node:path';
import { createRequire } from 'node:module';
const requireWeb = createRequire(path.resolve(__dirname, '../../web/package.json'));
const { test }: typeof import('../../web/node_modules/@playwright/test') = requireWeb('@playwright/test');
const canary = process.env.S1_REPORTER_CANARY!;
test(`success ${canary}`, async ({}, info) => {
  process.stdout.write(canary);
  process.stderr.write(canary);
  await info.attach('secret-body', { body: canary, contentType: 'text/plain' });
});
test(`failure ${canary}`, async ({}, info) => {
  await info.attach('secret-body', { body: canary, contentType: 'text/plain' });
  throw new Error(`Authorization Cookie password body ${canary}`);
});

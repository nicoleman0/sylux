import { readFileSync } from 'node:fs';
import { createServer, request } from 'node:http';
import { expect, test, type Locator } from '@playwright/test';

const pattern = readFileSync(new URL('./fixtures/media/pattern.mp4', import.meta.url));
const tone = readFileSync(new URL('./fixtures/media/tone.mp3', import.meta.url));
const files = [
  { id: 'pattern', name: 'pattern.mp4', bytes: pattern },
  { id: 'tone', name: 'tone.mp3', bytes: tone },
  { id: 'broken', name: 'broken.mp4', bytes: Buffer.from('Undecodable media fixture') },
  { id: 'pending', name: 'pending.mp3', bytes: tone },
];
const root = { id: 'root', parent_id: null, name: 'Files', kind: 'folder', etag: '"root:1"' };
const items = files.map(({ id, name, bytes }) => ({
  id, name, size: bytes.length, kind: 'file', parent_id: root.id, etag: '"1"',
}));

async function servePreview(vite: string) {
  const mediaRequests: { id: string; destination: string | undefined; range: string | undefined }[] = [];
  let heldRequests = 0;
  let canceledRequests = 0;
  const server = createServer((incoming, response) => {
    const path = new URL(incoming.url!, vite).pathname;
    const file = files.find(({ id }) => path === `/entries/${id}/content`);
    if (file) {
      const range = incoming.headers.range;
      mediaRequests.push({ id: file.id, range, destination: incoming.headers['sec-fetch-dest'] as string | undefined });
      const bounds = range?.match(/^bytes=(\d+)-(\d*)$/);
      const start = Number(bounds?.[1] ?? 0);
      const end = Math.min(Number(bounds?.[2] || file.bytes.length - 1), file.bytes.length - 1);
      const content = file.bytes.subarray(start, end + 1);
      response.writeHead(range ? 206 : 200, {
        'Accept-Ranges': 'bytes',
        'Content-Type': 'application/octet-stream',
        'Content-Disposition': 'attachment',
        'Content-Length': content.length,
        'Cache-Control': 'no-store',
        'X-Content-Type-Options': 'nosniff',
        ...(range && { 'Content-Range': `bytes ${start}-${end}/${file.bytes.length}` }),
      });
      if (file.id === 'pending') {
        heldRequests++;
        response.on('close', () => { if (!response.writableFinished) canceledRequests++; });
        response.write(content.subarray(0, 16));
      } else {
        response.end(content);
      }
      return;
    }

    const replies: Record<string, unknown> = {
      '/auth/csrf': { csrf_token: 'csrf' },
      '/auth/local': { account_id: 'owner', handle: 'owner', role: 'user', csrf_token: 'csrf' },
      '/folders/children': { root, items, next_cursor: null, has_more: false },
      '/folders/root/children': { items, next_cursor: null, has_more: false },
    };
    if (path in replies) {
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify(replies[path]));
      return;
    }

    const upstream = request(new URL(incoming.url!, vite), { method: incoming.method }, asset => {
      response.writeHead(asset.statusCode!, asset.headers);
      asset.pipe(response);
    });
    upstream.on('error', () => response.destroy());
    response.on('close', () => upstream.destroy());
    upstream.end();
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Expected test HTTP port');
  return {
    origin: `http://127.0.0.1:${address.port}`,
    mediaRequests,
    get heldRequests() { return heldRequests; },
    get canceledRequests() { return canceledRequests; },
    close() { server.close(); server.closeAllConnections(); },
  };
}

async function playAndSeek(media: Locator) {
  await media.evaluate((element: HTMLMediaElement) => { element.muted = true; });
  await media.press('Space');
  await expect.poll(() => media.evaluate((element: HTMLMediaElement) => element.currentTime)).toBeGreaterThan(0.1);
  const middle = await media.evaluate(async (element: HTMLMediaElement) => {
    const seeked = new Promise<void>(resolve => element.addEventListener('seeked', () => resolve(), { once: true }));
    const middle = element.duration / 2;
    element.currentTime = middle;
    await seeked;
    return middle;
  });
  expect(middle).toBeGreaterThan(3);
  await expect.poll(() => media.evaluate((element: HTMLMediaElement) => element.currentTime)).toBeGreaterThan(middle + 0.1);
}

async function tabTo(control: Locator) {
  for (let tab = 0; tab < 16; tab++) {
    if (await control.evaluate(element => element === document.activeElement)) break;
    await control.page().keyboard.press('Tab');
  }
  await expect(control).toBeFocused();
}

for (const [theme, width] of [['light', 1280], ['light', 390], ['dark', 1280], ['dark', 390]] as const) {
  test(`native media playback, seeking and cleanup in ${theme} at ${width}px`, async ({ page, browser, baseURL }, testInfo) => {
    const fixture = await servePreview(baseURL!);
    try {
      await page.setViewportSize({ width, height: 600 });
      await page.emulateMedia({ colorScheme: theme });
      await page.goto(fixture.origin);
      const codecs = await page.evaluate(() => ({
        h264: document.createElement('video').canPlayType('video/mp4; codecs="avc1.42c01e"'),
        mp3: document.createElement('audio').canPlayType('audio/mpeg'),
      }));
      await testInfo.attach('native codec support', {
        body: JSON.stringify({ chromium: browser.version(), ...codecs }), contentType: 'application/json',
      });
      expect(codecs.h264, 'Smoke Chromium must support the H.264 fixture').toMatch(/maybe|probably/);
      expect(codecs.mp3, 'Smoke Chromium must support the MP3 fixture').toMatch(/maybe|probably/);
      await page.getByLabel('Account name').fill('owner');
      await page.getByLabel('Password', { exact: true }).fill('password');
      await page.getByRole('button', { name: 'Sign in', exact: true }).click();
      await expect.poll(() => page.locator('html').evaluate(el => el.classList.contains('dark'))).toBe(theme === 'dark');

      const dialog = page.getByRole('dialog');
      const open = (filename: string) => page.getByRole('button', { name: `Preview ${filename}`, exact: true });
      const close = dialog.getByRole('button', { name: 'Close preview', exact: true });
      const download = dialog.getByRole('button', { name: 'Download file', exact: true });

      for (const [kind, file] of [['video', files[0]], ['audio', files[1]]] as const) {
        await open(file.name).press('Enter');
        const media = dialog.locator(kind);
        await expect(media).toBeInViewport({ ratio: 1 });
        await expect(close).toBeInViewport({ ratio: 1 });
        await expect(download).toBeInViewport({ ratio: 1 });
        await expect(media).toHaveAttribute('preload', 'none');
        await expect(media).toHaveJSProperty('controls', true);
        await expect(media).toHaveAttribute('src', `/entries/${file.id}/content`);
        expect(fixture.mediaRequests.filter(({ id }) => id === file.id)).toEqual([]);
        const retired = await media.elementHandle();
        await tabTo(media);
        await playAndSeek(media);
        if (kind === 'video') {
          expect(await media.evaluate((video: HTMLVideoElement) => video.getVideoPlaybackQuality().totalVideoFrames)).toBeGreaterThan(0);
        }
        const requests = fixture.mediaRequests.filter(({ id }) => id === file.id);
        expect(requests.length).toBeGreaterThan(0);
        expect(requests.every(({ destination, range }) => destination === kind && /^bytes=\d+-\d*$/.test(range ?? ''))).toBe(true);
        await page.screenshot({ path: testInfo.outputPath(`${kind}-${theme}.png`) });
        await tabTo(close);
        await page.keyboard.press('Enter');
        await expect(open(file.name)).toBeFocused();
        await expect.poll(() => retired!.evaluate((element: HTMLMediaElement) => ({
          paused: element.paused, src: element.getAttribute('src'), loading: element.networkState === element.NETWORK_LOADING,
        }))).toEqual({ paused: true, src: null, loading: false });
        await retired!.dispose();
      }

      await open('broken.mp4').click();
      await dialog.locator('video').evaluate((element: HTMLMediaElement) => { void element.play().catch(() => {}); });
      await expect(dialog.getByRole('status')).toHaveText('This media could not be played. Download the file to open it.');
      await expect(download).toBeInViewport({ ratio: 1 });
      const downloaded = page.waitForEvent('download');
      await download.click();
      expect((await downloaded).suggestedFilename()).toBe('broken.mp4');
      await download.press('Escape');

      // Add a same-document entry so Back exercises the app's history handler.
      await page.evaluate(() => window.history.pushState(window.history.state, '', window.location.href));
      for (const exit of ['close', 'navigation']) {
        await open('pending.mp3').click();
        const media = dialog.locator('audio');
        const retired = await media.elementHandle();
        await media.evaluate((element: HTMLMediaElement) => { void element.play().catch(() => {}); });
        await expect.poll(() => fixture.heldRequests).toBe(exit === 'close' ? 1 : 2);
        expect(fixture.canceledRequests).toBe(exit === 'close' ? 0 : 1);
        if (exit === 'close') await close.click();
        else await page.goBack();
        await expect(dialog).toHaveCount(0);
        await expect.poll(() => fixture.canceledRequests).toBe(exit === 'close' ? 1 : 2);

        await open('pattern.mp4').click();
        await expect(dialog.locator('video')).toBeVisible();
        await retired!.evaluate((element: HTMLMediaElement) => {
          Object.defineProperty(element, 'error', { value: { code: 3 } });
          element.dispatchEvent(new Event('error'));
        });
        await expect(dialog.locator('video')).toBeVisible();
        await expect(dialog.getByText('This media could not be played.', { exact: false })).toHaveCount(0);
        await retired!.dispose();
        await close.click();
      }
    } finally {
      fixture.close();
    }
  });
}

import { ResponseTooLargeError, type ApiClient } from './api';
import type { FileEntry } from './listing';

export const PREVIEW_LIMIT = 2 * 1024 * 1024;
export const PDF_PREVIEW_LIMIT = 20 * 1024 * 1024;
const MAX_PIXELS = 16_000_000;
const TEXT_EXTENSIONS = new Set(['txt', 'md', 'csv', 'json', 'log', 'yaml', 'yml', 'toml', 'rs', 'js', 'ts', 'css']);
const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif']);
export type Preview = { kind: 'text'; text: string } | { kind: 'image'; blob: Blob } | { kind: 'pdf'; bytes: Uint8Array<ArrayBuffer> } | { kind: 'unsupported'; message: string };

function previewLimit(entry: FileEntry): number {
  return entry.name.split('.').at(-1)?.toLowerCase() === 'pdf' ? PDF_PREVIEW_LIMIT : PREVIEW_LIMIT;
}

function tooLarge(limit: number): Extract<Preview, { kind: 'unsupported' }> {
  return { kind: 'unsupported', message: `This file is larger than the ${limit / 1024 / 1024} MiB preview limit. Download the file to open it.` };
}

export function previewUnavailable(entry: FileEntry): string | null {
  const limit = previewLimit(entry);
  if (entry.size > limit) return tooLarge(limit).message;
  const extension = entry.name.split('.').at(-1)?.toLowerCase() ?? '';
  if (extension !== 'pdf' && !TEXT_EXTENSIONS.has(extension) && !IMAGE_EXTENSIONS.has(extension)) return 'Preview is not available for this file type. Download the file to open it.';
  return null;
}

function imageFormat(bytes: Uint8Array): { type: string; width: number; height: number } | null {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (bytes.length >= 24 && [137, 80, 78, 71, 13, 10, 26, 10].every((byte, index) => bytes[index] === byte) &&
    view.getUint32(8) === 13 && view.getUint32(12) === 0x49484452) {
    return { type: 'image/png', width: view.getUint32(16), height: view.getUint32(20) };
  }
  const signature = String.fromCharCode(...bytes.slice(0, 6));
  if (bytes.length >= 10 && (signature === 'GIF87a' || signature === 'GIF89a')) {
    return { type: 'image/gif', width: view.getUint16(6, true), height: view.getUint16(8, true) };
  }
  if (bytes[0] !== 0xff || bytes[1] !== 0xd8) return null;
  let offset = 2;
  while (offset + 3 < bytes.length) {
    if (bytes[offset++] !== 0xff) return null;
    while (bytes[offset] === 0xff) offset++;
    const marker = bytes[offset++];
    if (marker === 0xda || marker === 0xd9) return null;
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue;
    if (offset + 2 > bytes.length) return null;
    const length = view.getUint16(offset);
    if (length < 2 || offset + length > bytes.length) return null;
    if ([0xc0, 0xc1, 0xc2].includes(marker) && length >= 8) {
      return { type: 'image/jpeg', height: view.getUint16(offset + 3), width: view.getUint16(offset + 5) };
    }
    offset += length;
  }
  return null;
}

export async function loadPreview(client: ApiClient, entry: FileEntry, url: string, signal: AbortSignal): Promise<Preview> {
  const unavailable = previewUnavailable(entry);
  if (unavailable) return { kind: 'unsupported', message: unavailable };
  const limit = previewLimit(entry);
  let blob: Blob;
  try { blob = await client.request<Blob>(url, { signal, maxResponseBytes: limit }); }
  catch (error) {
    if (error instanceof ResponseTooLargeError) return tooLarge(limit);
    throw error;
  }
  signal.throwIfAborted();
  if (!(blob instanceof Blob)) throw new Error('Unexpected preview response');
  if (blob.size > limit) return tooLarge(limit);
  const bytes = new Uint8Array(await blob.arrayBuffer());
  signal.throwIfAborted();
  const extension = entry.name.split('.').at(-1)!.toLowerCase();
  if (extension === 'pdf') {
    if (!new TextDecoder().decode(bytes.subarray(0, 1024)).includes('%PDF-')) {
      return { kind: 'unsupported', message: 'This file is not a PDF document. Download the file to open it.' };
    }
    return { kind: 'pdf', bytes };
  }
  if (IMAGE_EXTENSIONS.has(extension)) {
    const format = imageFormat(bytes);
    if (!format || !format.width || !format.height || format.width * format.height > MAX_PIXELS) {
      return { kind: 'unsupported', message: 'This image cannot be previewed safely. Download the file to open it.' };
    }
    // A verified raster MIME prevents SVG or HTML from gaining an active document context.
    return { kind: 'image', blob: new Blob([bytes], { type: format.type }) };
  }
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(text)) throw new Error('Binary content');
    return { kind: 'text', text };
  } catch {
    return { kind: 'unsupported', message: 'This file is not UTF-8 text. Download the file to open it.' };
  }
}

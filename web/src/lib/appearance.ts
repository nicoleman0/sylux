export type Appearance = {
  accentColor: string | null;
  loginBackgroundColor: string | null;
  logoUrl: string | null;
  loginImageUrl: string | null;
};
export type ImageChange = null | { keep: true } | { data: string; mediaType: string };
export type AppearanceUpdate = Pick<Appearance, 'accentColor' | 'loginBackgroundColor'> & {
  logo: ImageChange;
  loginImage: ImageChange;
};
export const defaultAppearance: Appearance = { accentColor: null, loginBackgroundColor: null, logoUrl: null, loginImageUrl: null };
export const imageLimits = { logo: 512 * 1024, loginImage: 2 * 1024 * 1024 };

export function readAppearance(value: unknown): Appearance {
  if (!value || typeof value !== 'object') throw new Error('Invalid appearance response');
  const item = value as Record<string, unknown>;
  for (const key of ['accentColor', 'loginBackgroundColor']) {
    if (item[key] !== null && (typeof item[key] !== 'string' || !/^#[\da-f]{6}$/i.test(item[key]))) throw new Error('Invalid appearance color');
  }
  for (const key of ['logoUrl', 'loginImageUrl']) {
    if (item[key] !== null && (typeof item[key] !== 'string' || !/^\/instance\/appearance\/assets\/[\da-f]{64}$/.test(item[key]))) throw new Error('Invalid appearance image URL');
  }
  return { accentColor: item.accentColor, loginBackgroundColor: item.loginBackgroundColor, logoUrl: item.logoUrl, loginImageUrl: item.loginImageUrl } as Appearance;
}

const rgb = (hex: string) => [1, 3, 5].map(start => parseInt(hex.slice(start, start + 2), 16));
const hex = (values: number[]) => '#' + values.map(value => Math.round(value).toString(16).padStart(2, '0')).join('');
const mix = (a: string, b: string, ratio: number) => hex(rgb(a).map((value, index) => value * (1 - ratio) + rgb(b)[index] * ratio));
const luminance = (color: string) => rgb(color).map(value => value / 255).map(value => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4).reduce((sum, value, index) => sum + value * [.2126, .7152, .0722][index], 0);
export const contrast = (a: string, b: string) => (Math.max(luminance(a), luminance(b)) + .05) / (Math.min(luminance(a), luminance(b)) + .05);

// Adjust the chosen hue so it can also serve as readable text and a focus ring.
export function appearanceStyle(appearance: Appearance, dark: boolean): string {
  if (!appearance.accentColor) return '';
  const surface = dark ? '#1d2323' : '#ffffff';
  const selection = mix(surface, appearance.accentColor, .1);
  let primary = appearance.accentColor;
  for (let step = 0; step <= 100; step++) {
    primary = mix(appearance.accentColor, dark ? '#ffffff' : '#000000', step / 100);
    if ([surface, selection, dark ? '#171c1c' : '#f6f7f7'].every(bg => contrast(primary, bg) >= 4.5)) break;
  }
  const foreground = contrast(primary, '#ffffff') >= contrast(primary, '#000000') ? '#ffffff' : '#000000';
  return Object.entries({
    '--primary': primary, '--primary-foreground': foreground,
    '--primary-hover': mix(primary, foreground === '#ffffff' ? '#000000' : '#ffffff', .12),
    '--secondary': selection, '--secondary-foreground': primary,
    '--accent': selection, '--accent-foreground': primary, '--ring': primary,
    '--sidebar-primary': primary, '--sidebar-primary-foreground': foreground,
    '--sidebar-accent': selection, '--sidebar-accent-foreground': primary, '--sidebar-ring': primary,
  }).map(([key, value]) => `${key}:${value}`).join(';');
}

export async function imageUpload(file: File, kind: keyof typeof imageLimits): Promise<{ data: string; mediaType: string }> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('Choose a PNG, JPEG or WebP image.');
  if (!file.size || file.size > imageLimits[kind]) throw new Error(kind === 'logo' ? 'Logo must be 512 KiB or smaller.' : 'Login image must be 2 MiB or smaller.');
  const bitmap = await createImageBitmap(file).catch(() => { throw new Error('This image could not be read. Choose another image.'); });
  const valid = bitmap.width <= 4096 && bitmap.height <= 4096 && bitmap.width * bitmap.height <= 8_000_000;
  bitmap.close();
  if (!valid) throw new Error('Images must be at most 4096 pixels per side and 8 million pixels in total.');
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = '';
  for (let start = 0; start < bytes.length; start += 8192) binary += String.fromCharCode(...bytes.subarray(start, start + 8192));
  return { data: btoa(binary), mediaType: file.type };
}

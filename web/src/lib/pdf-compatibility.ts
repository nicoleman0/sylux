const errorMessage = 'PDF previews need a newer browser. Update your browser to view this file here, or download it instead.';

export function getPdfPreviewCompatibilityError(): string | null {
  const mapPrototype = Map.prototype as unknown as Record<string, unknown>;
  const weakMapPrototype = WeakMap.prototype as unknown as Record<string, unknown>;

  if (typeof mapPrototype.getOrInsertComputed !== 'function' || typeof weakMapPrototype.getOrInsertComputed !== 'function') {
    return errorMessage;
  }

  return null;
}

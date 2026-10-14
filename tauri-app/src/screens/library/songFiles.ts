/**
 * Pure helpers for moving songs in and out of the library as `.rcsong` files
 * (M26-A), shared with the network panel (M26-D).
 */

/** The slug from an `exists:<slug>` error, or null for any other error. */
export function existsSlug(err: unknown): string | null {
  const text = String(err);
  const at = text.indexOf("exists:");
  if (at === -1) return null;
  const slug = text.slice(at + "exists:".length).trim();
  return slug.length > 0 ? slug : null;
}

/** The first `<slug>-N` (N ≥ 2) not in `taken`, for "Keep both". */
export function freeSlug(slug: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  for (let n = 2; ; n++) {
    const candidate = `${slug}-${n}`;
    if (!used.has(candidate)) return candidate;
  }
}

/** The last path component of a file path (either separator). */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

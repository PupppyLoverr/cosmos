/** cosmos://share?uri=… or cosmos://share?text=… — Android SEND lands here. */

export function parseShareUrl(url: string | null): { uri?: string; text?: string } | null {
  if (!url) return null;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "cosmos:") return null;
  const hostPath = `${parsed.hostname}${parsed.pathname}`.replace(/\/+/g, "/");
  if (!hostPath.includes("share")) return null;
  const uri = parsed.searchParams.get("uri") ?? undefined;
  const text = parsed.searchParams.get("text") ?? undefined;
  if (!uri && !text) return null;
  return { uri, text };
}

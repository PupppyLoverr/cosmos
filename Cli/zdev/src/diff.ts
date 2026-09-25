// Line diff + read-cache — backs the Ink port of brainless claude-diff.
// `edit` anchors are `<n>:<sha6>` positions into the pre-edit file; `read`
// results carry `n:sha|line` rows, so the transcript itself is the source
// of truth for the removed side of a diff.

export type DiffLine = {
  type: "add" | "del" | "ctx";
  /** New-file line number (deleted rows carry the old number). */
  n?: number;
  text: string;
};

/** Parse `read` tool output: `file_sha256=… lines=N` header then `n:sha|text`. */
export function parseReadDetail(detail: string): Map<number, string> {
  const out = new Map<number, string>();
  for (const l of detail.split("\n")) {
    const m = l.match(/^(\d+):[0-9a-f]{6}\|(.*)$/);
    if (m) out.set(parseInt(m[1], 10), m[2]);
  }
  return out;
}

/** LCS diff of two line lists; small inputs (edit hunks), O(n·m) is fine. */
export function diffLines(
  oldLines: string[],
  newLines: string[],
  oldStart = 1,
): DiffLine[] {
  const n = oldLines.length;
  const m = newLines.length;
  // dp[i][j] = LCS length of old[i:] vs new[j:]
  const dp: number[][] = Array.from({ length: n + 1 }, () =>
    new Array<number>(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      dp[i][j] =
        oldLines[i] === newLines[j]
          ? dp[i + 1][j + 1] + 1
          : Math.max(dp[i + 1][j], dp[i][j + 1]);

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      out.push({ type: "ctx", n: oldStart + i, text: oldLines[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push({ type: "del", n: oldStart + i, text: oldLines[i] });
      i++;
    } else {
      out.push({ type: "add", n: oldStart + j, text: newLines[j] });
      j++;
    }
  }
  for (; i < n; i++) out.push({ type: "del", n: oldStart + i, text: oldLines[i] });
  for (; j < m; j++) {
    out.push({ type: "add", n: oldStart + j, text: newLines[j] });
  }
  return out;
}

const anchor = (s: unknown): number | undefined => {
  const n = parseInt(String(s ?? "").split(":")[0], 10);
  return Number.isFinite(n) ? n : undefined;
};

/**
 * Build claude-diff rows for an `edit` input. `reads` maps file path →
 * {line number → text} recovered from prior `read` results in the transcript.
 * Falls back to a pure "+new_text" hunk when the old side is unavailable.
 */
export function buildEditDiff(
  input: Record<string, unknown> | undefined,
  reads: Map<string, Map<number, string>>,
): { header: string; lines: DiffLine[] }[] {
  const path = String(input?.path ?? "");
  const file = reads.get(path) ?? new Map<number, string>();
  const edits = Array.isArray(input?.edits) ? (input!.edits as any[]) : [];
  const hunks: { header: string; lines: DiffLine[] }[] = [];

  for (const e of edits) {
    const s = anchor(e?.start) ?? 1;
    const en = anchor(e?.end) ?? s;
    const oldL: string[] = [];
    let haveOld = true;
    for (let i = s; i <= en; i++) {
      const t = file.get(i);
      if (t === undefined) {
        haveOld = false;
        break;
      }
      oldL.push(t);
    }
    const newL = String(e?.new_text ?? "").replace(/\r\n/g, "\n").split("\n");
    while (newL.length && newL[newL.length - 1] === "") newL.pop();

    let lines: DiffLine[];
    if (haveOld) {
      // LCS rows; add 1 context row each side when the file extends past it
      const body = diffLines(oldL, newL, s);
      const before = file.get(s - 1);
      const after = file.get(en + 1);
      lines = [
        ...(before !== undefined
          ? [{ type: "ctx" as const, n: s - 1, text: before }]
          : []),
        ...body,
        ...(after !== undefined
          ? [{ type: "ctx" as const, n: en + 1, text: after }]
          : []),
      ];
    } else {
      lines = newL.map((t, i) => ({ type: "add" as const, n: s + i, text: t }));
    }
    const range = s === en ? `${s}` : `${s}–${en}`;
    hunks.push({ header: `@@ ${range} @@`, lines });
  }
  return hunks;
}

/** Numbered +rows for a `write` call (claude-diff add treatment). */
export function buildWriteDiff(content: string): DiffLine[] {
  const lines = content.replace(/\r\n/g, "\n").split("\n");
  while (lines.length && lines[lines.length - 1] === "") lines.pop();
  return lines.map((t, i) => ({ type: "add", n: i + 1, text: t }));
}

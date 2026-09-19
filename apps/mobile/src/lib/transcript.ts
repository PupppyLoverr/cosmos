// Applies WatchDocMessages TranscriptFrames onto an entries array.
// Mirror of crates/doc/src/transcript_delta.rs semantics: reset replaces;
// delta upserts anchored on `after`, appends text tails, removes ids.

import { SessionMessageEntry, TranscriptUpdate, MessagePart } from './protocol';

export function applyTranscriptUpdate(
  entries: SessionMessageEntry[],
  update: TranscriptUpdate,
): SessionMessageEntry[] {
  if (update.reset) return update.reset;
  const next = entries.slice();
  const indexOf = (id: string) => next.findIndex((e) => e.id === id);
  for (const id of update.remove ?? []) {
    const i = indexOf(id);
    if (i >= 0) next.splice(i, 1);
  }
  for (const u of update.upsert ?? []) {
    const existing = indexOf(u.entry.id);
    if (existing >= 0) {
      next[existing] = u.entry;
    } else {
      const at = u.after == null ? 0 : indexOf(u.after) + 1;
      next.splice(at <= 0 ? 0 : at, 0, u.entry);
    }
  }
  for (const a of update.append ?? []) {
    const i = indexOf(a.entry);
    if (i < 0) continue;
    const entry = next[i];
    const parts = entry.parts.map((p) =>
      p.id === a.part && (p.kind === 'text' || p.kind === 'reasoning')
        ? ({ ...p, text: p.text + a.text } as MessagePart)
        : p,
    );
    next[i] = { ...entry, parts };
  }
  if (typeof update.count === 'number' && next.length !== update.count) {
    // Desync tripwire — caller should resubscribe for a fresh reset.
    return next;
  }
  return next;
}

/** Pull the display text of an entry (all text parts joined). */
export function entryText(entry: SessionMessageEntry): string {
  return entry.parts
    .filter((p): p is Extract<MessagePart, { kind: 'text' }> => p.kind === 'text')
    .map((p) => p.text)
    .join('');
}

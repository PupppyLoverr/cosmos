// Working line — port of claude-thinking: cycling ✶ sparkle, a rotating
// verb (or a fixed label), elapsed time, tokens so far, "esc to interrupt".
import React, { useEffect, useState } from "react";
import { Text } from "ink";

import { DIM, GLYPHS, ROSE, VERBS } from "../theme";

export function Thinking({ running, tokens, verb }: { running: boolean; tokens?: number; verb?: string }) {
  const [tick, setTick] = useState(0);
  const [start] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => setTick((n) => n + 1), 120);
    return () => clearInterval(id);
  }, [running]);
  if (!running) return null;
  const secs = Math.floor((Date.now() - start) / 1000);
  const label = verb ?? `${VERBS[Math.floor((tick * 120) / 5200) % VERBS.length]}…`;
  const time = secs >= 60 ? `${Math.floor(secs / 60)}m ${secs % 60}s` : `${secs}s`;
  return (
    <Text wrap="truncate-end">
      <Text color={ROSE}>{GLYPHS[tick % GLYPHS.length]} </Text>
      <Text bold color={ROSE}>
        {label}{" "}
      </Text>
      <Text color={DIM}>
        ({time}
        {tokens ? ` · ↓ ${tokens.toLocaleString()} tokens` : ""}
        {verb ? "" : " · esc to interrupt"})
      </Text>
    </Text>
  );
}

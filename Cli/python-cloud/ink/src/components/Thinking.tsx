// Working line — port of claude-thinking: cycling ✶ sparkle, rotating
// verb, elapsed seconds + "esc to interrupt".
import React, { useEffect, useState } from "react";
import { Text } from "ink";
import { DIM, GLYPHS, ROSE, VERBS } from "../theme";

export function Thinking({
  running,
  tokens,
}: {
  running: boolean;
  tokens?: number;
}) {
  const [glyph, setGlyph] = useState(0);
  const [verbIdx, setVerbIdx] = useState(0);
  const [secs, setSecs] = useState(0);

  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => setGlyph((g) => (g + 1) % GLYPHS.length), 110);
    return () => clearInterval(id);
  }, [running]);

  useEffect(() => {
    if (!running) return;
    setSecs(0);
    const id = setInterval(() => setSecs((s) => s + 1), 1000);
    return () => clearInterval(id);
  }, [running]);

  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => setVerbIdx((v) => (v + 1) % VERBS.length), 5200);
    return () => clearInterval(id);
  }, [running]);

  if (!running) return null;
  return (
    <Text>
      <Text color={ROSE}>{GLYPHS[glyph]} </Text>
      <Text bold color={ROSE}>
        {VERBS[verbIdx]}…{" "}
      </Text>
      <Text color={DIM}>
        ({secs}s{tokens ? ` · ↑ ${tokens} tokens` : ""} · esc to interrupt)
      </Text>
    </Text>
  );
}

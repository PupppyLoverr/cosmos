// Composer — port of claude-prompt: rules around a `❯` input, the mode
// legend and model/context meter underneath. Multiline, hand-rolled input
// (the cursor is drawn inverse; the terminal cursor stays hidden).
import React from "react";
import { Box, Text } from "ink";

import { DIM, GRAY, MODE_COLOR, ROSE, RULE } from "../theme";

const PLACEHOLDER = "Ask zdev to build, fix or explain — @ adds files, ! runs a command";

const MODE_META: Record<string, { glyph: string; label: string; hint: string }> = {
  agent: { glyph: "⏵⏵", label: "agent", hint: "asks before risky tools" },
  plan: { glyph: "⏸", label: "plan mode", hint: "read-only, ends in a plan" },
  yolo: { glyph: "⏵⏵", label: "yolo", hint: "runs everything without asking" },
};

/** Render one buffer line with an inverse cursor at `col`. */
function CursorText({ line, col }: { line: string; col: number }) {
  return (
    <Text wrap="truncate-start">
      {line.slice(0, col)}
      <Text inverse>{line[col] ?? " "}</Text>
      {line.slice(col + 1)}
    </Text>
  );
}

export function Composer({
  value,
  cursor,
  mode,
  model,
  width,
  disabled,
  listening,
  ctxPct,
  hint,
}: {
  value: string;
  cursor: number;
  mode: string;
  model: string;
  width: number;
  disabled?: boolean;
  listening?: boolean;
  /** context window use 0..1, when known */
  ctxPct?: number;
  /** right-side status override (e.g. "press ctrl+c again to quit") */
  hint?: string;
}) {
  const m = MODE_META[mode] ?? MODE_META.agent;
  const lines = value.split("\n");
  let row = 0;
  let acc = 0;
  for (let i = 0; i < lines.length; i++) {
    row = i;
    if (cursor <= acc + lines[i].length) break;
    acc += lines[i].length + 1;
  }
  const col = Math.max(0, Math.min(cursor - acc, lines[row]?.length ?? 0));
  const rule = "─".repeat(Math.max(4, width));
  const ctx = ctxPct !== undefined && ctxPct > 0 ? ` · ${Math.round(ctxPct * 100)}% context` : "";
  return (
    <Box flexDirection="column" marginTop={1}>
      <Text color={RULE}>{rule}</Text>
      {value === "" ? (
        <Text>
          <Text color={disabled ? DIM : ROSE}>❯ </Text>
          {disabled ? (
            <Text color={DIM}>starting…</Text>
          ) : listening ? (
            <Text color={ROSE}>◉ listening — speak, then pause</Text>
          ) : (
            <>
              <Text inverse> </Text>
              <Text color={DIM}>{PLACEHOLDER}</Text>
            </>
          )}
        </Text>
      ) : (
        lines.map((l, i) => (
          <Box key={i}>
            <Text color={i === 0 ? ROSE : DIM}>{i === 0 ? "❯ " : "  "}</Text>
            {i === row ? <CursorText line={l} col={col} /> : <Text wrap="truncate-end">{l}</Text>}
          </Box>
        ))
      )}
      <Text color={RULE}>{rule}</Text>
      <Box justifyContent="space-between" width={width}>
        <Text wrap="truncate-end">
          <Text color={MODE_COLOR[mode] ?? "#ffffff"} bold={mode !== "agent"}>
            {m.glyph} {m.label}
          </Text>
          <Text color={DIM}> · {m.hint} (shift+tab)</Text>
        </Text>
        <Text color={hint ? GRAY : DIM} wrap="truncate-start">
          {hint ?? `${model}${ctx}`}
        </Text>
      </Box>
    </Box>
  );
}

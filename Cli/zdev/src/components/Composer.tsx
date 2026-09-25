// Composer — port of claude-prompt: dual rules around a `❯` input row,
// model chip above-right, mode legend below. Multiline hand-rolled input.
import React from "react";
import { Box, Text, useStdout } from "ink";
import { DIM, MODE_COLOR, ROSE, RULE } from "../theme";

const PLACEHOLDER = "Ask zdev to build features, fix bugs, or work on your code";

const MODE_META: Record<string, { glyph: string; label: string }> = {
  agent: { glyph: "⏵⏵", label: "agent" },
  plan: { glyph: "⏸", label: "plan" },
  yolo: { glyph: "⏵⏵", label: "yolo" },
};

function rule(width: number) {
  return "─".repeat(Math.max(4, width));
}

/** Render buffer lines with an inverse cursor at absolute index `cursor`. */
function CursorText({ line, col }: { line: string; col: number }) {
  const before = line.slice(0, col);
  const at = line[col] ?? " ";
  const after = line.slice(col + 1);
  return (
    <Text wrap="truncate-end">
      {before}
      <Text inverse>{at}</Text>
      {after}
    </Text>
  );
}

export function Composer({
  value,
  cursor,
  mode,
  model,
  disabled,
  listening,
}: {
  value: string;
  cursor: number; // absolute index into value
  mode: string;
  model: string;
  disabled?: boolean;
  listening?: boolean;
}) {
  const { stdout } = useStdout();
  const width = (stdout?.columns ?? 80) - 1;
  const m = MODE_META[mode] ?? MODE_META.agent;
  const modeColor = MODE_COLOR[mode] ?? "#ffffff";

  // map absolute cursor → (row, col) over the split lines
  const lines = value.split("\n");
  let curRow = 0;
  let acc = 0;
  for (let i = 0; i < lines.length; i++) {
    if (cursor <= acc + lines[i].length) {
      curRow = i;
      break;
    }
    acc += lines[i].length + 1;
    curRow = i;
  }
  const curCol = Math.min(cursor - acc, lines[curRow]?.length ?? 0);

  return (
    <Box flexDirection="column" marginTop={0}>
      <Text color={RULE}>{rule(width)}</Text>
      {value === "" && !disabled ? (
        <Box flexDirection="row">
          <Text color={ROSE}>❯ </Text>
          <Text>
            <Text inverse> </Text>
            <Text color={DIM}>{PLACEHOLDER}</Text>
          </Text>
        </Box>
      ) : value === "" && disabled ? (
        <Box flexDirection="row">
          <Text color={DIM}>❯ </Text>
          <Text color={DIM}> connecting to zdev…</Text>
        </Box>
      ) : (
        lines.map((l, i) => (
          <Box key={i} flexDirection="row">
            <Text color={i === 0 ? ROSE : DIM}>{i === 0 ? "❯ " : "  "}</Text>
            {i === curRow ? <CursorText line={l} col={curCol} /> : <Text wrap="truncate-end">{l}</Text>}
          </Box>
        ))
      )}
      <Text color={RULE}>{rule(width)}</Text>
      <Box justifyContent="space-between">
        <Text wrap="truncate-end">
          <Text color={modeColor}>{m.glyph} {m.label}</Text>
          <Text color={DIM}>
            {" "}· / cmds · @file · ctrl+m dictate
          </Text>
        </Text>
        <Text color={DIM} wrap="truncate-end">
          {listening ? "◉ listening… · " : ""}◉ {model} · esc · ctrl+c
        </Text>
      </Box>
    </Box>
  );
}

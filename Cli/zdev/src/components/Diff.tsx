// Diff — Ink port of brainless claude-diff: a bordered code block of
// numbered rows, `+`/`-`/space mark column, tinted add/del backgrounds.
// `hunks` groups rows under `@@ range @@` headers (one per edit anchor pair).
import React from "react";
import { Box, Text } from "ink";
import type { DiffLine } from "../diff";
import { DIM, GRAY } from "../theme";

const NUM = "#4a4a4a"; // line numbers (spec)
const MARK_ADD = "#e0e0e0";
const MARK_DEL = "#6f6f6f";
const MARK_CTX = "#5a5a5a";
const BG_ADD = "#1e1e1e"; // added rows: subtle lift
const BG_DEL = "#000000"; // removed rows: sunk to black
const TXT = "#e8e8e8";
const CTX = "#9a9a9a";

function Row({ l }: { l: DiffLine }) {
  const add = l.type === "add";
  const del = l.type === "del";
  const mark = add ? "+" : del ? "-" : " ";
  return (
    <Text wrap="truncate-end">
      <Text color={NUM}>{String(l.n ?? "").padStart(4)} </Text>
      <Text color={add ? MARK_ADD : del ? MARK_DEL : MARK_CTX}>{mark} </Text>
      <Text
        backgroundColor={add ? BG_ADD : del ? BG_DEL : undefined}
        color={l.type === "ctx" ? CTX : TXT}
      >
        {l.text === "" ? " " : l.text}
      </Text>
    </Text>
  );
}

export function Diff({
  hunks,
  max = 18,
}: {
  hunks: { header: string; lines: DiffLine[] }[];
  max?: number;
}) {
  const total = hunks.reduce((a, h) => a + h.lines.length + 1, 0);
  let left = max;
  return (
    <Box
      flexDirection="column"
      borderStyle="single"
      borderColor="#242424"
      paddingX={1}
      marginTop={0}
    >
      {hunks.map((h, hi) => {
        if (left <= 0) return null;
        const shown = h.lines.slice(0, Math.max(0, left - 1));
        left -= shown.length + 1;
        return (
          <Box key={hi} flexDirection="column">
            <Text color={MARK_CTX}>{h.header}</Text>
            {shown.map((l, i) => (
              <Row key={i} l={l} />
            ))}
            {shown.length < h.lines.length ? (
              <Text color={DIM}>    … {h.lines.length - shown.length} more</Text>
            ) : null}
          </Box>
        );
      })}
      {total > max ? (
        <Text color={GRAY}>    … {total - max} more diff rows</Text>
      ) : null}
    </Box>
  );
}

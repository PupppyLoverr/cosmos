// Transcript rows — port of claude-message (❯ user chip), claude-tool-call
// (⏺ tool line + ⎿ detail disclosure) and claude-diff (+/- tinted rows).
import React from "react";
import { Box, Text } from "ink";
import { ACTIVE, DIM, GRAY, GREEN, RED, ROSE, YELLOW } from "../theme";
import { Markdown } from "../markdown";
import type { Item } from "../types";

const STATUS_COLOR = { pending: YELLOW, completed: GREEN, failed: RED } as const;

function TintedLine({ l }: { l: string }) {
  const add = /^\+(?![-+])/.test(l);
  const del = /^-(?![-+])/.test(l);
  const hunk = /^@@|^diff |^index |^---|^\+\+\+/.test(l);
  return (
    <Text
      wrap="truncate-end"
      backgroundColor={add ? "#1a2b1e" : del ? "#2b1a1e" : undefined}
      color={add ? GREEN : del ? RED : hunk ? DIM : GRAY}
    >
      {l}
    </Text>
  );
}

/** Diff-aware detail body: +/- prefixed lines get the tinted treatment.
 * For edit/write calls the result is just "ok <path>" — synthesize the
 * +rows from the tool input so the expanded view shows what changed. */
function Detail({
  text,
  name,
  input,
}: {
  text: string;
  name: string;
  input?: Record<string, unknown>;
}) {
  const cap = 24;
  let synth: string[] = [];
  if (name === "edit" && Array.isArray(input?.edits)) {
    for (const e of input!.edits as any[]) {
      synth.push(`@@ ${e?.start ?? "?"} → ${e?.end ?? "?"} @@`);
      for (const l of String(e?.new_text ?? "").split("\n"))
        if (l) synth.push(`+ ${l}`);
    }
  } else if (name === "write" && typeof input?.content === "string") {
    synth = (input.content as string)
      .split("\n")
      .slice(0, cap)
      .map((l) => `+ ${l}`);
  }
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const shown = lines.slice(0, cap);
  return (
    <Box flexDirection="column" paddingLeft={4}>
      {synth.slice(0, cap).map((l, i) => (
        <TintedLine key={`s${i}`} l={l} />
      ))}
      {shown.map((l, i) => (
        <TintedLine key={i} l={l} />
      ))}
      {lines.length > cap && (
        <Text color={DIM}>… {lines.length - cap} more lines</Text>
      )}
    </Box>
  );
}

function ToolRow({
  item,
  expanded,
  focused,
}: {
  item: Extract<Item, { kind: "tool" }>;
  expanded: boolean;
  focused: boolean;
}) {
  const firstLine = (item.detail ?? "").split("\n")[0] ?? "";
  return (
    <Box flexDirection="column">
      <Text wrap="truncate-end">
        <Text color={STATUS_COLOR[item.status]}>⏺ </Text>
        <Text bold={focused} color={focused ? ACTIVE : undefined}>
          {item.name}
        </Text>
        {item.arg ? <Text color={GRAY}>({item.arg})</Text> : null}
        {focused ? <Text color={DIM}>  ← enter/o to collapse</Text> : null}
      </Text>
      {(firstLine || expanded) && (
        <Text wrap="truncate-end">
          <Text>{"  "}</Text>
          <Text color={DIM}>⎿ </Text>
          <Text color={item.status === "failed" ? RED : DIM}>
            {expanded ? "" : firstLine}
          </Text>
          {!expanded && (item.detail ?? "").includes("\n") ? (
            <Text color={DIM}> (ctrl+o to expand)</Text>
          ) : null}
        </Text>
      )}
      {expanded && (item.detail || item.input) ? (
        <Detail text={item.detail ?? ""} name={item.name} input={item.input} />
      ) : null}
    </Box>
  );
}

export function TranscriptItem({
  item,
  expanded,
  focused,
}: {
  item: Item;
  expanded: boolean;
  focused: boolean;
}) {
  switch (item.kind) {
    case "user":
      // ❯ chip: dark full-width row — backgroundColor lives on Text in Ink.
      return (
        <Box marginTop={1}>
          <Text backgroundColor="#1c1c24" wrap="truncate-end">
            <Text bold color={ROSE}>
              {" ❯ "}
            </Text>
            <Text>{item.text}</Text>
            <Text>{" ".repeat(200)}</Text>
          </Text>
        </Box>
      );
    case "assistant":
      return (
        <Box flexDirection="column" marginTop={1} paddingLeft={1}>
          <Markdown text={item.text} />
        </Box>
      );
    case "tool":
      return (
        <Box marginTop={0}>
          <ToolRow item={item} expanded={expanded} focused={focused} />
        </Box>
      );
    case "plan":
      return (
        <Box
          flexDirection="column"
          marginTop={1}
          borderStyle="single"
          borderColor={ROSE}
          paddingX={1}
        >
          <Text bold color={ROSE}>
            ⏸ plan — /approve to run, or keep refining
          </Text>
          <Markdown text={item.text} />
        </Box>
      );
    case "note":
      return (
        <Text color={DIM} italic>
          {"  "}
          {item.text}
        </Text>
      );
  }
}

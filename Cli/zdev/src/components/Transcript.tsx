// Transcript rows — ports of: claude-message (❯ user chip), claude-tool-call
// (⏺ line + ⎿ disclosure + cyan arg), claude-diff (numbered tinted rows),
// grok-tool card (┃ ◆ sub-agent block), grok-turn-end (◆ footer) and
// grok-working (braille pending spinner).
import React, { useEffect, useState } from "react";
import { Box, Text } from "ink";
import {
  ACTIVE,
  BRAILLE,
  CYAN,
  DIM,
  GRAY,
  GREEN,
  RED,
  ROSE,
  SILVER,
  YELLOW,
} from "../theme";
import { Markdown } from "../markdown";
import { buildEditDiff, buildWriteDiff, type DiffLine } from "../diff";
import { toolLabel } from "../reducer";
import { Diff } from "./Diff";
import type { Item } from "../types";

const STATUS_COLOR = { pending: YELLOW, completed: GREEN, failed: RED } as const;

export type ReadCache = Map<string, Map<number, string>>;

function TintedLine({ l }: { l: string }) {
  const add = /^\+(?![-+])/.test(l);
  const del = /^-(?![-+])/.test(l);
  const hunk = /^@@|^diff |^index |^---|^\+\+\+/.test(l);
  return (
    <Text
      wrap="truncate-end"
      backgroundColor={add ? "#1e1e1e" : del ? "#0a0a0a" : undefined}
      color={add ? GREEN : del ? RED : hunk ? DIM : GRAY}
    >
      {l}
    </Text>
  );
}

/** Braille pulse on a pending ⏺ (grok-working grammar). */
function PendingMark() {
  const [f, setF] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setF((n) => (n + 1) % BRAILLE.length), 90);
    return () => clearInterval(id);
  }, []);
  return <Text color={YELLOW}>{BRAILLE[f]} </Text>;
}

/** Detail body under ⎿ — real unified diff for edit/write when the old
 * side is recoverable from earlier `read` output; tinted raw lines otherwise. */
function Detail({
  item,
  reads,
}: {
  item: Extract<Item, { kind: "tool" }>;
  reads: ReadCache;
}) {
  const cap = 24;
  const text = item.detail ?? "";
  if (item.name === "edit") {
    const hunks = buildEditDiff(item.input, reads);
    if (hunks.length) {
      return (
        <Box flexDirection="column" paddingLeft={4}>
          <Diff hunks={hunks} />
          {text ? <Text color={DIM}>{text.split("\n")[0]}</Text> : null}
        </Box>
      );
    }
  }
  if (item.name === "write" && typeof item.input?.content === "string") {
    const lines = buildWriteDiff(item.input.content);
    return (
      <Box flexDirection="column" paddingLeft={4}>
        <Diff hunks={[{ header: `@@ ${item.arg ?? "file"} @@`, lines }]} />
        {text ? <Text color={DIM}>{text.split("\n")[0]}</Text> : null}
      </Box>
    );
  }
  // read results carry `n:sha|line` anchors — render as a clean numbered
  // listing (hashes are internal plumbing, not content).
  if (item.name === "read") {
    const rows = text
      .replace(/\r\n/g, "\n")
      .split("\n")
      .map((l) => l.match(/^(\d+):[0-9a-f]{6}\|(.*)$/))
      .filter((m): m is RegExpMatchArray => !!m)
      .slice(0, cap);
    return (
      <Box flexDirection="column" paddingLeft={4}>
        {rows.map((m, i) => (
          <Text key={i} wrap="truncate-end">
            <Text color="#4a4a4a">{m[1].padStart(4)} </Text>
            <Text color={DIM}>│ </Text>
            <Text>{m[2] === "" ? " " : m[2]}</Text>
          </Text>
        ))}
      </Box>
    );
  }
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const shown = lines.slice(0, cap);
  return (
    <Box flexDirection="column" paddingLeft={4}>
      {shown.map((l, i) => (
        <TintedLine key={i} l={l} />
      ))}
      {lines.length > cap && (
        <Text color={DIM}>… {lines.length - cap} more lines</Text>
      )}
    </Box>
  );
}

/** grok-tool card variant — left gutter + ◆ heading for `task` sub-agents. */
function TaskCard({
  item,
  expanded,
}: {
  item: Extract<Item, { kind: "tool" }>;
  expanded: boolean;
}) {
  const border = STATUS_COLOR[item.status];
  const firstLine = (item.detail ?? "").split("\n")[0] ?? "";
  return (
    <Box
      flexDirection="column"
      borderStyle="bold"
      borderLeft
      borderRight={false}
      borderTop={false}
      borderBottom={false}
      borderColor={border}
      paddingLeft={1}
    >
      <Text wrap="truncate-end">
        <Text color={border}>◆ </Text>
        <Text bold color={SILVER}>
          Task
        </Text>
        {item.arg ? <Text color={GRAY}> {item.arg}</Text> : null}
        {item.status === "pending" ? <Text color={DIM}>  working…</Text> : null}
      </Text>
      {(firstLine || expanded) && (
        <Text wrap="truncate-end" color={item.status === "failed" ? RED : DIM}>
          {expanded ? "" : firstLine}
          {!expanded && (item.detail ?? "").includes("\n") ? (
            <Text color={DIM}> (ctrl+o to expand)</Text>
          ) : null}
        </Text>
      )}
      {expanded && item.detail ? (
        <Box flexDirection="column">
          {item.detail
            .split("\n")
            .slice(0, 24)
            .map((l, i) => (
              <Text key={i} color={DIM} wrap="truncate-end">
                {l}
              </Text>
            ))}
        </Box>
      ) : null}
    </Box>
  );
}

function ToolRow({
  item,
  expanded,
  focused,
  reads,
}: {
  item: Extract<Item, { kind: "tool" }>;
  expanded: boolean;
  focused: boolean;
  reads: ReadCache;
}) {
  if (item.name === "task") return <TaskCard item={item} expanded={expanded} />;
  const label = toolLabel(item.name);
  const expandable =
    !!item.detail?.includes("\n") ||
    item.name === "edit" ||
    (item.name === "write" && typeof item.input?.content === "string");

  // ⎿ line — claude-diff summary grammar for edits/writes, raw result
  // first line otherwise ("Updated f.ts with 2 additions and 1 removal").
  let firstLine = (item.detail ?? "").split("\n")[0] ?? "";
  if (item.status === "completed") {
    if (item.name === "edit") {
      const hunks = buildEditDiff(item.input, reads);
      if (hunks.length) {
        const a = hunks
          .flatMap((h) => h.lines)
          .filter((l) => l.type === "add").length;
        const d = hunks
          .flatMap((h) => h.lines)
          .filter((l) => l.type === "del").length;
        firstLine = `Updated ${item.arg ?? "file"} with ${a} addition${a === 1 ? "" : "s"} and ${d} removal${d === 1 ? "" : "s"}`;
      }
    } else if (item.name === "write" && typeof item.input?.content === "string") {
      const n = buildWriteDiff(item.input.content).length;
      firstLine = `Wrote ${n} line${n === 1 ? "" : "s"} to ${item.arg ?? "file"}`;
    }
  }
  return (
    <Box flexDirection="column">
      <Text wrap="truncate-end">
        {item.status === "pending" ? (
          <PendingMark />
        ) : (
          <Text color={STATUS_COLOR[item.status]}>⏺ </Text>
        )}
        <Text bold={focused} color={focused ? ACTIVE : "#e8e8e8"}>
          {label}
        </Text>
        {item.arg ? (
          <Text>
            <Text color={DIM}>(</Text>
            <Text color={CYAN}>{item.arg}</Text>
            <Text color={DIM}>)</Text>
          </Text>
        ) : null}
        {focused ? <Text color={DIM}>  ← enter/o to collapse</Text> : null}
      </Text>
      {(firstLine || expanded) && (
        <Text wrap="truncate-end">
          <Text color={DIM}>{"  "}⎿ </Text>
          <Text color={item.status === "failed" ? RED : DIM}>
            {expanded ? "" : firstLine}
          </Text>
          {!expanded && expandable ? (
            <Text color={DIM}> (ctrl+o to expand)</Text>
          ) : null}
        </Text>
      )}
      {expanded ? <Detail item={item} reads={reads} /> : null}
    </Box>
  );
}

export function TranscriptItem({
  item,
  expanded,
  focused,
  streaming,
  reads,
}: {
  item: Item;
  expanded: boolean;
  focused: boolean;
  streaming?: boolean;
  reads: ReadCache;
}) {
  switch (item.kind) {
    case "user":
      // compact ❯ chip — bg hugs the text, no full-width bar
      return (
        <Box marginTop={1}>
          <Text wrap="truncate-end">
            <Text backgroundColor="#242424" color={ROSE} bold>
              {" ❯ "}
            </Text>
            <Text backgroundColor="#242424">{item.text}</Text>
          </Text>
        </Box>
      );
    case "assistant":
      return (
        <Box flexDirection="column" marginTop={1} paddingLeft={1}>
          <Markdown text={item.text} />
          {streaming ? <Text color={ROSE}>▌</Text> : null}
        </Box>
      );
    case "tool":
      return (
        <Box marginTop={0}>
          <ToolRow
            item={item}
            expanded={expanded}
            focused={focused}
            reads={reads}
          />
        </Box>
      );
    case "plan":
      // grok-plan: framed plan.md viewer — title bar, numbered lines, key row.
      return (
        <Box flexDirection="column" marginTop={1}>
          <Box
            flexDirection="column"
            borderStyle="round"
            borderColor={YELLOW}
            paddingX={1}
          >
            <Text color={DIM}>─ plan</Text>
            {item.text
              .split("\n")
              .slice(0, 16)
              .map((l, i) => (
                <Text key={i} wrap="truncate-end">
                  <Text color={DIM}>{String(i + 1).padStart(3)} </Text>
                  <Text>{l}</Text>
                </Text>
              ))}
            {item.text.split("\n").length > 16 ? (
              <Text color={DIM}>
                {"    "}… {item.text.split("\n").length - 16} more lines
              </Text>
            ) : null}
            <Text color={DIM}>
              /approve to run {"  "}│{"  "}esc keep planning
            </Text>
          </Box>
        </Box>
      );
    case "note":
      return (
        <Text color={DIM} italic>
          {"  "}
          {item.text}
        </Text>
      );
    case "turn":
      // grok-turn-end: `◆ Turn completed in 8.0s.` as a status line.
      return (
        <Text>
          <Text color={SILVER}>◆ </Text>
          <Text color={DIM}>
            Turn completed in {item.secs.toFixed(1)}s
            {item.out ? ` · ↓ ${item.out} tokens` : ""}.
          </Text>
        </Text>
      );
  }
}

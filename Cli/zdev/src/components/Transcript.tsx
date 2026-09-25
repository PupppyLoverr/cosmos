// Transcript rows — ports of claude-message (❯ user chip), claude-tool-call
// (⏺ line + ⎿ result), claude-diff (numbered tinted rows), grok-tool card
// (┃ ◆ sub-agent block), grok-turn-end (◆ footer) and grok-working (braille
// pending spinner). Rows are rendered once and committed to scrollback, so
// each one shows what matters up front: edit/write diffs inline, a few lines
// of command output, one-line summaries for reads and searches. ctrl+o
// (verbose) shows full results for later rows.
import React, { useEffect, useState } from "react";
import { Box, Text } from "ink";

import { buildEditDiff, buildWriteDiff } from "../diff";
import { logoGrid, logoShade } from "../logo";
import { Markdown } from "../markdown";
import { tailView, toolLabel } from "../reducer";
import { BRAILLE, CYAN, DIM, GRAY, GREEN, RED, ROSE, SILVER, YELLOW } from "../theme";
import type { Item } from "../types";
import { Diff } from "./Diff";

const STATUS_COLOR = { pending: YELLOW, completed: GREEN, failed: RED } as const;

export type ReadCache = Map<string, Map<number, string>>;
type ToolItem = Extract<Item, { kind: "tool" }>;

/** Braille pulse on a pending ⏺ (grok-working grammar). */
function PendingMark() {
  const [f, setF] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setF((n) => (n + 1) % BRAILLE.length), 90);
    return () => clearInterval(id);
  }, []);
  return <Text color={YELLOW}>{BRAILLE[f]} </Text>;
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

/** One-line result summary for the ⎿ row. */
export function summarize(item: ToolItem, reads: ReadCache): string {
  const text = (item.detail ?? "").replace(/\r\n/g, "\n");
  const lines = text.split("\n").filter((l) => l.trim());
  if (item.status === "failed") return (lines[0] ?? "failed").replace(/^ERROR:\s*/, "");
  switch (item.name) {
    case "read": {
      const rows = lines.filter((l) => /^\d+:[0-9a-f]{6}\|/.test(l));
      const total = text.match(/lines=(\d+)/)?.[1];
      if (!rows.length) return total ? `Read ${plural(+total, "line")}` : "Read";
      const a = rows[0].split(":")[0];
      const b = rows[rows.length - 1].split(":")[0];
      return total && rows.length < +total ? `Read lines ${a}–${b} of ${total}` : `Read ${plural(rows.length, "line")}`;
    }
    case "grep": {
      const hits = lines.filter((l) => !l.startsWith("[cap"));
      return hits.length ? `Found ${plural(hits.length, "match")}${text.includes("[cap") ? " (capped)" : ""}` : "No matches";
    }
    case "glob":
      return lines.length ? `Found ${plural(lines.length, "file")}` : "No files";
    case "ls":
      return `${plural(lines.length, "entry")}`.replace("entrys", "entries");
    case "edit": {
      const hunks = buildEditDiff(item.input, reads);
      const all = hunks.flatMap((h) => h.lines);
      const a = all.filter((l) => l.type === "add").length;
      const d = all.filter((l) => l.type === "del").length;
      return hunks.length ? `Updated ${item.arg ?? "file"} with ${plural(a, "addition")} and ${plural(d, "removal")}` : (lines[0] ?? "");
    }
    case "write":
      return typeof item.input?.content === "string"
        ? `Wrote ${plural(buildWriteDiff(item.input.content).length, "line")} to ${item.arg ?? "file"}`
        : (lines[0] ?? "");
    case "bash":
      return lines.length ? "" : "(no output)";
    default:
      return lines[0] ?? "";
  }
}

/** Numbered read listing (hashes are plumbing, not content). */
function ReadRows({ text, cap }: { text: string; cap: number }) {
  const rows = text
    .split("\n")
    .map((l) => l.match(/^(\d+):[0-9a-f]{6}\|(.*)$/))
    .filter((m): m is RegExpMatchArray => !!m);
  return (
    <Box flexDirection="column" paddingLeft={5}>
      {rows.slice(0, cap).map((m, i) => (
        <Text key={i} wrap="truncate-end">
          <Text color="#4a4a4a">{m[1].padStart(4)} </Text>
          <Text color={DIM}>│ </Text>
          <Text>{m[2] === "" ? " " : m[2]}</Text>
        </Text>
      ))}
      {rows.length > cap ? <Text color={DIM}>… {rows.length - cap} more lines</Text> : null}
    </Box>
  );
}

function Output({ text, cap }: { text: string; cap: number }) {
  const lines = text.replace(/\r\n/g, "\n").replace(/\n+$/, "").split("\n");
  const shown = lines.slice(0, cap);
  return (
    <Box flexDirection="column">
      {shown.map((l, i) => (
        <Text key={i} wrap="truncate-end">
          <Text color={DIM}>{i === 0 ? "  ⎿  " : "     "}</Text>
          <Text color={GRAY}>{l === "" ? " " : l}</Text>
        </Text>
      ))}
      {lines.length > cap ? (
        <Text color={DIM}>
          {"     "}… +{lines.length - cap} lines (ctrl+o for full output)
        </Text>
      ) : null}
    </Box>
  );
}

/** grok-tool card variant — left gutter + ◆ heading for `task` sub-agents. */
function TaskCard({ item, verbose }: { item: ToolItem; verbose: boolean }) {
  const border = STATUS_COLOR[item.status];
  const lines = (item.detail ?? "").split("\n");
  return (
    <Box flexDirection="column" borderStyle="bold" borderLeft borderRight={false} borderTop={false} borderBottom={false} borderColor={border} paddingLeft={1}>
      <Text wrap="truncate-end">
        <Text color={border}>◆ </Text>
        <Text bold color={SILVER}>
          Task
        </Text>
        {item.arg ? <Text color={GRAY}> {item.arg}</Text> : null}
        {item.status === "pending" ? <Text color={DIM}>  working…</Text> : null}
      </Text>
      {lines.slice(0, verbose ? 30 : 3).map((l, i) => (
        <Text key={i} wrap="truncate-end" color={DIM}>
          {l}
        </Text>
      ))}
    </Box>
  );
}

function ToolRow({ item, verbose, reads }: { item: ToolItem; verbose: boolean; reads: ReadCache }) {
  if (item.name === "task") return <TaskCard item={item} verbose={verbose} />;
  const label = toolLabel(item.name);
  const summary = item.status === "pending" ? "" : summarize(item, reads);
  const text = item.detail ?? "";
  const cap = verbose ? 60 : 4;
  let body: React.ReactNode = null;
  if (item.status !== "pending") {
    if (item.status === "failed") {
      body = verbose ? <Output text={text} cap={cap} /> : null;
    } else if (item.name === "edit") {
      const hunks = buildEditDiff(item.input, reads);
      if (hunks.length)
        body = (
          <Box paddingLeft={5}>
            <Diff hunks={hunks} max={verbose ? 80 : 16} />
          </Box>
        );
    } else if (item.name === "write" && typeof item.input?.content === "string") {
      body = (
        <Box paddingLeft={5}>
          <Diff hunks={[{ header: `@@ ${item.arg ?? "file"} @@`, lines: buildWriteDiff(item.input.content) }]} max={verbose ? 80 : 10} />
        </Box>
      );
    } else if (item.name === "bash" && text.trim()) {
      body = <Output text={text} cap={cap} />;
    } else if (verbose && text.trim()) {
      body = item.name === "read" ? <ReadRows text={text} cap={cap} /> : <Output text={text} cap={cap} />;
    }
  }
  return (
    <Box flexDirection="column" marginTop={1}>
      <Text wrap="truncate-end">
        {item.status === "pending" ? <PendingMark /> : <Text color={STATUS_COLOR[item.status]}>{item.status === "failed" ? "✗ " : "⏺ "}</Text>}
        <Text bold color="#ececec">
          {label}
        </Text>
        {item.arg ? (
          <Text>
            <Text color={DIM}>(</Text>
            <Text color={CYAN}>{item.arg}</Text>
            <Text color={DIM}>)</Text>
          </Text>
        ) : null}
      </Text>
      {summary ? (
        <Text wrap="truncate-end">
          <Text color={DIM}>{"  ⎿  "}</Text>
          <Text color={item.status === "failed" ? "#9a9a9a" : DIM} italic={item.status === "failed"}>
            {summary}
          </Text>
        </Text>
      ) : null}
      {body}
    </Box>
  );
}

/** Welcome box — the zcli galaxy on the left, session facts on the right. */
export function HeaderBox({
  version,
  user,
  model,
  cwd,
  width,
}: {
  version: string;
  user: string;
  model: string;
  cwd: string;
  width: number;
}) {
  const wide = width >= 72;
  const grid = logoGrid(wide ? 24 : 12, wide ? 8 : 4);
  const title = ` zdev ${version} `;
  const inner = Math.max(20, width - 2);
  return (
    <Box flexDirection="column" marginBottom={1}>
      <Text color={ROSE}>
        {"╭─"}
        <Text bold color={ROSE}>
          {title}
        </Text>
        {"─".repeat(Math.max(2, inner - title.length - 1))}╮
      </Text>
      <Box borderStyle="round" borderTop={false} borderColor={ROSE} paddingX={1} width={width}>
        <Box flexDirection="column" flexShrink={0} marginRight={3}>
          {grid.map((row, r) => (
            <Text key={r}>
              {row.map((c, i) => {
                const s = logoShade(c.lum);
                return (
                  <Text key={i} color={s.color} bold={s.bold}>
                    {c.ch}
                  </Text>
                );
              })}
            </Text>
          ))}
        </Box>
        <Box flexDirection="column" flexGrow={1} flexShrink={1}>
          <Text bold color={ROSE}>
            Welcome back{user ? `, ${user}` : ""}.
          </Text>
          <Text color={GRAY} wrap="truncate-middle">
            {model}
          </Text>
          <Text color={DIM} wrap="truncate-middle">
            {cwd}
          </Text>
          {wide ? (
            <>
              <Text> </Text>
              <Text color={DIM} wrap="truncate">
                <Text color={GRAY}>/</Text> commands · <Text color={GRAY}>@</Text> files · <Text color={GRAY}>!</Text> shell ·{" "}
                <Text color={GRAY}>#</Text> remember
              </Text>
              <Text color={DIM} wrap="truncate">
                <Text color={GRAY}>shift+tab</Text> mode · <Text color={GRAY}>ctrl+o</Text> details · <Text color={GRAY}>esc</Text>{" "}
                interrupt
              </Text>
            </>
          ) : null}
        </Box>
      </Box>
    </Box>
  );
}

export function TranscriptItem({
  item,
  verbose,
  reads,
  header,
  cap,
}: {
  item: Item;
  verbose: boolean;
  reads: ReadCache;
  header?: React.ReactNode;
  /** streaming: show at most this many rows ({rows, width}) */
  cap?: { rows: number; width: number };
}) {
  switch (item.kind) {
    case "header":
      return <>{header}</>;
    case "user":
      return (
        <Box marginTop={1}>
          <Text wrap="wrap">
            <Text backgroundColor="#242424" color={ROSE} bold>
              {" ❯ "}
            </Text>
            <Text backgroundColor="#242424">{item.text} </Text>
          </Text>
        </Box>
      );
    case "assistant": {
      if (!item.text.trim()) return null;
      const view = cap ? tailView(item.text, cap.rows, cap.width) : { hidden: 0, text: item.text };
      return (
        <Box flexDirection="column" marginTop={1} paddingLeft={2}>
          {view.hidden ? <Text color={DIM}>⋮ {view.hidden} more lines above — the full reply lands in scrollback when it settles</Text> : null}
          <Markdown text={view.text} />
        </Box>
      );
    }
    case "tool":
      return <ToolRow item={item} verbose={verbose} reads={reads} />;
    case "plan": {
      const lines = item.text.split("\n");
      return (
        <Box flexDirection="column" marginTop={1} borderStyle="round" borderColor={YELLOW} paddingX={1}>
          <Text bold color={SILVER}>
            ◆ Plan
          </Text>
          <Box flexDirection="column" marginTop={1}>
            <Markdown text={lines.slice(0, verbose ? 200 : 40).join("\n")} />
          </Box>
          {lines.length > 40 && !verbose ? <Text color={DIM}>… {lines.length - 40} more lines</Text> : null}
          <Text color={DIM}>
            <Text color={ROSE}>/approve</Text> to run it · or keep refining
          </Text>
        </Box>
      );
    }
    case "note":
      return (
        <Box marginTop={item.tone === "error" ? 1 : 0}>
          <Text color={item.tone === "error" ? "#d6d6d6" : item.tone === "ok" ? GREEN : DIM} italic={item.tone !== "error"} wrap="wrap">
            {item.tone === "error" ? <Text color={RED}>✗ </Text> : "  "}
            {item.text}
          </Text>
        </Box>
      );
    case "shell": {
      const lines = item.out.replace(/\r\n/g, "\n").replace(/\n+$/, "").split("\n");
      const cap = verbose ? 60 : 12;
      return (
        <Box flexDirection="column" marginTop={1}>
          <Text>
            {item.pending ? <PendingMark /> : <Text color={item.code ? RED : GREEN}>{item.code ? "✗ " : "⏺ "}</Text>}
            <Text bold>$ </Text>
            <Text color={CYAN}>{item.cmd}</Text>
            {item.code ? <Text color={DIM}> (exit {item.code})</Text> : null}
          </Text>
          {!item.pending && item.out.trim() ? <Output text={lines.slice(0, cap + 1).join("\n")} cap={cap} /> : null}
        </Box>
      );
    }
    case "turn":
      return (
        <Box marginTop={1}>
          <Text>
            <Text color={SILVER}>◆ </Text>
            <Text color={DIM}>
              {item.cancelled ? "Interrupted" : "Done"} in {item.secs < 60 ? `${item.secs.toFixed(1)}s` : `${Math.floor(item.secs / 60)}m ${Math.round(item.secs % 60)}s`}
              {item.out ? ` · ↓ ${item.out.toLocaleString()} tokens` : ""}
            </Text>
          </Text>
        </Box>
      );
  }
}

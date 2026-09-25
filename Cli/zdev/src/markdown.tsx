// Markdown for assistant turns: headings, **bold**, *italic*, `code`,
// [links](url), fenced code with a language label and grayscale syntax
// highlighting, bullets (nested), numbered lists, quotes, rules and tables.
// Line-oriented — replies stream in and are committed paragraph by
// paragraph, so each chunk must render on its own.
import React from "react";
import { Box, Text } from "ink";

import { TOK_STYLE, tokenize } from "./highlight";
import { DIM, ROSE } from "./theme";

const INLINE = /(\*\*[^*\n]+\*\*|__[^_\n]+__|\*[^*\s][^*\n]*\*|`[^`\n]+`|\[[^\]\n]+\]\([^)\s]+\))/g;
const CODE = "#e0e0e0";
const RULE_C = "#3c3c3c";

export function Inline({ text, color }: { text: string; color?: string }) {
  const parts = text.split(INLINE);
  return (
    <>
      {parts.map((p, i) => {
        if (!p) return null;
        if ((p.startsWith("**") && p.endsWith("**")) || (p.startsWith("__") && p.endsWith("__")))
          if (p.length > 4)
            return (
              <Text key={i} bold color={color}>
                {p.slice(2, -2)}
              </Text>
            );
        if (p.startsWith("`") && p.endsWith("`") && p.length > 2)
          return (
            <Text key={i} color={CODE} backgroundColor="#1c1c1c">
              {p.slice(1, -1)}
            </Text>
          );
        const link = p.match(/^\[([^\]]+)\]\(([^)\s]+)\)$/);
        if (link)
          return (
            <Text key={i}>
              <Text underline color={color}>
                {link[1]}
              </Text>
              {link[2] !== link[1] ? <Text color={DIM}> ({link[2]})</Text> : null}
            </Text>
          );
        if (p.startsWith("*") && p.endsWith("*") && p.length > 2)
          return (
            <Text key={i} italic color={color}>
              {p.slice(1, -1)}
            </Text>
          );
        return (
          <Text key={i} color={color}>
            {p}
          </Text>
        );
      })}
    </>
  );
}

function CodeLine({ line, lang }: { line: string; lang: string }) {
  const toks = tokenize(line, lang);
  return (
    <Text wrap="truncate-end">
      <Text color={RULE_C}>│ </Text>
      {toks.length ? (
        toks.map((t, i) => {
          const s = TOK_STYLE[t.kind];
          return (
            <Text key={i} color={s.color} bold={s.bold} italic={s.italic}>
              {t.text}
            </Text>
          );
        })
      ) : (
        <Text> </Text>
      )}
    </Text>
  );
}

const cells = (row: string) =>
  row
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((c) => c.trim());

function Table({ rows }: { rows: string[] }) {
  const body = rows.filter((r, i) => !(i === 1 && /^\s*\|?\s*:?-{2,}/.test(r)));
  const grid = body.map(cells);
  const ncol = Math.max(...grid.map((r) => r.length));
  const plain = (s: string) => s.replace(/\*\*|__|`/g, "");
  const width = Array.from({ length: ncol }, (_, c) => Math.min(40, Math.max(3, ...grid.map((r) => plain(r[c] ?? "").length))));
  const line = (l: string, m: string, r: string) => l + width.map((w) => "─".repeat(w + 2)).join(m) + r;
  return (
    <Box flexDirection="column">
      <Text color={RULE_C}>{line("┌", "┬", "┐")}</Text>
      {grid.map((r, ri) => (
        <React.Fragment key={ri}>
          <Text wrap="truncate-end">
            {width.map((w, c) => {
              const v = plain(r[c] ?? "");
              const shown = v.length > w ? v.slice(0, w - 1) + "…" : v.padEnd(w);
              return (
                <Text key={c}>
                  <Text color={RULE_C}>│ </Text>
                  <Text bold={ri === 0}>{shown} </Text>
                </Text>
              );
            })}
            <Text color={RULE_C}>│</Text>
          </Text>
          {ri === 0 && grid.length > 1 ? <Text color={RULE_C}>{line("├", "┼", "┤")}</Text> : null}
        </React.Fragment>
      ))}
      <Text color={RULE_C}>{line("└", "┴", "┘")}</Text>
    </Box>
  );
}

export function Markdown({ text }: { text: string }) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out: React.ReactNode[] = [];
  let fence: { lang: string } | null = null;
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    const f = l.match(/^\s*(```|~~~)\s*([\w+#.-]*)/);
    if (f) {
      if (fence) {
        fence = null;
      } else {
        fence = { lang: f[2] || "" };
        if (fence.lang)
          out.push(
            <Text key={i} color={DIM}>
              {"  "}
              {fence.lang}
            </Text>,
          );
      }
      continue;
    }
    if (fence) {
      out.push(<CodeLine key={i} line={l} lang={fence.lang} />);
      continue;
    }
    // tables: a header row followed by a |---| separator
    if (/^\s*\|.*\|\s*$/.test(l) && /^\s*\|?\s*:?-{2,}/.test(lines[i + 1] ?? "")) {
      const rows: string[] = [];
      while (i < lines.length && /^\s*\|.*\|?\s*$/.test(lines[i]) && lines[i].includes("|")) rows.push(lines[i++]);
      i--;
      out.push(<Table key={i} rows={rows} />);
      continue;
    }
    if (/^\s*(---|\*\*\*|___)\s*$/.test(l)) {
      out.push(
        <Text key={i} color={RULE_C}>
          {"─".repeat(40)}
        </Text>,
      );
      continue;
    }
    const q = l.match(/^(\s*)>\s?(.*)$/);
    if (q) {
      out.push(
        <Text key={i}>
          {q[1]}
          <Text color={RULE_C}>▎ </Text>
          <Text italic color="#b0b0b0">
            <Inline text={q[2]} />
          </Text>
        </Text>,
      );
      continue;
    }
    const h = l.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      out.push(
        <Text key={i} bold color={h[1].length <= 2 ? ROSE : undefined} underline={h[1].length === 1}>
          <Inline text={h[2]} />
        </Text>,
      );
      continue;
    }
    const b = l.match(/^(\s*)([-*+•]|\d+[.)])\s+(\[[ xX]\]\s+)?(.*)$/);
    if (b) {
      const depth = Math.floor(b[1].replace(/\t/g, "  ").length / 2);
      const numbered = /^\d/.test(b[2]);
      const box = b[3] ? (/x/i.test(b[3]) ? "☒ " : "☐ ") : "";
      out.push(
        <Text key={i}>
          {"  ".repeat(depth)}
          <Text color={DIM}>{numbered ? b[2] : depth % 2 ? "◦" : "•"} </Text>
          {box ? <Text color={DIM}>{box}</Text> : null}
          <Inline text={b[4]} />
        </Text>,
      );
      continue;
    }
    out.push(
      <Text key={i}>
        <Inline text={l} />
      </Text>,
    );
  }
  return <>{out}</>;
}

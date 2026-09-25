// Tiny markdown-ish renderer for assistant turns — **bold**, *italic*,
// `code`, ``` fenced blocks, -/* bullets, # headings. Not a real parser;
// line-oriented with inline tokenizing, which is all chat output needs.
import React from "react";
import { Text } from "ink";
import { DIM, ROSE } from "./theme";

const INLINE = /(\*\*[^*]+\*\*|\*[^*\n]+\*|`[^`\n]+`)/g;

function Inline({ text }: { text: string }) {
  const parts = text.split(INLINE);
  return (
    <>
      {parts.map((p, i) => {
        if (p.startsWith("**") && p.endsWith("**") && p.length > 4)
          return (
            <Text key={i} bold>
              {p.slice(2, -2)}
            </Text>
          );
        if (p.startsWith("`") && p.endsWith("`") && p.length > 2)
          return (
            <Text key={i} color={ROSE}>
              {p.slice(1, -1)}
            </Text>
          );
        if (p.startsWith("*") && p.endsWith("*") && p.length > 2)
          return (
            <Text key={i} italic>
              {p.slice(1, -1)}
            </Text>
          );
        return <Text key={i}>{p}</Text>;
      })}
    </>
  );
}

export function Markdown({ text }: { text: string }) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out: React.ReactNode[] = [];
  let inFence = false;
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (/^\s*```/.test(l)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) {
      out.push(
        <Text key={i} color={GRAYISH}>
          {"  "}
          {l}
        </Text>,
      );
      continue;
    }
    if (/^\s*(---|\*\*\*|___)\s*$/.test(l)) {
      out.push(
        <Text key={i} color={DIM}>
          {"─".repeat(24)}
        </Text>,
      );
      continue;
    }
    const q = l.match(/^(\s*)>\s?(.*)$/);
    if (q) {
      out.push(
        <Text key={i} color={GRAYISH}>
          {q[1]}
          <Text color={DIM}>│ </Text>
          <Text italic>{q[2]}</Text>
        </Text>,
      );
      continue;
    }
    const h = l.match(/^(#{1,4})\s+(.*)$/);
    if (h) {
      out.push(
        <Text key={i} bold color={ROSE}>
          {h[2]}
        </Text>,
      );
      continue;
    }
    const b = l.match(/^(\s*)([-*•]|\d+\.)\s+(.*)$/);
    if (b) {
      out.push(
        <Text key={i}>
          {b[1]}
          <Text color={DIM}>{/^\d/.test(b[2]) ? b[2] : "•"} </Text>
          <Inline text={b[3]} />
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

const GRAYISH = "#a9a9a9";

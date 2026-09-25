// Footer — usage/session line under the composer. Right side carries the
// grok-status grammar: `ctx used / limit` when the model's window is known.
import React from "react";
import { Box, Text } from "ink";
import { DIM, GREEN, YELLOW } from "../theme";
import type { Usage } from "../types";

function fmt(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

export function Footer({
  usage,
  sessionId,
  status,
  ctxLimit,
}: {
  usage: Usage;
  sessionId: string;
  status: string;
  ctxLimit?: number;
}) {
  const seen = usage.input + usage.output + usage.cache_read + usage.cache_write;
  const ctxPct = ctxLimit ? usage.input / ctxLimit : 0;
  const ctxColor = ctxPct > 0.85 ? "#6f6f6f" : ctxPct > 0.6 ? YELLOW : GREEN;
  return (
    <Box justifyContent="space-between">
      <Text color={DIM}>
        {seen || usage.cum_usd
          ? `↑ ${usage.input} · ↓ ${usage.output}` +
            (usage.cache_read ? ` · cr ${usage.cache_read}` : "") +
            (usage.cum_usd ? ` · Σ $${usage.cum_usd.toFixed(4)}` : "")
          : ""}
      </Text>
      <Text color={DIM}>
        {ctxLimit ? (
          <>
            <Text>ctx </Text>
            <Text color={ctxColor}>{fmt(usage.input)}</Text>
            <Text> / {fmt(ctxLimit)}</Text>
            <Text> · </Text>
          </>
        ) : null}
        {status ? `${status} · ` : ""}
        {sessionId ? `session ${sessionId.slice(0, 8)}` : "connecting…"}
      </Text>
    </Box>
  );
}

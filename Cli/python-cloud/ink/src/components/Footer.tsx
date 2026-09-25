// Footer — usage/session line under the composer (dims when idle).
import React from "react";
import { Box, Text } from "ink";
import { DIM } from "../theme";
import type { Usage } from "../types";

export function Footer({
  usage,
  sessionId,
  status,
}: {
  usage: Usage;
  sessionId: string;
  status: string;
}) {
  const seen = usage.input + usage.output + usage.cache_read + usage.cache_write;
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
        {status ? `${status} · ` : ""}
        {sessionId ? `session ${sessionId.slice(0, 8)}` : "connecting…"}
      </Text>
    </Box>
  );
}

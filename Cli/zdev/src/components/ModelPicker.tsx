// Model picker — /model overlay listing config.toml aliases via the
// session/list_models ACP call. FREE chip when all prices are 0.
import React from "react";
import { Box, Text } from "ink";
import { ACTIVE, DIM, GRAY, GREEN, ROSE } from "../theme";
import type { Model } from "../types";

export function ModelPicker({
  models,
  sel,
  current,
  loading,
}: {
  models: Model[];
  sel: number;
  current: string;
  loading: boolean;
}) {
  return (
    <Box
      flexDirection="column"
      borderStyle="round"
      borderColor={ROSE}
      paddingX={1}
      marginTop={1}
    >
      <Text bold color={ROSE}>
        Select model
      </Text>
      {loading ? (
        <Text color={DIM}>loading…</Text>
      ) : (
        models.map((m, i) => {
          const on = sel === i;
          return (
            <Text key={m.alias} wrap="truncate-end">
              <Text color={on ? ACTIVE : GRAY} bold={on}>
                {on ? "❯ " : "  "}
                {m.alias.padEnd(18)}
              </Text>
              <Text color={DIM}>{m.provider.padEnd(10)}</Text>
              <Text color={GRAY}>{m.id}</Text>
              {m.free ? <Text color={GREEN}> FREE</Text> : null}
              {m.alias === current ? <Text color={ROSE}> ●</Text> : null}
            </Text>
          );
        })
      )}
      <Text color={DIM}>↑/↓ move · enter select · esc cancel</Text>
    </Box>
  );
}

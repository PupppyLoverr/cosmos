// Session picker — /resume listbox. Same visual grammar as ModelPicker:
// ❯ active row, dim metadata, newest first.
import React from "react";
import { Box, Text } from "ink";
import { ACTIVE, DIM, GRAY, ROSE } from "../theme";

export function SessionPicker({
  sessions,
  sel,
  current,
}: {
  sessions: Record<string, any>[];
  sel: number;
  current: string;
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
        Resume session
      </Text>
      {sessions.map((s, i) => {
        const on = sel === i;
        const when = String(s.started ?? "").slice(5, 16).replace("T", " ");
        const preview = String(s.preview ?? "").slice(0, 46) || "(empty)";
        return (
          <Text key={s.id} wrap="truncate-end">
            <Text color={on ? ACTIVE : GRAY} bold={on}>
              {on ? "❯ " : "  "}
              {(s.id ?? "").slice(0, 12).padEnd(14)}
            </Text>
            <Text color={DIM}>{when.padEnd(12)}</Text>
            <Text color={on ? undefined : DIM}>
              {preview}
              {s.id === current ? <Text color={ROSE}> ●</Text> : null}
            </Text>
          </Text>
        );
      })}
      <Text color={DIM}>↑/↓ move · enter resume · esc cancel</Text>
    </Box>
  );
}

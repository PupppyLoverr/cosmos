// Permission radiogroup — port of claude-permission: "Do you want to
// proceed?" box, arrow-key/number selection, ❯ on the active row.
import React from "react";
import { Box, Text } from "ink";
import { DIM, GRAY, ROSE } from "../theme";
import { summarizeArg } from "../reducer";
import type { PermissionReq } from "../types";

export function Permission({
  req,
  sel,
}: {
  req: PermissionReq;
  sel: number;
}) {
  const arg = summarizeArg(req.tool, req.input);
  return (
    <Box
      flexDirection="column"
      borderStyle="round"
      borderColor={ROSE}
      paddingX={1}
      marginTop={1}
    >
      <Text bold color={ROSE}>
        {req.tool}
        {arg ? <Text color={GRAY}>({arg})</Text> : null}
      </Text>
      <Text> </Text>
      <Text>Do you want to proceed?</Text>
      {req.options.map((o, i) => {
        const on = sel === i;
        return (
          <Text key={o.id}>
            <Text color={on ? ROSE : DIM}>{on ? "❯ " : "  "}</Text>
            <Text
              color={on ? undefined : GRAY}
              backgroundColor={on ? "#2b1a14" : undefined}
            >
              {i + 1}. {o.name}
            </Text>
          </Text>
        );
      })}
      <Text color={DIM}>↑/↓ move · enter select · esc deny</Text>
    </Box>
  );
}

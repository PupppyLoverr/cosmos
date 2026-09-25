// Permission radiogroup — port of claude-permission: titled box, the exact
// command/diff you're approving on top, "Do you want to proceed?" then
// arrow-key/number options with ❯ on the active row.
import React from "react";
import { Box, Text } from "ink";
import { CYAN, DIM, GRAY, ROSE } from "../theme";
import { summarizeArg, toolLabel } from "../reducer";
import { buildEditDiff, buildWriteDiff } from "../diff";
import { Diff } from "./Diff";
import type { ReadCache } from "./Transcript";
import type { PermissionReq } from "../types";

/** What you're actually approving — command line or diff preview. */
function Preview({ req, reads }: { req: PermissionReq; reads: ReadCache }) {
  if (req.tool === "bash" && typeof req.input?.command === "string") {
    return <Text color={CYAN}>{req.input.command}</Text>;
  }
  if (req.tool === "edit") {
    const hunks = buildEditDiff(req.input, reads);
    if (hunks.length) return <Diff hunks={hunks} max={12} />;
  }
  if (req.tool === "write" && typeof req.input?.content === "string") {
    const lines = buildWriteDiff(req.input.content);
    return <Diff hunks={[{ header: "@@ new file @@", lines }]} max={12} />;
  }
  const arg = summarizeArg(req.tool, req.input);
  return arg ? <Text color={CYAN}>{arg}</Text> : null;
}

export function Permission({
  req,
  sel,
  reads,
}: {
  req: PermissionReq;
  sel: number;
  reads: ReadCache;
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
        {toolLabel(req.tool)}
        {arg && req.tool !== "bash" ? (
          <Text color={GRAY}>({arg})</Text>
        ) : null}
      </Text>
      <Preview req={req} reads={reads} />
      <Text> </Text>
      <Text>Do you want to proceed?</Text>
      {req.options.map((o, i) => {
        const on = sel === i;
        return (
          <Text key={o.id}>
            <Text color={on ? ROSE : DIM}>{on ? "❯ " : "  "}</Text>
            <Text
              color={on ? undefined : GRAY}
              backgroundColor={on ? "#262626" : undefined}
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

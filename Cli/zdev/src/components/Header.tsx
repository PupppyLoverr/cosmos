// Welcome box — port of brainless claude-header. The galaxy is generated
// (Galaxy.tsx: seeded logarithmic-spiral splat → dot glyphs), not art text.
// Title rides the top border like a fieldset legend.
import React from "react";
import { Box, Text } from "ink";
import { DIM, GRAY, ROSE } from "../theme";
import { Galaxy } from "./Galaxy";

export function Logo() {
  return (
    <Box flexDirection="column">
      <Galaxy width={40} height={13} />
    </Box>
  );
}

export function Header({
  version,
  user,
  model,
  cwd,
  width,
  tips = [
    "type a task, hit enter",
    "@file · ctrl+m dictates",
    "/ commands · ctrl+o expands",
  ],
  whatsNew = [
    "real diffs on every edit",
    "/resume · /login · ctx meter",
  ],
}: {
  version: string;
  user: string;
  model: string;
  cwd: string;
  width: number;
  tips?: string[];
  whatsNew?: string[];
}) {
  const title = ` zdev ${version} `;
  const fill = Math.max(4, width - title.length - 5);
  // galaxy column adapts to terminal width; info column takes the rest
  const gw = Math.max(24, Math.min(40, width - 34));
  return (
    <Box flexDirection="column" marginBottom={1}>
      <Text color={ROSE}>
        {"╭─"}
        <Text bold color={ROSE}>
          {title}
        </Text>
        {"─".repeat(fill)}╮
      </Text>
      <Box
        borderStyle="round"
        borderTop={false}
        borderColor={ROSE}
        paddingX={1}
      >
        <Box flexDirection="column" width={gw + 1} flexShrink={0}>
          <Galaxy width={gw} height={13} />
        </Box>
        <Box
          flexDirection="column"
          flexGrow={1}
          paddingLeft={2}
          paddingTop={1}
        >
          <Text bold color={ROSE}>
            Welcome back{user ? `, ${user}` : ""}.
          </Text>
          <Text color={GRAY} wrap="truncate">
            {model}
          </Text>
          <Text color={GRAY} wrap="truncate">
            {cwd}
          </Text>
          <Text> </Text>
          <Text bold color={ROSE}>
            Tips
          </Text>
          {tips.map((t, i) => (
            <Text key={i} color={DIM} wrap="truncate">
              {t}
            </Text>
          ))}
          <Text> </Text>
          <Text bold color={ROSE}>
            What's new
          </Text>
          {whatsNew.map((t, i) => (
            <Text key={i} color={DIM} wrap="truncate">
              {t}
            </Text>
          ))}
        </Box>
      </Box>
    </Box>
  );
}

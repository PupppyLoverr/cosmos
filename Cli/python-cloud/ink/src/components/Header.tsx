// Welcome box — port of brainless claude-header: pixel logo, "Welcome back",
// model/cwd lines left; tips + what's-new right. Title rides the top border.
import React from "react";
import { Box, Text } from "ink";
import { DIM, GRAY, ROSE } from "../theme";

// Blocky "Z" sprite (zdev), 12×6 bitmap — each pixel is two cells wide so it
// stays proportional on ~2:1 terminal cells.
const LOGO_BITS = [
  "111111111111",
  "000000001110",
  "000000111000",
  "000011100000",
  "001110000000",
  "111111111111",
];

export function Logo({ color = ROSE }: { color?: string }) {
  return (
    <Box flexDirection="column">
      {LOGO_BITS.map((row, y) => (
        <Text key={y} color={color}>
          {row.replace(/1/g, "██").replace(/0/g, "  ")}
        </Text>
      ))}
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
    "Ask zdev to build features, fix bugs, or work on your code",
    "/ for commands · ctrl+o expands tool output",
  ],
  whatsNew = [
    "Permission prompts: allow once / always / deny",
    "Modes plan · agent · yolo, switchable live",
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
  return (
    <Box flexDirection="column" marginBottom={1}>
      {/* title embedded in the top rule, like the brainless fieldset legend */}
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
        justifyContent="space-between"
      >
        <Box flexDirection="column" width="55%">
          <Box flexDirection="row">
            <Logo />
            <Box flexDirection="column" marginLeft={2} marginTop={1}>
              <Text bold color={ROSE}>
                Welcome back {user}!
              </Text>
              <Text> </Text>
              <Text color={GRAY}>{model}</Text>
              <Text color={GRAY}>{cwd}</Text>
            </Box>
          </Box>
        </Box>
        <Box flexDirection="column" width="42%" paddingLeft={1}>
          <Text bold color={ROSE}>
            Tips for getting started
          </Text>
          {tips.map((t, i) => (
            <Text key={i} wrap="truncate">
              {t}
            </Text>
          ))}
          <Text> </Text>
          <Text bold color={ROSE}>
            What's new
          </Text>
          {whatsNew.map((t, i) => (
            <Text key={i} wrap="truncate" color={DIM}>
              {t}
            </Text>
          ))}
          <Text color={DIM}>/help for more</Text>
        </Box>
      </Box>
    </Box>
  );
}

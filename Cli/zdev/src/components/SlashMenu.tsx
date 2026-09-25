// Slash palette — port of claude-slash-menu: sits above the composer,
// prefix-filters on the text after `/`, active row is light blue.
import React from "react";
import { Box, Text } from "ink";
import { ACTIVE, DIM, GRAY } from "../theme";

const NAME_COLS = 12;

export function SlashMenu({
  list,
  active,
}: {
  list: { name: string; desc: string }[];
  active: number;
}) {
  if (!list.length) return null;
  return (
    <Box flexDirection="column" marginTop={1}>
      {list.slice(0, 9).map((c, i) => {
        const on = i === active;
        return (
          <Text key={c.name} wrap="truncate-end">
            <Text color={on ? ACTIVE : GRAY} bold={on}>
              {on ? "❯ " : "  "}
              {c.name.padEnd(NAME_COLS)}
            </Text>
            <Text color={on ? undefined : DIM}>{c.desc}</Text>
          </Text>
        );
      })}
    </Box>
  );
}

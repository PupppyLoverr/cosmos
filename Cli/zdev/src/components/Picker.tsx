// Picker — the one list overlay behind /model, /connect, /resume and the
// `/` and `@` menus: optional search row, a scrolling window with ❯ on the
// active row, aligned columns, and a key legend.
import React from "react";
import { Box, Text } from "ink";

import { ACTIVE, DIM, GHOST, GRAY, GREEN, ROSE, RULE } from "../theme";

export type Row = {
  key: string;
  label: string;
  /** dim text after the label */
  detail?: string;
  /** right-aligned column */
  right?: string;
  /** green tag after the label (✓ connected, ★ default) */
  tag?: string;
  /** section heading shown above this row */
  section?: string;
};

export function Picker({
  title,
  subtitle,
  query,
  placeholder = "Type to search",
  rows,
  sel,
  scroll,
  height = 10,
  width,
  footer,
  loading,
  empty = "nothing matches",
  boxed = true,
}: {
  title?: string;
  subtitle?: React.ReactNode;
  /** undefined = no search row */
  query?: string;
  placeholder?: string;
  rows: Row[];
  sel: number;
  /** first visible row, for lists without a selection (sel = -1) */
  scroll?: number;
  height?: number;
  width: number;
  footer?: string;
  loading?: string;
  empty?: string;
  boxed?: boolean;
}) {
  const n = rows.length;
  const s = sel < 0 ? -1 : Math.max(0, Math.min(sel, n - 1));
  const first = Math.max(0, Math.min(s < 0 ? (scroll ?? 0) : s - Math.floor(height / 2), n - height));
  const shown = rows.slice(first, first + height);
  const labelW = Math.min(44, Math.max(8, ...shown.map((r) => r.label.length)) + 2);
  const inner = Math.max(20, width - (boxed ? 4 : 0));
  const body = (
    <>
      {title ? (
        <Text bold color={ROSE}>
          {title}
        </Text>
      ) : null}
      {subtitle ? <Box>{typeof subtitle === "string" ? <Text color={DIM}>{subtitle}</Text> : subtitle}</Box> : null}
      {query !== undefined ? (
        <Box flexDirection="column" marginTop={title ? 1 : 0}>
          <Text>
            <Text color={DIM}>⌕ </Text>
            {query ? <Text>{query}</Text> : <Text color={GHOST}>{placeholder}</Text>}
            <Text inverse> </Text>
          </Text>
          <Text color={RULE}>{"─".repeat(inner)}</Text>
        </Box>
      ) : null}
      {loading ? (
        <Text color={GRAY}>{loading}</Text>
      ) : n === 0 ? (
        <Text color={DIM}>{empty}</Text>
      ) : (
        <>
          {first > 0 ? <Text color={DIM}>  ↑ {first} more</Text> : null}
          {shown.map((r, i) => {
            const on = first + i === s;
            return (
              <React.Fragment key={r.key}>
                {r.section && (i > 0 || first === 0) ? (
                  <Text color={DIM} bold>
                    {r.section}
                  </Text>
                ) : null}
                <Box width={inner}>
                  <Box flexGrow={1} flexShrink={1}>
                    <Text wrap="truncate-end">
                      <Text color={on ? ROSE : DIM}>{on ? "❯ " : "  "}</Text>
                      <Text color={on ? ACTIVE : GRAY} bold={on}>
                        {r.label.padEnd(labelW)}
                      </Text>
                      {r.detail ? <Text color={DIM}>{r.detail}</Text> : null}
                      {r.tag ? <Text color={GREEN}> {r.tag}</Text> : null}
                    </Text>
                  </Box>
                  {r.right ? (
                    <Box flexShrink={0} marginLeft={1}>
                      <Text color={DIM}>{r.right}</Text>
                    </Box>
                  ) : null}
                </Box>
              </React.Fragment>
            );
          })}
          {first + height < n ? <Text color={DIM}>  ↓ {n - first - height} more</Text> : null}
        </>
      )}
      {footer ? (
        <Box marginTop={boxed ? 1 : 0}>
          <Text color={DIM}>{footer}</Text>
        </Box>
      ) : null}
    </>
  );
  if (!boxed) return <Box flexDirection="column">{body}</Box>;
  return (
    <Box flexDirection="column" borderStyle="round" borderColor="#5a5a5a" paddingX={1} marginTop={1} width={width}>
      {body}
    </Box>
  );
}

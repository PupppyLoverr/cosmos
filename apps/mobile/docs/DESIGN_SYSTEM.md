# Mobile design system

This document describes the presentation tokens and reusable UI primitives in
`apps/mobile`. Values are defined in `src/lib/theme.ts`; components consume
those values rather than redefining them in screens.

## Tokens

### Spacing and layout

| Token | Value | Use |
|---|---:|---|
| `spacing(n)` | `n * 4` | 4-point layout grid |
| `edge` | 16 | Screen-edge content inset |
| `radii.control` / `sm` | 8 | Small controls |
| `radii.row` | 12 | Icon tiles, attachment chips |
| `radii.panel` / `md` | 14 | Grouped settings cards, secondary buttons |
| `radii.lg` | 16 | Cards |
| `radii.xl` / `bubble` | 20 | Transcript bubbles |
| `radii.xxl` / `composer` | 24 | The floating composer card |
| `radii.chip` / `cta` / `pill` | 999 | Host/filter chips, ink CTAs, search pill, circle buttons |

The shape language follows the Codex / Cursor references: floating white
chrome (circle buttons, the search pill, the composer card) with a soft
`floatShadow`, capsule chips and CTAs, plain list rows with no card behind
them. `spacing`, `edge`, and `radii` live in `src/lib/theme.ts`.

### Navigation

There is no tab bar. Home (`app/index.tsx`) is the root; the sidebar
(`components/drawer.tsx`) opens from the menu circle or a swipe from the left
edge and links to Sessions, Desktops, Pair and Settings, plus recent sessions.
Every screen draws its own chrome with `TopBar` / `CircleButton` /
`ScreenTitle` from `components/chrome.tsx` — no native headers.

### Typography

The `type` ramp uses SF-style sizes and line heights. Text colors are applied
from the resolved theme at the component call site.

| Token | Size / line height | Primary use |
|---|---|---|
| `largeTitle` | 34 / 41, semibold | Native large navigation titles |
| `title` | 28 / 34, semibold | Screen-level title (onboarding beats) |
| `title2` | 22 / 28 | Sheet title |
| `title3` | 20 / 25 | Section hero |
| `headline` | 17 / 22 | Emphasized button text, empty-state line |
| `body` | 16 / 22 | Main row and form text |
| `callout` | 16 / 21 | Supporting prominent text |
| `subhead` | 15 / 20 | Supporting row text |
| `footnote` | 13 / 18 | Secondary text |
| `caption` | 12 / 16 | Status chips |
| `caption2` | 11 / 13 | Small labels |
| `meta` | 13 / 18 | harness · desktop · time under a title |
| `sessionTitle` | 16 / 21, semibold | Session / desktop card title |
| `composer` | 16 / 22 | Composer input |
| `transcript` | 16 / 23 | Markdown transcript |
| `mono` | 13 / 18 | Menlo on iOS, monospace elsewhere |

One family (SF Pro / Roboto). The only display type is the "Cosmos" wordmark
on the onboarding splash. Text remains scalable by default; rows must allow
content to wrap at larger Dynamic Type sizes.

### Color, status, and accents

The default Cosmos family is white `#FFFFFF` (grouped fills `#F4F4F5`) / near-black `#0B0B0C` (the
dark splash value in `app.json`) with ink text and one quiet link-blue accent
(`#0A66D9` light, `#5E9EFF` dark). Primary buttons are ink, not accent. User
turns are an inverse ink bubble. No purple, no gradient mesh.

`resolveTheme` produces semantic colors for light/dark schemes, surface
materials, backdrops, wallpaper, and transparency preferences
(`src/lib/theme.ts:308-366`). Components should use the resolved theme rather
than hard-coded semantic colors.

`statusColor(t, status)` maps:

- `working` to `t.link`
- `awaitingInput` to `t.warning`
- `errored` to `t.danger`
- `completed` and `connected` to `t.success`
- other statuses to `t.textTertiary`

The mapping is at `src/lib/theme.ts:545-550`. Theme families and accent choices
are defined by `families`, `familyById`, `accentOptions`, and `accentSwatch`
(`src/lib/theme.ts:125-243`). `tintFor` provides deterministic decorative
avatar colors (`src/lib/theme.ts:552-558`).

### Shadows and motion

`shadows.card`, `shadows.raised`, and `shadows.overlay` are the shared elevation
values (`src/lib/theme.ts:519-523`). Motion durations and curves are exported
as `motion`, `enterBezier`, and `collapseBezier`
(`src/lib/theme.ts:525-543`).

## Surface primitives

- `Glass` selects Liquid Glass, blur, tonal, or opaque presentation based on
  platform and theme support (`src/components/surface.tsx:41-98`).
- `PresetGradient` renders a theme backdrop preset
  (`src/components/surface.tsx:100-114`).
- `Backdrop` renders the configured wallpaper/backdrop when enabled
  (`src/components/surface.tsx:121-147`).
- `Screen` is the root themed container for a screen
  (`src/components/surface.tsx:149-158`).

Screens should use `Screen` for the root and `Glass` for grouped surfaces.
`Backdrop` is provided by the screen composition when the selected theme uses a
backdrop.

## Shared components

### Rows and groups

`Row` is an iOS-style single setting or action row. Use it for a title,
optional subtitle/value, leading icon, trailing content, selection, and
navigation affordance (`src/components/bits.tsx:224-272`).

`Group` is the inset grouped container for related rows. Use `header` and
`footer` for section context; it inserts separators between children
(`src/components/bits.tsx:306-328`).

`SectionLabel` is for standalone section headings. `Caption` is secondary text
that remains readable over wallpaper (`src/components/bits.tsx:330-349`).

### Buttons and controls

- `Button` is a labeled action with `primary`, `tinted`, `secondary`, `ghost`,
  and `destructive` variants and `sm`, `md`, `lg` sizes
  (`src/components/bits.tsx:75-114`).
- `IconButton` is for circular icon-only actions. It requires a spoken `label`
  (`src/components/bits.tsx:116-144`).
- `Chip` is a selectable suggestion/filter capsule
  (`src/components/bits.tsx:146-164`).
- `Segmented` is a mutually exclusive compact choice control
  (`src/components/bits.tsx:169-193`).
- `Slider` hosts the native Expo UI slider
  (`src/components/bits.tsx:199-209`).
- `Tag` is a compact non-interactive label
  (`src/components/bits.tsx:215-222`).

### Status and connection

`StatusDot` is a compact color indicator. Use it when the surrounding text
already explains the state (`src/components/bits.tsx:56-62`).

`StatusPill` combines a status dot and label. Use it for visible state labels
such as Working, Input, Failed, Running, and Done. It supports `sm` and `md`
sizes and gates its pulse through Reduce Motion (`src/components/bits.tsx`).

`StatusChip` is the trailing state on a session card: Live / Waiting / Done /
Error on a 6pt-radius tint. Idle sessions draw no chip — the time is in the
meta line.

`DesktopChip` is the Codex-style host chip (name + presence dot) shown at the
top of Sessions. Tapping it opens Desktops.

`PromptRow` is a suggested prompt as a plain text row. Suggestions are never
cards, never starred, and only name things Cosmos actually does.

`SkeletonRows` renders the standard loading placeholders for list screens
(`src/components/bits.tsx`).

### States and notices

`Notice` is an inline informational, warning, or error message. Use it when
content can remain in the current flow. Non-info notices expose the `alert`
accessibility role; info notices expose a combined label
(`src/components/bits.tsx:394-409`).

`EmptyState` is for a screen or list with no content: one line and at most
one action. No glyph tiles, illustrations, or sparkles.

Do not use `EmptyState` for transient connection warnings; use `Notice`.

## Accessibility

- Interactive targets use a minimum 44pt effective target. `HIT_SLOP_44`
  calculates symmetric extra hit slop for smaller visuals (`src/lib/a11y.ts`).
- Icon-only controls require an explicit accessibility label.
- `Chip` labels itself with its visible label.
- `Row` composes a default label from title, subtitle, and value unless the
  caller provides one.
- Empty-state titles use the `header` role.
- Warning/error notices use the `alert` role. Info notices use an accessible
  combined title/body label.
- `useFontScale` reads the system font scale and `useLargeText` returns true at
  `fontScale >= 1.3` (`src/lib/a11y.ts`).
- Rows may wrap titles to two lines when a subtitle is present and large text
  is enabled.
- Do not disable Dynamic Type globally. Review constrained labels at large and
  accessibility text sizes.
- `resolveTheme` already considers Reduce Transparency; preserve that behavior.

## Motion

`useReduceMotion` is defined in `src/lib/a11y.ts` and re-exported from
`src/lib/motion.tsx` so existing imports remain valid. It tracks
`AccessibilityInfo.isReduceMotionEnabled()` and runtime changes.

Decorative pulses, entrance transitions, and disclosure transitions must
respect `useReduceMotion`. `StatusDot` and `StatusPill` already gate pulses.
When reduced motion is enabled, use the final visual state without animation.

## Desktop-to-iOS mapping

The mobile tokens mirror the desktop UI values documented in
`docs/2026-09-22-ios-redesign-plan.md:45-68`:

- 4px desktop grid → `spacing(n)`
- 16px panel inset → `edge`
- 6px control radius → `radii.control`
- 8px session row radius → `radii.row`
- 10px panel radius → `radii.panel`
- 16px bubble radius → `radii.bubble`
- 26px composer radius → `radii.composer`
- Desktop metadata/title sizes → `type.meta` and `type.sessionTitle`
- Desktop input/transcript sizes → `type.composer` and `type.transcript`
- Desktop motion timings → `motion`, `enterBezier`, `collapseBezier`
- Desktop status colors → `statusColor`

Use existing values for presentation changes. New visual values require a
token decision rather than a screen-local literal.

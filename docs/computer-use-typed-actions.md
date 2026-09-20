# Typed Computer Use actions — plan (no implementation in this PR)

Status: research / design only. Nothing in this document is wired up. The
implementation path stays exactly what ships today:

```
harness / model  →  Cosmos Computer API (crates/computer)  →  cua-driver (open source)  →  macOS / Windows / Linux
```

No new dependency. No cloud Computer Use. No Jev / Jev Cloud / Cua Cloud. The
"Jev" material below is used for ideas only — Cosmos does not call TypeSafe.

## 1. Why

The Save-sheet proof (54 MCP round-trips for a two-line TextEdit save) shows
where time goes: not in the driver (an AX press is ~10–40 ms) but in the loop
around it — a frontier LLM re-reads a 30 KB digest, picks an element, emits a
JSON tool call, and waits for the observe-after-act round-trip. Every action is
an autoregressive generation of ~100–400 tokens. TypeSafe's Jev launch note and
Cua's `jev-use` preview make the same point from the other side: when the *set
of possible next actions is bounded*, selecting one is a classification problem,
not a text-generation problem, and classifiers are 100× cheaper and faster.

We steal that framing, not the vendor:

* make the action space typed and bounded *before* the model sees it,
* let the model **select** among candidates instead of **compose** free JSON,
* prefer accessibility/semantic actions, fall back to screenshots, and use raw
  input last,
* recover from stale UI structurally (snapshot ids), not by retrying blindly,
* run several agents in parallel on isolated windows.

## 2. What exists today (baseline to build on)

`crates/computer` already has most of the type surface; it is just not
exposed as a typed *action* contract.

| Piece | Where | Notes |
|---|---|---|
| `WindowState { snapshot_id, elements[] }` | `backend.rs` | Typed elements with `index`, driver `token`, `role`, `label`, `value`, `frame`, state flags. Handles are snapshot-bound. |
| `ObserveRequest` / `ObserveScope` | `backend.rs` | frontmost / window / desktop; semantic + screenshot switches. |
| `ActionRequest { tool, args: Value, delivery, session }` | `backend.rs` | **Untyped** — `args` is raw JSON from the tool call. This is the gap. |
| `Delivery::{Background, Foreground}` | `backend.rs` | Full-Control gate already exists. |
| `verify_state` predicates | `backend.rs` (`VerifyOutcome`) | Driver vocabulary, kept as JSON. |
| 26 MCP tools (`computer_observe`, `computer_find`, `computer_click`, `computer_invoke_menu`, …) | `surface.rs` | Flat tool-per-verb surface; each carries its own ad-hoc target props (`target_props`). |
| Native macOS menu dispatch | `macos_menu.rs`, `menu.rs` | Closed-menu AX press; already the "semantic first" path for menus. |

## 3. Proposed types

All types live in **Cosmos**, not the driver. The driver keeps its own
(evolving) MCP schema; Cosmos owns the stable contract the harness sees.

### 3.1 Target — how an action names *what* it acts on

```rust
/// Ordered preference: semantic handle → text query → pixel point.
pub enum Target {
    /// Snapshot-bound element handle from `WindowState`. Cheapest, safest.
    Element { snapshot_id: String, index: u32 },
    /// Resolve by label/role inside a window at execution time
    /// (server-side `computer_find`). Survives snapshot rotation.
    Query { pid: i64, window_id: Option<i64>, text: String, role: Option<Role> },
    /// Window-local pixel point. Only valid with `Evidence::Screenshot`.
    Point { pid: i64, window_id: i64, x: f64, y: f64 },
}

pub enum Role { Button, Edit, MenuItem, Checkbox, Tab, Row, Cell, Link, Static, Other(String) }
```

### 3.2 Action — a closed enum instead of `tool: String, args: Value`

```rust
pub enum Action {
    Click       { target: Target, button: MouseButton, count: u8 },
    SetValue    { target: Target, value: String },          // AX set, no keystrokes
    TypeText    { target: Option<Target>, text: String },   // keystrokes, only if SetValue unavailable
    PressKey    { key: Key, modifiers: Modifiers },
    Scroll      { target: Target, dx: i32, dy: i32 },
    ScrollUntil { target: Target, until: Predicate, max_steps: u8 },
    Drag        { from: Target, to: Target },
    InvokeMenu  { pid: i64, path: Vec<String> },           // native closed-menu dispatch
    Focus       { pid: i64, window_id: Option<i64> },
    Launch      { app: AppRef },
    Wait        { until: Predicate, timeout_ms: u32 },
    Verify      { predicates: Vec<Predicate> },
}
```

Each variant declares the *evidence tier* it needs (below), so the API can
refuse or downgrade before the driver is called.

### 3.3 Evidence tier — accessibility first, screenshot second, raw last

```rust
pub enum Evidence {
    /// Target resolved from the AX/DOM tree. Default; background-safe.
    Semantic,
    /// Target came from pixels (screenshot grounding). Allowed only when the
    /// user enabled screenshots; still background delivery.
    Screenshot,
    /// Raw HID-level input at a point with no element under it. Requires
    /// Full-Control (`Delivery::Foreground`) and is logged as such.
    Raw,
}
```

Rule: `Target::Element | Query` ⇒ `Semantic`; `Target::Point` ⇒ `Screenshot`
if an element exists under the point, else `Raw`. The API computes this, the
model does not get to claim it.

### 3.4 Result — typed, with the fresh snapshot attached

```rust
pub struct ActionResult {
    pub status: ActionStatus,           // Applied | Refused(reason) | Stale | Failed(err)
    pub evidence: Evidence,             // what tier actually executed
    pub after: Option<WindowState>,     // fresh snapshot (new snapshot_id)
    pub verify: Option<VerifyOutcome>,  // if the action carried predicates
    pub latency: Latency,               // resolve / execute / observe in ms
}
```

### 3.5 Candidate set — the piece borrowed from `jev-use`

```rust
/// Bounded menu of typed next-actions derived from a snapshot, ranked by
/// Cosmos heuristics (role, label match to goal, distance to focus).
pub struct CandidateSet {
    pub snapshot_id: String,
    pub candidates: Vec<(CandidateId, Action)>,   // ≤ 24
}
```

A new read-only tool `computer_candidates { goal: String, pid, window_id }`
returns this. A harness may then call `computer_apply { candidate: CandidateId }`
instead of composing a full action. Nothing forces a harness to use it — the
verb tools stay.

## 4. Where the types sit

```
harness (Devin / any MCP client)
   │  MCP tools: verb tools (today) + computer_candidates / computer_apply (new)
   ▼
Cosmos Computer API          ← Action / Target / Evidence / ActionResult / CandidateSet live HERE
   │  service.rs: permission gate, evidence-tier computation, stale check,
   │              candidate ranking, session isolation, latency accounting
   ▼
ComputerBackend trait        ← `act(&ActionRequest)` becomes `act(&Action, ctx)`;
   │                           `ActionRequest.args: Value` goes away
   ▼
cua.rs (MCP client → cua-driver serve)   ← translates Action → driver tool call
```

The driver is untouched. If cua-driver later grows a typed schema of its own,
`cua.rs` maps between the two; the harness contract does not move.

## 5. Latency budget (per action, p50 target on Apple Silicon)

| Phase | Today (measured/observed) | Target | How |
|---|---|---|---|
| Model decides | 1.5–6 s (frontier LLM, ~200 tok) | ≤ 300 ms when selecting a candidate; unchanged when composing | candidate selection is a tiny output (one id) — even a frontier LLM emits it in a few tokens; a small local classifier could do it in tens of ms later |
| Resolve target | n/a (model passes index) | ≤ 20 ms | in-process lookup against cached `WindowState` |
| Execute (AX press / set value) | 10–40 ms | 10–40 ms | unchanged |
| Observe-after-act | 150–700 ms (full digest) | ≤ 120 ms | *delta* observe: re-walk only the target's window, cap `max_elements` at the previous count, skip screenshot unless the action was `Screenshot`/`Raw` |
| Verify | 30–100 ms | 30–100 ms | unchanged |
| **Total** | **~2–7 s** | **≤ 0.5 s** (select) / **≤ 1 s** (compose) | |

Success metric for a later PR: the TextEdit Save-sheet task in **≤ 15 MCP
round-trips** and **≤ 20 s wall clock** (today: 54 calls, several minutes).

## 6. Stale-UI recovery

Today staleness is a driver error string parsed by `ComputerError::from_tool_error`.
Proposed:

1. Every `Target::Element` carries its `snapshot_id`; the service compares it to
   the window's latest snapshot **before** calling the driver.
2. If stale, the service tries to re-resolve the same element by
   `(role, label, parent path)` in the fresh snapshot. If exactly one match →
   proceed and report `status: Applied, evidence: Semantic, note: "re-resolved"`.
3. If zero or many matches → return `status: Stale` with the fresh
   `WindowState` attached and a `CandidateSet` for the same goal. The harness
   gets a new bounded menu instead of a bare error.
4. Transient sheets/dialogs: `computer_candidates` always includes the
   dialog's default/cancel buttons at the top when a sheet is frontmost, so the
   "unexpected dialog" case is a one-select recovery.

## 7. Parallel agents and isolation

The driver already targets windows rather than the shared cursor, and
`ComputerBackend::session_scoped(label)` exists. Rules for a typed
implementation:

* One **lease** per `(pid, window_id)`; `computer_apply` on a leased window from
  another session returns `Refused(WindowLeased)`.
* `Evidence::Semantic` actions never take the global input lock — they can run
  concurrently across windows.
* `Evidence::Raw` (foreground) takes a process-wide lock; at most one raw action
  in flight, others queue with a timeout.
* Clipboard tools are per-session serialized (shared OS resource).
* The service exposes `computer_status.sessions[]` so a supervisor harness can
  see who holds what.

## 8. What Jev / jev-use do that Cosmos will **not** copy

Based on TypeSafe's launch post and Cua's `jev-use` preview thread (read
2026-09-20; details are theirs, not verified in code):

* **Hosted model dependency.** Jev is API-only; window state would leave the
  machine. Cosmos Computer Use stays local-first — no screen digests or DOM go
  to a third party as part of the control loop.
* **A vendor-specific decision model.** Cosmos types the *action space*; which
  model selects from it is the harness's choice (Devin today). We do not bind
  the API to Jev, OpenRouter, or any provider.
* **Cua Perception / OmniParser opt-in visual parsing.** Not adopted now; our
  screenshot tier stays "grounding for a point", not a second parser.
* **Confidence-gated auto-execution.** Jev returns calibrated probabilities; a
  later PR *may* add an optional `confidence: f32` field to `computer_apply`
  so a harness can escalate low-confidence picks to the user, but Cosmos will
  not auto-act on a confidence threshold itself.
* **Replacing the LLM loop.** Composition (what the task is, when it's done)
  stays with the harness. Cosmos only makes each step typed and cheap.

## 9. Done-when for the later implementation PR

1. `Action`, `Target`, `Evidence`, `ActionResult`, `CandidateSet` types in
   `crates/computer`, with `serde` schemas exported to the MCP `inputSchema`
   from the same source (no hand-maintained JSON in `surface.rs`).
2. `ComputerBackend::act` takes `&Action`; `cua.rs` maps to driver calls;
   `mock.rs` implements it; `zeron-computer` tests still pass.
3. New tools `computer_candidates` and `computer_apply`; existing verb tools
   unchanged in name and behaviour (backwards compatible for the mobile Watch
   Live/STOP path and the existing evidence runs).
4. Evidence tier computed server-side; `Raw` refused unless Full-Control.
5. Stale handling per §6 with a unit test for re-resolve and for the
   ambiguous → `Stale + CandidateSet` case.
6. Window leases per §7 with a two-session test on the mock backend.
7. Latency fields populated; a `computer_status` health line reports p50/p95.
8. Re-run the TextEdit Save-sheet proof through Devin → Cosmos MCP → Cua on the
   real driver: ≤ 15 round-trips, file contents byte-identical to the request.
9. `cargo fmt`, `cargo check`, `cargo test -p zeron-computer`, updater audit
   still PASS; `Cargo.lock` adds **no** new crate.
10. This document updated from "plan" to "shipped", with measured numbers
    replacing the targets in §5.

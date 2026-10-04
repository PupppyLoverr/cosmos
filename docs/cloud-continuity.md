# Cloud continuity: what Cloud mode is, and what it is not

Status: REVIEW / DESIGN. Nothing in this document is implemented by it. v0.2.0-alpha Cloud execution is roadmap-only.
Reviewed against `origin/main` @ `0d83d7f6`. Sources read: `ARCHITECTURE.md`, `docs/chat2-sync.md`, `docs/registry-sync.md`, and the cloud/session paths in `crates/` and `edge/` cited below.

Locked inputs (not reopened here): Rust on device; Cloudflare Workers + Durable Objects at the edge; session docs are Loro CRDTs relayed as opaque append-only rows, and no DO parses doc bytes; no Postgres, no Hono server, no WebRTC, no new session database; Local and Synced are different stores and signing in does not upload local sessions; BYOK is the product, and Cosmos holds no model bill.

Notation: `path:line` is a real location on `origin/main`. **NOT BUILT** means no code exists for it today.

> Doc drift: `docs/chat2-sync.md` still says `Status: PLANNED`, but the protocol is built: `edge/src/chat-room.ts` and `crates/sync/src/chat_client.rs` both exist. This review treats chat2 as shipped.

---

## 0. What exists today (the honest baseline)

Today's "Cloud mode" is a **per-chat routing flag on the laptop's engine**, not a separate execution target with its own control plane.

- A chat is "cloud" when its registry row carries a `cloud_seat` binding. `crates/ui/src/state.rs:1921` filters the sidebar with `chat.cloud_seat.is_some() == self.cloud_mode`.
- When a command for that chat drains on the **laptop's** engine, `Sessions::dispatch` sees the binding and calls `dispatch_remote` instead of spawning a local harness (`crates/engine/src/sessions.rs:673-687`, `:912`).
- The laptop engine then owns the whole turn. It builds the harness command, writes the user message into the doc, writes a run row to its own `cloud-runs.json` ledger (`crates/engine/src/sessions.rs:1028-1050`), stages credentials, and starts `drive_remote_run`, `drive_boat_run`, or `tail_remote_log` (`crates/engine/src/sessions.rs:2273`, `:2709`, `:3422`). Those tasks read seat output and write it into the Loro doc **from the laptop**.
- Seat config, secrets and the run ledger are laptop files under the device data dir, not the profile store. `CloudStore::load(data_dir)` (`crates/engine/src/lib.rs:361`) uses `cloud.json`, `cloud-keys.json` and `cloud-runs.json` (`crates/engine/src/cloud.rs:30-32`). Workspace backups go to `{data_dir}/seat-backups/` (`crates/engine/src/cloud.rs:993-996`).
- The model process does run on the seat: an SSH box, the Railway trial VM (`crates/engine/src/railway.rs:1-19`), or a Boat sandbox (`crates/engine/src/boat.rs:1-26`). But the **agent loop's control** (command intake, run state, transcript writes) runs on the laptop.

By the rule in this document ("if the loop still runs on the laptop, it is not Cloud mode"), **today's Cloud mode is remote exec driven by the laptop, not Cloud mode.** Detached SSH/Railway runs (`crates/engine/src/sessions.rs:2405-2420`) and Boat runs keep the *process* alive with the lid closed. The transcript stops advancing until the same laptop reopens and backfills it (`crates/engine/src/sessions.rs:290-420`, `resume_cloud_runs` / `reattach_cloud_run`). Detached mode is off by default for manually added seats (`crates/ui/src/cloud/seats.rs:58`).

Built pieces the design below reuses:

| Piece | Where | Role in Cloud mode |
| --- | --- | --- |
| Headless engine + service install | `apps/zeron/src/main.rs:1-3`, `apps/zeron/src/daemon.rs:313-321` (systemd `Restart=on-failure`) | The runner process already exists as a binary: `cosmos headless`. |
| Profile isolation | `crates/engine/src/profile.rs:34-70` | Local vs Synced stores. |
| ChatRoom opaque row relay | `edge/src/chat-room.ts`, `crates/sync/src/chat_client.rs` | Source of truth for the transcript and command queue. |
| Registry rows (`chat.device_id` = host) | `edge/src/registry-room.ts`, `crates/engine/src/workspace_host.rs:760-769` | Says which device executes a chat. |
| Host-only command execution + processed ledger | `crates/engine/src/doc_host.rs:10-11`, `:4738` | Only the host drains commands; it marks a command processed before executing it. |
| DeviceRoom relay | `edge/src/device-room.ts` | Laptop and phone RPC to the runner (files, terminals). |

---

## 1. Local mode

Local mode runs the harness **and** the model call on the laptop. That is correct, and it stays that way.

- **Store path:** `EngineProfile::local` sets `store_root = {data_dir}/profiles/local/` (`crates/engine/src/profile.rs:34-46`; table in `ARCHITECTURE.md` §1). Under it:
  - `docs.sqlite3`: Loro snapshots, outbox and processed-command ledger (`crates/sync/src/store.rs:79-96`, opened at `crates/engine/src/lib.rs:250`).
  - `journals/{chat_id}.jsonl`: run journal (`crates/engine/src/lib.rs:252`, `crates/engine/src/run_journal.rs:1-4`).
  - `uploads/`: attachments.
- **No edge:** `edge_enabled = false` for `WorkspaceScope::Local` (`crates/engine/src/lib.rs:826-827`). No ChatRoom, no RegistryRoom, no DeviceRoom.
- **Lid closes:** the harness child process and the engine stop, so the loop dies. On the next boot, `RunJournal` finds the stale journal and stamps the streaming entry `aborted` (`crates/engine/src/sessions.rs:11`). **This is accepted behaviour. Local mode makes no continuity promise.**
- **Sign-in does not upload local sessions.** The profile is fixed at startup (`ARCHITECTURE.md` §1). The only local→synced path is the explicit, user-chosen import in the sync wizard: `crates/ui/src/shell.rs:6170-6173` runs `spawn_local_import` only when the user picked import, through `crates/engine/src/local_import.rs`. Default sign-in copies nothing.

---

## 2. Cloud mode is a different machine

**Definition.** A Cloud chat is a synced chat whose **host device** (`chat.device_id` in the registry) is a **runner** engine on a machine other than the laptop. The runner drains the chat's command queue, runs the harness, and writes the transcript. The laptop and phone are viewports. If the laptop's engine is the one draining commands, the chat is not a Cloud chat, whatever folder or seat badge it has.

Most of this already falls out of the existing host model: `WorkspaceHost::is_host` gates command execution on `chat.device_id == self.device_id` (`crates/engine/src/workspace_host.rs:760-769`). Cloud mode = **create the chat with `device_id = <runner device id>`** and run a headless engine on the runner. The missing pieces are listed as NOT BUILT below.

### 2.1 What stays running when the laptop is off

| Item | Answer | Status |
| --- | --- | --- |
| Runner process | `cosmos headless`, signed into the **same synced profile** (`{data_dir}/orgs/{org}/{user}/` on the runner, `crates/engine/src/profile.rs:48-70`), installed as a service (`apps/zeron/src/daemon.rs:313-321`). It joins RegistryRoom, joins ChatRoom for chats it hosts, and connects outbound to its DeviceRoom. | Binary and service BUILT. Running it as a *Cloud seat* (seat ↔ runner device binding, `device_id` assignment at chat creation, seat bootstrap that installs and logs in `cosmos headless`): **NOT BUILT** |
| Where | A machine the user controls: their own VPS (SSH seat), their Boat sandbox, or a claimed Railway box. Not the Railway 60-minute trial VM: it expires (`crates/engine/src/railway.rs:1-3`, `crates/engine/src/sessions.rs:3612-3636`) and is only a demo or throwaway target. | **NOT BUILT** (bootstrap) |
| Who pays for the VM | **The user**: their VPS bill, their Boat account (`boat_…` token, `crates/engine/src/boat.rs:4-5`), or their Railway account (their own SSH key is the credential, `crates/engine/src/railway.rs:2-3`). A later **funded** Cosmos runner pool is possible but is not the default and is not designed here. There is **never a Cosmos-paid default VM.** | Policy. No Cosmos-paid VM code exists, and none must be added. |
| Who pays for the model | The user (§2.4). | Policy |

What does **not** stay running: anything on the laptop. That includes today's `drive_remote_run` / `tail_remote_log` / `mirror_boat_events` mirroring, `cloud::Scheduler` automations (`crates/engine/src/lib.rs:364-370`, `crates/engine/src/cloud.rs:3894`), and the `cloud-runs.json` ledger. In Cloud mode all of this moves to the runner, or is not needed because the runner hosts the chat directly.

### 2.2 What the laptop is when it reopens: a viewport

The reopened laptop is **not** the host of a Cloud chat. It does not resume, re-dispatch or re-mirror anything.

Catch-up uses the existing chat2 protocol, and nothing else:

1. **Instant paint** from the disposable cache: the warm snapshot plus cursor in the synced profile's `docs.sqlite3` (`crates/sync/src/store.rs:79-96`, `{data_dir}/orgs/{org}/{user}/`).
2. **`hello{cursor}` → `state{seqFloor, headSeq, checkpointSeq, checkpointFrontier, checkpointSize}`**, metadata only (`docs/chat2-sync.md`; `edge/src/chat-room.ts:392-394`).
3. If the local doc already includes the checkpoint frontier → **rows only**: `rows{after: cursor}`, the Loro tail. On reconnect, `excludeDevice` skips rows this device itself wrote (`crates/sync/src/chat_client.rs:1143`).
4. If the gap is below `seqFloor` (the room compacted past our cursor) → **`GET /chat2/{chatId}/checkpoint` with `Range: bytes=N-`** (206, resumable after a lid-close mid-download; `edge/src/chat-room.ts:150-172`, `crates/sync/src/chat_client.rs:155-156`), then rows after `checkpointSeq`.
5. The sidebar row, title, status and preview come from RegistryRoom cursor deltas (`docs/registry-sync.md`). The tail sidecar (`PUT /chat2/{id}/tail`, `crates/engine/src/doc_host.rs:2741`) is published **by the host**, which is now the runner.

Explicitly **not**:
- no full re-upload of the laptop's doc (the laptop wrote nothing new; any unacked outbox rows are just its own pending commands, pushed once with their batch IDs, which the room deduplicates);
- no poll of a local JSONL: the laptop's `journals/` never saw the Cloud turn;
- no `seat_exec` log tail and no `cloud-runs.json` reattach. Today's `resume_cloud_runs` (`crates/engine/src/sessions.rs:294`) must **not run** for runner-hosted chats. **NOT BUILT:** the gate that skips them.

### 2.3 Session identity: one chat id, two execution targets

- A chat has **one id**. Its execution target is the registry row's `device_id`: the laptop (local execution in the synced profile) or the runner (Cloud). The `cloud_seat` binding (`crates/proto` `CloudSeatBinding`, written via `WorkspaceHost::rebind_chat_seat`, `crates/engine/src/workspace_host.rs:919-930`) becomes a description of the runner, not a routing switch on the laptop.
- **Cloud chats exist only in the synced profile.** Their doc lives in that profile's ChatRoom `chat2/{chatId}` (`edge/src/chat-room.ts`). Their row lives in RegistryRoom `reg1/{org}/{user}` (`docs/registry-sync.md`).
- **The DO is the source of truth for Cloud chats.** Every device's `docs.sqlite3` copy is a **cache**: `crates/sync/src/store.rs:81` already says "the DO room is authoritative". It can be deleted and rebuilt from checkpoint + rows. Nothing on the laptop is the only copy of a Cloud chat.
- **Cloud chats are not rows in the Local store.** Today nothing stops a Local-profile engine from creating a chat with a `cloud_seat` binding: `MutateParams::CreateChat` passes `cloud_seat` through with no scope check (`crates/engine/src/rpc.rs:1252-1273`). **NOT BUILT:** a refusal that Cloud chat creation requires `WorkspaceScope::Synced`, plus a UI that hides the Cloud toggle (or sends the user to sign-in) in the Local profile.
- **Local-only chats never enter ChatRoom.** This is already enforced by construction: the Local profile has `edge_enabled = false` (`crates/engine/src/lib.rs:827`), and the only bridge is the explicit, user-chosen import (§1).

### 2.4 The model call: one turn with the lid closed

Target trace (all **NOT BUILT** unless marked):

1. Phone or laptop appends a `QueueCommand{send}` to the chat's Loro doc → opaque rows → ChatRoom (BUILT: `ARCHITECTURE.md` command plane, `edge/src/chat-room.ts`). The laptop can be closed right after.
2. The runner is the registry host for the chat. Its `DocHost` drains the command, marks it processed **before** executing (BUILT: `crates/engine/src/doc_host.rs:10-11`), and calls the ordinary **local** `Sessions` path on the runner. There is no `dispatch_remote` hop, because the runner is local to itself.
3. **Key.** The runner reads only credentials **sealed to this runner's device key**: a per-runner encrypted blob the user created on purpose, from the laptop, for this runner. Its only plaintext copy is in runner memory or the runner's own credential store. **NOT BUILT** (no sealing code exists today: `rg seal` finds nothing in `crates/engine/src/cloud.rs` or `crates/proto/src/cloud.rs`). The two allowed sources are:
   - a BYOK key the user explicitly sealed for this runner (successor to today's "use this key for Cloud sessions" toggle, `crates/engine/src/cloud.rs:733-768`);
   - a subscription plugin the user already pays for and logged into for this runner: Codex (`~/.codex/auth.json`), Claude Code subscription, Agent ACP, Devin (`ByokProvider::DevinSub`, `crates/proto/src/cloud.rs:150-151`).
4. **Allowlisted host.** The harness may send the key only to the base URL bound to the sealed key: the provider default (`ByokProvider::default_base_url`, used at `crates/engine/src/cloud.rs:2160-2162`), or the custom `base_url` the user typed when sealing (`crates/engine/src/cloud.rs:283`). The same `https`, DNS-only, no-userinfo/port/query rules as `agent_accounts::trusted_base` apply (`crates/engine/src/agent_accounts/stores.rs:683-700`). **NOT BUILT** for runner credentials.
5. The runner streams transcript rows into ChatRoom itself (BUILT path: `DocHost` → chat2 client) and publishes the tail sidecar. The laptop is not involved.
6. **Sealed key missing, expired or revoked:** the runner marks the command failed with a visible transcript line ("No key sealed for this runner — seal one from a signed-in device"). It does not start a harness. It **never** falls back to:
   - a Cosmos-owned key (none exists; none may be added);
   - another runner's key;
   - live harvesting from the laptop. Today's `claude_oauth_token()` reads `~/.claude/.credentials.json` from the laptop at dispatch time (`crates/engine/src/cloud.rs:1212-1230`), and `harness_credentials` / `stage_credential` copy CLI logins per run (`crates/engine/src/cloud.rs:1238-1256`, `:1340`; `crates/engine/src/sessions.rs:2380-2402`). Both need the laptop awake, so they are invalid for Cloud mode by definition.

Today's behaviour to retire (current facts, not design): keys sit in plaintext JSON in `cloud-keys.json` on the laptop (`crates/engine/src/boat.rs:4-5`, `crates/engine/src/cloud.rs:31`). They are exported as shell env to the seat on every run (`crates/engine/src/sessions.rs:2325-2336`). Logins are staged to the seat and deleted again only on Railway and Boat (`crates/engine/src/cloud.rs:1406-1415`). `ChatgptSub` is exported as `OPENAI_API_KEY` (`crates/engine/src/cloud.rs:755`). Boat runs always call the box's integrated `claude` agent with `model: None` (`crates/engine/src/sessions.rs:2977`), which ignores the composer's harness and model.

### 2.5 Failure cases

| Case | Authoritative state | Behaviour | Status |
| --- | --- | --- | --- |
| **Runner dies mid-tool** (OOM, VM reboot) | ChatRoom rows up to the last flush (`DO_FLUSH_MS=5s`, `ARCHITECTURE.md` §2). Runner's own `journals/` on its disk. | systemd restarts `cosmos headless` (`apps/zeron/src/daemon.rs:321`). On boot the runner's stale-journal scan stamps the streaming entry `aborted` (`crates/engine/src/sessions.rs:11`, BUILT for local runs). The command stays processed, so it is **not** re-executed automatically: a tool call with side effects (push, deploy, rm) must never replay blind. The user sees "runner restarted mid-turn" and sends again. Harness-native resume (`claude --resume`, `codex resume`) may continue the thread (BUILT idea: `crates/engine/src/cloud.rs` `continuation_flags`, used at `crates/engine/src/sessions.rs:984-987`). | Restart + abort: BUILT on the runner because it is the normal engine. Cloud-specific "runner restarted" note: **NOT BUILT** |
| **VM destroyed / Railway trial expires** | ChatRoom transcript survives (DO + nightly R2). Workspace files on the VM are gone. | The transcript is intact. The workspace is gone unless the user's repo was pushed. Today's laptop-side `seat-backups/` (`crates/engine/src/cloud.rs:993-996`) does not apply to a runner. A runner-side workspace snapshot to R2 would be a blob next to ChatRoom, not a database. | **NOT BUILT** |
| **Laptop opens during an active turn** | ChatRoom | Laptop catches up per §2.2 and then receives live rows over the WebSocket. It does not take over, re-dispatch, or tail a log. Stop / steer / answer-input are just `QueueCommand` entries the runner drains. | Catch-up and live rows BUILT. Laptop-side "don't resume runner-hosted chats" gate: **NOT BUILT** |
| **Two viewports attach** (laptop + phone) | ChatRoom; RegistryRoom presence is ephemeral | Both see the same rows. Both can append commands, and Loro merges them. Only the host (runner) executes them, once, through the processed ledger (`crates/engine/src/doc_host.rs:2057`, `:2935`). Conflicting commands (two sends) queue in order; dedupe/supersede rules apply (`ARCHITECTURE.md` §2 ledger rules 1–3). | BUILT (the same as any synced chat) |
| **Network drop on the runner** | Runner's local outbox in its `docs.sqlite3` | Rows queue locally and push on reconnect, deduplicated by batch ID (`docs/chat2-sync.md`). The turn keeps running offline. | BUILT |
| **Two runners claim one chat** | Registry `device_id` (LWW) | Only the row's `device_id` executes (`crates/engine/src/workspace_host.rs:760-769`). Moving a chat between runners is an explicit rebind, refused while a turn runs (today's `CLOUD_MOVE_CHAT` guard, `crates/engine/src/rpc.rs:4214-4218`). | Guard BUILT for seats. Runner-to-runner rebind: **NOT BUILT** |
| **User switches a Cloud chat back to Local** ("return to this Mac") | See below | See below | **NOT BUILT** (no Cloud→local path exists; `CLOUD_MOVE_CHAT`, `crates/engine/src/rpc.rs:4197-4250`, only moves seat→seat) |

#### "Return to this Mac": what we copy and what we refuse

Switching a Cloud chat to "run here" means **rebinding the registry row's `device_id` from the runner to the laptop, inside the synced profile.** The chat keeps its id and stays in ChatRoom. It becomes an ordinary synced chat hosted by the laptop. It never moves into the Local profile; that would break §2.3. The rebind is refused while the runner has a turn running, the same way the `CLOUD_MOVE_CHAT` guard refuses today.

| Thing | Copy? | Why |
| --- | --- | --- |
| Transcript + command history | **No copy needed.** Same ChatRoom doc, now hosted by the laptop. | One chat id, one source of truth. |
| Registry row (title, config, harness/model pick) | Rewritten in place (`device_id`, `cloud_seat` cleared) | LWW row update, not a copy. |
| Pending, unprocessed commands | Left in the doc. The new host drains them. | They are durable doc entries. |
| Committed + **pushed** git state | Laptop checks out the branch from the remote. | Git is the transport for code. |
| **Uncommitted** runner workspace edits | **Refuse silently. Offer instead:** the runner commits them to a WIP branch and pushes, and the laptop checks it out. If the user declines, they stay on the runner. | No silent file sync between machines. A diff the user did not see is not copied. |
| Live harness process / mid-turn state | **Refuse.** Switching is blocked until the turn ends or is stopped. | A process cannot move between machines. |
| Harness-native session (Claude Code / Codex conversation files) | **Refuse.** The laptop starts the harness fresh with a transcript recap (today's `prior_turns_recap`, `crates/engine/src/sessions.rs:992`). | Those files hold runner paths and sometimes credentials. |
| Sealed keys / subscription logins | **Refuse, both directions.** Sealed to one device and never re-exported. The laptop uses its own logins. | BYOK rule: a credential lives where the user put it. |
| Runner's `journals/`, run ledger, terminals, previews | **Refuse.** They are runner-local diagnostics. | The laptop's `journals/` starts at its first turn. |
| Attachments | Already blobs referenced by the doc. The laptop fetches them on demand. | No bulk copy. |

The reverse ("send this chat to the cloud") is the same rebind in the other direction, with the same refusals: push first, no credential copy, no process copy. A **Local-profile** chat cannot be sent to the cloud without the explicit local→synced import (§1). Signing in never does it implicitly.

---

## 3. NOT BUILT: the full gap list

1. Seat ↔ runner device binding; Cloud chat creation sets `device_id = runner` (`crates/engine/src/rpc.rs:1252-1273` today passes the laptop's or the caller's `device_id`).
2. Seat bootstrap: install `cosmos headless`, sign the runner into the user's synced profile (device-flow login on the runner, `apps/zeron/src/auth_cli.rs`), and `cosmos daemon install`.
3. Scope gate: refuse `cloud_seat` / Cloud chats in `WorkspaceScope::Local`.
4. Laptop gate: `resume_cloud_runs` and `dispatch_remote` must not touch runner-hosted chats. `cloud-runs.json` mirroring is retired for them.
5. Credential sealing to a runner device key, the per-key host allowlist, and the missing-key hard failure. Live laptop credential harvesting (`claude_oauth_token`, `stage_credential`) is retired for Cloud chats.
6. Runner-side workspace snapshots to R2 (a blob next to ChatRoom; no new database).
7. Cloud→local and local→cloud rebind with the copy/refuse table above.
8. Boat: honour the composer's harness and model (today it is hard-wired to `claude`, `crates/engine/src/sessions.rs:2977`), or run `cosmos headless` inside the box.
9. Docs: flip `docs/chat2-sync.md` status from PLANNED.

No item needs a new database, a server outside Workers/DOs, or a Cosmos-paid VM or model key.

---

## 4. Summary table

| | Where the loop runs | Source of truth | What is on disk | What happens when the lid closes |
| --- | --- | --- | --- | --- |
| **local chat** | Laptop engine spawns the harness locally (local path in `crates/engine/src/sessions.rs` after the `cloud_seat` check at `:676`) | Laptop's Local store (`crates/engine/src/profile.rs:34-46`); no edge (`crates/engine/src/lib.rs:827`) | `{data_dir}/profiles/local/docs.sqlite3` + `journals/` + `uploads/` (`ARCHITECTURE.md` §1; `crates/sync/src/store.rs:79-96`; `crates/engine/src/lib.rs:250-252`) | Loop dies; the next boot stamps the turn aborted from the stale journal (`crates/engine/src/run_journal.rs:1-4`, `crates/engine/src/sessions.rs:11`). Accepted. |
| **synced chat** | Host device = registry `device_id`, normally the laptop (`crates/engine/src/workspace_host.rs:760-769`) | ChatRoom DO `chat2/{chatId}` (`edge/src/chat-room.ts`) + RegistryRoom (`edge/src/registry-room.ts`); local copy is cache (`crates/sync/src/store.rs:81`) | Cache: `{data_dir}/orgs/{org}/{user}/docs.sqlite3` + `journals/` (`crates/engine/src/profile.rs:48-70`) | Loop dies if the laptop was the host. Phone sees the transcript up to the last flush and can queue commands, but nothing executes until the host returns (`crates/engine/src/doc_host.rs:4738`). |
| **cloud chat** | Runner `cosmos headless` on a user-paid VM as the registry host: **NOT BUILT** (binary exists: `apps/zeron/src/main.rs:1`, `apps/zeron/src/daemon.rs:313-321`). Today: laptop drives a seat process (`crates/engine/src/sessions.rs:912`). | ChatRoom DO (`edge/src/chat-room.ts`) for the transcript. Run state on the runner: **NOT BUILT**. Today: laptop `cloud-runs.json` (`crates/engine/src/cloud.rs:32`). | Laptop: disposable cache in the synced profile (`crates/engine/src/profile.rs:48-70`). Runner: its own synced store + sealed keys: **NOT BUILT**. Today: laptop `cloud.json` / `cloud-keys.json` / `seat-backups/` (`crates/engine/src/cloud.rs:30-31`, `:993-996`). | Target: loop keeps running on the runner; the laptop is a viewport on reopen via the Loro tail + Range checkpoint (`edge/src/chat-room.ts:150-172`): **NOT BUILT**. Today: detached/Boat process survives, the transcript freezes until the laptop backfills it (`crates/engine/src/sessions.rs:294-420`); non-detached runs die (`crates/engine/src/sessions.rs:373`). |

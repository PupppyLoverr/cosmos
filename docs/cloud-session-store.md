# Cloud session store: what is the source of truth for a cloud chat?

Status: DESIGN ANSWER. Nothing is implemented by this document, and no dependency is added.
Checked against `origin/main` @ `0d83d7f6`. Companion to `docs/cloud-continuity.md`.

## Question

Cloud chats are cached on the laptop. Is that the right source of truth?

## Short answer

No. A cloud chat's source of truth must be its ChatRoom row log. Today that holds only when the engine runs in the **Synced** profile. In the **Local** profile, a cloud chat's only copy is on the laptop. Separately, in **both** profiles, a cloud chat's run state, seat config and keys exist only on the laptop. Both are design bugs, not features.

## The two stores, confirmed

| Profile | Store root | Edge | Code |
| --- | --- | --- | --- |
| Local | `{data_dir}/profiles/local/` (`docs.sqlite3`, `journals/`, `uploads/`) | none (`edge_enabled = false`) | `crates/engine/src/profile.rs:34-46`; `crates/engine/src/lib.rs:250-252`, `:826-827` |
| Synced | `{data_dir}/orgs/{org}/{user}/` (same layout) **plus** ChatRoom DO `chat2/{chatId}` and RegistryRoom | on; `DocHost` joins chat2 when `config.edge.is_some()` | `crates/engine/src/profile.rs:48-70`; `crates/engine/src/lib.rs:858-860`; `crates/engine/src/doc_host.rs:1519`; `edge/src/chat-room.ts` |

In Synced, the local `docs.sqlite3` is already declared a cache: "the DO room is authoritative" (`crates/sync/src/store.rs:81`). That is correct, and it stays that way.

## Where the design is wrong today

**Bug 1: a cloud chat can be created in the Local profile, where its only copy is on the laptop.**
- The Cloud/Agent toggle is rendered in the sidebar regardless of profile (`crates/ui/src/shell.rs:9120` → `render_cloud_mode_toggle`, `:8365`; `set_cloud_mode`, `:8339-8363`, has no scope check).
- A new chat in Cloud mode sends `createChat` with `cloudSeat` (`crates/ui/src/composer.rs:10400-10406`). The CloudRail's PR-review chat does the same (`crates/ui/src/cloud.rs:1843-1851`), and side chats inherit it (`crates/ui/src/side_chat.rs:55-57`).
- The engine accepts it with no scope check (`crates/engine/src/rpc.rs:1252-1273` → `WorkspaceHost::create_chat_with_parent`, `crates/engine/src/workspace_host.rs:932-…`), even though `EngineRpc` knows its `workspace_scope` (`crates/engine/src/rpc.rs:774`).
- With edge disabled, the row lands in the Local registry and the Loro body in `{data_dir}/profiles/local/docs.sqlite3`. That is the **only copy**.

**Bug 2: cloud run state is laptop-only in every profile.**
- `CloudStore::load(data_dir)` uses the **device root**, not the profile store (`crates/engine/src/lib.rs:361`). It holds `cloud.json`, `cloud-keys.json` and `cloud-runs.json` (`crates/engine/src/cloud.rs:30-32`), and workspace backups go to `{data_dir}/seat-backups/` (`crates/engine/src/cloud.rs:993-996`).
- These files sit outside both profiles, so they are shared by Local and Synced and never synced. The run ledger that `resume_cloud_runs` relies on (`crates/engine/src/sessions.rs:294`) dies with the laptop.

**Bug 3: promotion happens as a side effect, not an explicit copy.**
- `CloudMoveChat` overwrites `cloudSeat` on **any** existing chat, including one that was never a cloud chat (`crates/engine/src/rpc.rs:4197-4250` → `crates/doc/src/registry.rs:1085-1100`).
- Re-sending `createChat` with `cloudSeat` for an existing chat stamps it with `set_chat_cloud_seat_if_unset` (`crates/engine/src/workspace_host.rs:942-945`, `crates/doc/src/registry.rs:1058-1080`).
- Either one turns a chat into a cloud chat in place, with no copy and no change of store.

Not bugs: `set_cloud_mode` only switches the view, and a mode toggle never rewrites a chat (`crates/ui/src/shell.rs:8339-8363`). Moving local work into Synced already exists as an explicit, opt-in import (`crates/ui/src/shell.rs:6170-6173`, `crates/engine/src/local_import.rs`).

**Attachments, as they are today:** they live only in the host device's account-scoped uploads root, and "nothing is mirrored to the edge" (`crates/engine/src/uploads.rs:1-11`; root from `crates/engine/src/profile.rs:40`, `:65`). The only R2 path is the tool-output sidecar `PUT/GET /blob/{chatId}/{partId}` into the existing `BLOBS` bucket, size-capped by `MAX_TOOL_BLOB_BYTES` (`edge/src/index.ts:388-420`; writer `crates/engine/src/doc_host.rs:4618-4636`).

## The rule (implement this)

A cloud chat exists only in the Synced profile. Its source of truth is its ChatRoom row log (`chat2/{chatId}`: opaque, append-only Loro rows plus client-built checkpoints, never parsed by the DO), and its sidebar row is in RegistryRoom. Every device, including the laptop that created it, holds only a rebuildable cache in `{data_dir}/orgs/{org}/{user}/docs.sqlite3`. Deleting that cache loses nothing, because it can be rebuilt from checkpoint + rows. Cloud run state (the run ledger, seat binding and workspace backup pointers) is recorded either in the chat's own Loro doc or in the registry row, so it rides the same room. Laptop-only `{data_dir}/cloud-*.json` files must never be the only record of a cloud chat. Sealed keys stay on the device they were sealed to and are never synced. Attachments stay in the profile's `uploads_root`. When, and only when, the profile is Synced, they are also written to the existing `BLOBS` R2 bucket through the existing `/blob/{chatId}/{partId}` route under an attachment part-id prefix, with no new bucket or store. Local-profile chats never reach any edge room or R2, so a Local-scope engine rejects `cloudSeat` on `createChat` and on `CloudMoveChat`, and the UI hides Cloud mode in the Local profile (or routes to sign-in). Turning a local chat into a cloud chat is an explicit copy the user starts: a new chat id in the Synced profile, seeded from the source chat's entries through the existing local-import write path. It is never done by toggling a mode, stamping `cloudSeat` on an existing row, or moving seats. `CloudMoveChat` may only rebind a chat that is already a cloud chat. There is no second database: no Supabase/Firebase/Convex, and no parsed-transcript tables.

## Files that would change

| File | Change |
| --- | --- |
| `crates/engine/src/rpc.rs` | `CreateChat`: reject `cloudSeat` when `workspace_scope == Local`. `CLOUD_MOVE_CHAT`: reject when scope is Local or the chat has no existing `cloud_seat`. New explicit "copy chat to cloud" method handler. |
| `crates/rpc/src/lib.rs` | Method constant for the explicit copy. |
| `crates/engine/src/workspace_host.rs` | `create_chat_with_parent`: drop the `set_chat_cloud_seat_if_unset` stamp on an already-existing chat (`:942-945`). |
| `crates/doc/src/registry.rs` | Remove `set_chat_cloud_seat_if_unset`, or limit it to the create transaction. `set_chat_cloud_seat` refuses a row with no prior seat. |
| `crates/engine/src/local_import.rs` | Per-chat copy into the Synced profile under a new chat id (reuses the existing import write path; source chat untouched). |
| `crates/engine/src/lib.rs` | Construct `CloudStore` from the Synced profile only, rooted at `profile.store_root()`; no `CloudStore` in Local (`:361-370`). |
| `crates/engine/src/cloud.rs` | Run ledger and backup pointers move out of `cloud-runs.json` / `seat-backups/` into the chat doc / registry row. `cloud-keys.json` stays device-local (sealed keys). |
| `crates/engine/src/sessions.rs` | `dispatch_remote` / `resume_cloud_runs` read and write run state through the doc/registry instead of `CloudStore` files; refuse in Local scope. |
| `crates/engine/src/uploads.rs` | In Synced scope, mirror committed attachments to `/blob/{chatId}/{attachment part id}`; resolve misses from R2 before proxying to the host. |
| `crates/engine/src/doc_host.rs` | Attachment upload helper beside `upload_tool_sidecar` (`:4618-4636`). |
| `edge/src/index.ts` | Allow an attachment part-id prefix on the existing `/blob` route with its own size cap (the `MAX_TOOL_BLOB_BYTES` cap is for tool output). Same `BLOBS` bucket, same per-user key prefix. |
| `crates/ui/src/shell.rs` | Hide or disable the Cloud toggle when `workspace_scope` is Local, with a sign-in route (`:8339-8420`, `:9120`). Add an explicit "Copy to Cloud" action. |
| `crates/ui/src/composer.rs` | Never attach `cloudSeat` in Local scope (`:10400-10406`). |
| `crates/ui/src/cloud.rs` | Seat move only for chats that already have a seat (`:1424`). The PR-review chat is created only in Synced (`:1843-1851`). |

No dependency, no new bucket, no new database, no DO schema change (ChatRoom still stores opaque rows).

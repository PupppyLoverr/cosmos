# Selective upstream port: v0.2.77 → v0.2.79

This merge ports the upstream release selectively onto the Cosmos branch. Local
Computer Use routing, updater/branding behavior, and the iOS visual structure
remain authoritative.

## Upstream changes

| Upstream PR | Area | Decision | Notes |
| --- | --- | --- | --- |
| #90 | Pinned sidebar and session synchronization | Taken | Kept the protocol, registry, workspace, and model synchronization changes. |
| #454 | Cursor authentication and steering | Taken | Kept interrupted-context preservation and combined it with the local busy-run drain and steering behavior. |
| #455 | Compact sidebar and project icons | Taken selectively | Kept the icon assets and desktop/UI support that merge cleanly without replacing Cosmos-specific presentation. |

## Release chores

| Chore | Decision | Reason |
| --- | --- | --- |
| Cargo version bump to `0.2.79` | Taken | Required by the upstream release tag and explicitly allowed for this port. |
| Upstream release workflow and supporting research/test fixtures | Taken | These are part of the selected upstream release and do not alter Cosmos updater branding. |
| iOS pinned-session model and sync methods | Taken | They fit the local `AppModel`, `WorkspaceStore`, `RegistryCore`, and pinned-session model. |
| iOS pinned-session visual/sidebar replacement | Skipped | The local re-themed app uses shared `SessionList` views; upstream `List` replacements were not applied. |
| Upstream HomeView/SpaceView row layout | Skipped | Replacing the local `SessionList` structure would discard the Cosmos mobile visual treatment. |
| Upstream updater/branding changes | Skipped | Local Cosmos protections remain authoritative; upstream updater hosts, release endpoints, update toggles, and internal bundle naming were not reintroduced. |
| `apps/mobile` changes | Skipped | The port scope explicitly excludes mobile, and upstream has no mobile changes required here. |

# Cosmos for Android

A native Kotlin + Jetpack Compose companion for Cosmos — the Android counterpart
to `apps/ios`. The phone is a **remote companion**: it joins your account's
edge and mirrors your desktop/cloud agent sessions. No engine runs on the phone.

It speaks the **same protocol** the desktop and iOS clients use:

- **Registry row-table** (`GET /registry/{org}/rows`, `POST /registry/{org}/push`)
  — plain JSON LWW rows (`crates/doc/src/registry.rs`, `edge/src/registry-room.ts`).
  Drives the session list, live status, spaces and device presence. Implemented
  natively in Kotlin (`net/CosmosClient.kt`), no Loro needed.
- **chat2 session doc** (`/chat2/{id}/rows`, `/checkpoint`) — an append-only log
  of opaque **Loro** update blobs. Materializing the full transcript and
  producing durable `run`/`interrupt` commands needs a Loro engine; that is the
  optional native bridge (a UniFFI wrapper over `crates/sync` + `crates/doc`),
  behind `net/ChatBridge.kt`. Until it is bundled, the session view shows live
  status + the latest message from the registry and routes a send as a host
  nudge (clearly surfaced as non-durable).

## Vertical slice

`CONNECT → SESSION LIST → SESSION → SEND → STREAM/REFRESH → STOP`. Connect,
list, state and refresh run against the real registry protocol today; send/stop
and full transcript streaming land with the native Loro bridge.

## Design

Cosmos visual language, ported from `crates/ui/src/theme.rs` (see
`theme/Color.kt`): always-dark monochrome surfaces (#060606 / #0d0d0d), indigo
accent, Geist / Geist Mono, restrained spacing, status dots (pink working,
emerald done, amber awaiting, red error). Native Android navigation.

## Build

```sh
cd apps/android
# local.properties must point at your SDK (sdk.dir=…); the CI/dev machine used
#   ~/android-sdk with NDK 26.x, platform 34, build-tools 34.0.0.
./gradlew :app:assembleDebug        # -> app/build/outputs/apk/debug/app-debug.apk
./gradlew :app:assembleRelease      # debug-signed preview APK (no prod keystore)
./gradlew :app:testDebugUnitTest    # JVM unit tests
```

Requires JDK 17+ and Android SDK (compileSdk 34). `applicationId`
`sh.cosmos.companion` (debug builds suffix `.debug`).

## Connect (dev mode)

Point **Edge URL** at an `AUTH_MODE=dev` edge. From the Android emulator the
host machine is `http://10.0.2.2:<port>` (e.g. `wrangler dev --var AUTH_MODE:dev`
on `:27640`). Enter a **User ID** and **Org ID**; the dev bearer is `user@org`,
the same identity the desktop signs in with, so sessions synced by a desktop on
the same account appear on the phone.

Production Play Store signing remains a follow-up: release builds are signed
with the debug key for preview installs only.

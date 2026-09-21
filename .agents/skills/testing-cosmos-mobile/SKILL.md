---
name: testing-cosmos-mobile
description: How to run and E2E-test the Cosmos Expo mobile companion app (apps/mobile) in the iOS Simulator against a locally running Cosmos desktop engine — pairing, sessions, Watch Live, appearance.
---

# Testing the Cosmos mobile app (apps/mobile) on the iOS Simulator

## Services
- The phone pairs to the desktop engine's IPC port (default `27654`). Confirm with
  `lsof -nP -iTCP:27654 -sTCP:LISTEN` — the listener is the `zeron` binary inside
  `target/package/Cosmos.app`. Never restart it mid-test; the phone loses its stream.
- The simulator shares the Mac loopback, so LOCAL NETWORK pairing works with `127.0.0.1:27654`
  (the host field accepts `host` or `host:port`).
- Start Metro in the background: `cd apps/mobile && npx expo start --ios > /tmp/metro.log 2>&1 &`.
  It auto-opens Expo Go on the booted sim. Reload JS without a TTY via
  `curl -X POST http://localhost:8081/reload`.

## Useful UI paths
- Desktops tab → long-press a linked row → Unlink (to re-test pairing from scratch).
- Desktops tab → "Link a desktop on this network" → host, name → Check connection → Trust & link.
- Sessions tab header "+" → Harness / Model rows open native Alert lists; long lists must be
  dragged (not mouse-wheel scrolled) to reach later models.
- Session screen header "display" icon = Watch Live; it only appears when Computer Use is
  available on the desktop or recent transcript entries contain computer tool calls.
- Settings → Appearance segmented control (System / Light / Dark).

## Gotchas
- Routes presented as `formSheet` (react-native-screens 4.26 + iOS 27 sim + Expo Go) may render
  completely blank (only grabber/title). If a sheet is blank, try `presentation: 'modal'`.
- Full-res evidence: `xcrun simctl io <udid> screenshot file.png` (the scaled desktop screenshot
  is too small to read).
- The Name TextInput autocorrects ("This Mac" → "This Tac"); cmd+a, backspace and retype.
- In dark mode the accent color is near-white, so filled accent buttons with `#fff` labels
  (e.g. "Trust & link") can appear label-less — zoom before assuming the button is missing.
- Sign-in uses Google via Woozlit; persisted account state lives in Expo Go's SecureStore.
  If signed out and Google asks for password/2FA, document and use LOCAL pairing instead.
- While an account is persisted, every `/sign-in` entry point is hidden. To render the screen
  without signing out (you can't sign back in), deep-link it:
  `xcrun simctl openurl <udid> "exp://127.0.0.1:8081/--/sign-in"`.
- After killing/restarting Metro, `POST /reload` may return 200 while Expo Go keeps the stale
  bundle — reopen with `xcrun simctl openurl <udid> exp://127.0.0.1:8081`, or use the Simulator
  Device → Shake (ctrl+cmd+z) → Reload, which is also visible in recordings. Confirm a reload
  happened via a fresh "iOS Bundled" line in the Metro log.
- Reanimated 4 needs `react-native-worklets` as an explicit dependency; a missing lock entry
  shows up as a red "Unable to resolve module react-native-worklets" screen in Expo Go.
- Mouse-wheel does not scroll RN ScrollViews in the sim; use drags. The Expo Go floating dev
  bubble sits bottom-right and steals taps (e.g. Watch Live "Pause") — drag it elsewhere first.
- Photo picker: seed the sim library with `xcrun simctl addmedia <udid> file.jpg` (use `sips` to
  convert if PIL is unavailable). Prefer atomic `left_click_drag` for slider tests; low-level
  `left_mouse_down`/`mouse_move`/`left_mouse_up` sequences can leave the button stuck and fire
  stray taps (creating sessions / changing prefs). If input misbehaves, send `left_mouse_up` and
  re-click the Simulator titlebar to refocus (Cosmos.app can steal focus).
- Compare wallpaper effects at full res: `sips -c 400 400 --cropOffset X Y shot.png --out crop.png`
  on the `simctl` screenshots — the scaled desktop capture hides halftone/scanline detail.

## Native dev-client build (recorded runs; preferred over Expo Go)
- Build + install + Metro in one shot:
  `cd apps/mobile && LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 npx expo run:ios --device <udid>`
  (`ios/` is CNG-gitignored; run:ios does prebuild + pods itself). Keep the process alive —
  it owns Metro.
- `pod install` crashes on CocoaPods 1.17 + Ruby 4 with a UnicodeNormalize ASCII-8BIT error;
  always prefix `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8`.
- If xcodebuild reports "Unable to find a destination matching …" / "iOS x.y is not installed"
  and `-showdestinations` lists ZERO eligible destinations, the iOS platform component is
  missing — run `sudo xcodebuild -downloadPlatform iOS` (≈8.5 GB), NOT Xcode GUI. Check the sim's
  OS matches the toolchain first: an iOS 27.0 sim is invisible to Xcode 26.x until the platform
  component lands; pick a same-model device on a supported runtime via `xcrun simctl list`.
- Release/unsigned builds: check `codesign -d --entitlements - <binary>` — an app built with
  `CODE_SIGNING_ALLOWED=NO` has NO entitlements blob at all. On iOS 27 sims,
  `expo-secure-store` then throws `KeyChainException: A required entitlement is not present`
  on every set/get → pairing (Trust & Link persists `cosmos.desktops`) always fails; iOS ≤26
  tolerates it. Rebuild with normal `xcodebuild -configuration Release` ("Sign to Run Locally"
  embeds the DER entitlements blob — empty dict is enough) and pairing works. Re-signing an
  unsigned bundle ad-hoc cannot fix it: launchd refuses binaries carrying
  `keychain-access-groups`/`application-identifier` without provisioning, and an XML-slot-only
  empty entitlements dict does NOT satisfy iOS 27's keychain — the DER-encoded blob is required.
- `simctl` (install/launch/io screenshot) on a freshly-booted sim can sit silent for 60-90s
  while installd settles — just wait; don't assume it's hung.
- In the pair sheet, pressing Return inside the Address field triggers "Check Connection".

## Driving the CuaDriver / Watch Live path
- The packaged desktop embeds the engine; launch `target/package/Cosmos.app/Contents/MacOS/zeron`
  directly (env `ZERON_MOCK_QUESTION=1 ZERON_MOCK_DELAY_MS=500` makes MockHarness runs park at
  `awaitingInput` — ideal for Stop/interrupt testing).
- Computer page (Settings → Computer): the state badge is display-only; enable via the
  "Screen Context" row toggle; "Install component" pulls the pinned Cua Driver to
  `~/.zeron/computer/cua-driver/versions/<ver>-<triple>/`.
- The driver daemon needs the `com.trycua.driver` TCC identity → it must be started through
  LaunchServices (`open -a CuaDriver --args serve`), and `open -a` only resolves after the
  bundle has been registered once (`open <path-to-CuaDriver.app>` by full path).
- Frames require BOTH Screen Recording AND Accessibility granted to CuaDriver
  (System Settings → Privacy & Security; each toggle shows an admin password sheet — the
  account password works). The daemon's permission gate refuses all actions while either is
  pending; `ComputerObserve` then returns `permissions_pending` and the watch stage shows the
  error text. Stop/interrupt is independent of frames and still works.
- Watch out: Cosmos writes `manifest-observe.yaml` with a `schema_version` field that
  cua-driver 0.28.2 rejects (`expected version`) — manifest-driven daemon launches fail to
  start entirely.

## Seeding engine state over IPC (for list density / running sessions)
- `ws://127.0.0.1:27654`, unauthenticated JSON `{id, method, params}` → `{id, ok|err|item}`.
- `LocalDevice` → `{deviceId}`; `Mutate {op:'createChat', chatId, deviceId, config:{harness,
  model:null, reasoning:'medium', modelOptions:{}, sandbox:'workspace-write'}}`;
  `Mutate {op:'renameChat'|'setChatActivity'|'setChatConfig'}`; harness ids are kebab-case
  ('mock' = built-in MockHarness, never shown in pickers); `QueueCommand {chatId,
  command:{kind:'run', request:{prompt, harness, cwd, sandbox, autoApprove:false,
  model:null, reasoning:'medium', modelOptions:{}, resume:null}, messageId}}` starts a run
  on the local device; `WatchSessions` first frame returns `[{chatId,status,...}]`.

## Devin Secrets Needed
- none for local pairing. Google account credentials only if re-testing Woozlit sign-in.

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

## Devin Secrets Needed
- none for local pairing. Google account credentials only if re-testing Woozlit sign-in.

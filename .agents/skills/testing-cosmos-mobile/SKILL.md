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

## Devin Secrets Needed
- none for local pairing. Google account credentials only if re-testing Woozlit sign-in.

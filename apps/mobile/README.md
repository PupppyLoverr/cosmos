# Cosmos mobile (Expo)

The canonical phone client. Kotlin/Compose `apps/android` is **legacy reference**
and is not the production UI.

This app talks the same protocol as iOS:

- registry row-table (`GET /registry/{org}/rows`) — live sessions/devices/spaces
- device-room relay ControlRpc (`QueueCommand`, `Mutate`, Computer pairing)
- chat2 tail for transcripts
- Woozlit (Firebase custom token) or `AUTH_MODE=dev` bearer `user@org`

There is **no** mock session list, fake Computer Use, or send-that-only-nudges.
Send is `QueueCommand { kind: run }`. Stop is `QueueCommand { kind: interrupt }`.
Computer Use is pairing-gated (`callerDeviceId` + `pairingToken`).

## Android

```
npx expo prebuild --platform android
npx expo run:android
# or: npm run apk
```

Package id: `sh.cosmos.mobile`.

Push notifications: local presentation always; remote FCM reports
`credential-missing` until a real FCM key is supplied at runtime.

import * as Notifications from "expo-notifications";
import { deliveryForPush, presentNotice, type Notice } from "./protocol/notices";
import type { NoticeKind } from "./protocol/types";

Notifications.setNotificationHandler({
  handleNotification: async () => ({
    shouldShowAlert: true,
    shouldPlaySound: false,
    shouldSetBadge: false,
    shouldShowBanner: true,
    shouldShowList: true,
  }),
});

export function hasRemotePushCredential(): boolean {
  const token = process.env.EXPO_PUBLIC_FCM_TOKEN ?? process.env.EXPO_PUBLIC_PUSH_TOKEN;
  return Boolean(token && token.trim());
}

export async function presentComputerNotice(input: {
  kind: NoticeKind;
  title: string;
  body: string;
}): Promise<Notice> {
  return presentNotice(input, {
    hasPushCredential: hasRemotePushCredential(),
    presentLocal: async (n) => {
      try {
        await Notifications.scheduleNotificationAsync({
          content: { title: n.title, body: n.body, data: { kind: n.kind } },
          trigger: null,
        });
      } catch {
        // Local presentation can fail without a permission grant. The notice
        // is still recorded with an honest delivery field.
      }
    },
  });
}

export async function requestLocalPermission(): Promise<boolean> {
  const existing = await Notifications.getPermissionsAsync();
  if (existing.granted) return true;
  const asked = await Notifications.requestPermissionsAsync();
  return asked.granted;
}

export { deliveryForPush };

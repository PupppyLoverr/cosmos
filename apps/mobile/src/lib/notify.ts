// Notifications — local notifications driven by live stream events (turn
// finished, approval requested, run failed, desktop offline/reconnected).
// Remote push delivery requires an APNs-registered build + server relay; the
// app schedules local notifications for state changes observed while it runs
// and posts expo push tokens only when a projectId exists.

import * as Notifications from 'expo-notifications';
import { Platform } from 'react-native';

Notifications.setNotificationHandler({
  handleNotification: async () => ({
    shouldPlaySound: false,
    shouldSetBadge: true,
    shouldShowBanner: true,
    shouldShowList: true,
  }),
});

export async function ensureNotificationPermission(): Promise<boolean> {
  const existing = await Notifications.getPermissionsAsync();
  if (existing.granted) return true;
  const res = await Notifications.requestPermissionsAsync();
  if (Platform.OS === 'android') {
    await Notifications.setNotificationChannelAsync('sessions', {
      name: 'Sessions',
      importance: Notifications.AndroidImportance.DEFAULT,
    });
  }
  return res.granted;
}

export async function notifySessionEvent(title: string, body: string, data: Record<string, string>) {
  const granted = await ensureNotificationPermission();
  if (!granted) return;
  await Notifications.scheduleNotificationAsync({
    content: { title, body, data },
    trigger: null,
  });
}

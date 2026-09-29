import { AppState, Platform, type AppStateStatus } from 'react-native';
import { useEffect, useMemo, useRef, useState } from 'react';
import * as LiveActivity from 'expo-live-activity';
import { harnessLabel } from '../components/harness-mark';
import { useCosmos, useSnapshot } from './client';
import { DESKTOP_OFFLINE } from './rpc';
import type { Chat, Session } from './protocol';

const LIVE_STATUSES = new Set<Session['status']>(['working', 'awaitingInput', 'errored']);

type ActivityRecord = {
  id: string;
  chatId: string;
};

type ActivityState = LiveActivity.LiveActivityState & {
  chatId: string;
};

const activityConfig: LiveActivity.LiveActivityConfig = {
  backgroundColor: '#0B0B0C',
  titleColor: '#FFFFFF',
  subtitleColor: '#A1A1AA',
  deepLinkUrl: '',
  padding: { horizontal: 16, vertical: 14 },
  imagePosition: 'left',
  imageAlign: 'center',
  imageSize: { width: 40, height: 40 },
};

function sessionPriority(status: Session['status']): number {
  if (status === 'awaitingInput') return 3;
  if (status === 'errored') return 2;
  return 1;
}

function newestFirst(a: Session, b: Session): number {
  const priority = sessionPriority(b.status) - sessionPriority(a.status);
  if (priority !== 0) return priority;
  return Date.parse(b.updatedAt) - Date.parse(a.updatedAt);
}

function sessionTitle(chat?: Chat): string {
  return chat?.title?.trim() || chat?.lastMessagePreview?.trim() || 'Session';
}

function timeLabel(date: Date): string {
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function toActivityState(state: ActivityState): LiveActivity.LiveActivityState {
  const { chatId: _chatId, ...activityState } = state;
  return activityState;
}

export function useLiveActivity(): void {
  const client = useCosmos();
  const sessions = useSnapshot(client.sessions);
  const chats = useSnapshot(client.chats);
  const connection = useSnapshot(client.connection);
  const [appState, setAppState] = useState<AppStateStatus>(AppState.currentState);
  const activityRef = useRef<ActivityRecord | null>(null);
  const lastSerializedRef = useRef<string | null>(null);
  const lastTitleRef = useRef('Session');
  const unavailableRef = useRef(false);

  const liveSessions = useMemo(
    () => sessions
      .filter((session) => LIVE_STATUSES.has(session.status))
      .sort(newestFirst),
    [sessions],
  );
  const focused = liveSessions[0];
  const focusedChat = focused ? chats.find((chat) => chat.id === focused.chatId) : undefined;
  const focusedTitle = sessionTitle(focusedChat);
  const liveSubtitle = focused
    ? focused.status === 'awaitingInput'
      ? 'Needs your input'
      : focused.status === 'errored'
        ? 'Failed — tap to retry'
        : `Working · ${harnessLabel(focusedChat?.config?.harness) ?? 'Cosmos'}`
    : 'Finished';
  const subtitle = focused && liveSessions.length > 1
    ? `${liveSubtitle} · +${liveSessions.length - 1} more`
    : liveSubtitle;
  const activityState = useMemo<ActivityState | null>(() => {
    if (!focused) return null;
    const stateSubtitle = connection.detail === DESKTOP_OFFLINE && connection.status !== 'connected'
      ? 'Desktop asleep · resumes on wake'
      : connection.status === 'disconnected'
        ? 'Cosmos not connected'
        : subtitle;
    const displayedSubtitle = appState === 'background'
      ? `${stateSubtitle} · as of ${timeLabel(new Date())}`
      : stateSubtitle;
    return {
      chatId: focused.chatId,
      title: focusedTitle,
      subtitle: displayedSubtitle,
      progressBar: undefined,
      imageName: 'cosmos_mark',
      dynamicIslandImageName: 'cosmos_island',
    };
  }, [appState, connection.status, connection.detail, focused, focusedTitle, subtitle]);

  useEffect(() => {
    if (Platform.OS !== 'ios') return;
    const subscription = AppState.addEventListener('change', setAppState);
    return () => subscription.remove();
  }, []);

  useEffect(() => {
    if (Platform.OS !== 'ios') return;
    let subscription: ReturnType<typeof LiveActivity.addActivityUpdatesListener>;
    try {
      subscription = LiveActivity.addActivityUpdatesListener((event) => {
        if (
          activityRef.current?.id === event.activityID
          && (event.activityState === 'dismissed' || event.activityState === 'ended')
        ) {
          activityRef.current = null;
          lastSerializedRef.current = null;
        }
      });
    } catch {
      subscription = undefined;
    }
    return () => subscription?.remove();
  }, []);

  useEffect(() => {
    if (Platform.OS !== 'ios') return;
    if (!activityState) {
      unavailableRef.current = false;
      if (activityRef.current) {
        try {
          LiveActivity.stopActivity(activityRef.current.id, {
            title: lastTitleRef.current,
            subtitle: 'Finished',
          });
        } catch {
          // Live Activities can disappear while the app is suspended.
        }
        activityRef.current = null;
        lastSerializedRef.current = null;
      }
      return;
    }

    const serialized = JSON.stringify(toActivityState(activityState));
    const current = activityRef.current;

    if (current && current.chatId !== activityState.chatId) {
      try {
        LiveActivity.stopActivity(current.id, {
          title: lastTitleRef.current,
          subtitle: 'Finished',
        });
      } catch {
        // Continue with the newly focused session.
      }
      activityRef.current = null;
      lastSerializedRef.current = null;
    }

    lastTitleRef.current = activityState.title;

    if (!activityRef.current) {
      if (unavailableRef.current) return;
      try {
        const id = LiveActivity.startActivity(toActivityState(activityState), {
          ...activityConfig,
          deepLinkUrl: `/session/${activityState.chatId}`,
        });
        if (!id) {
          unavailableRef.current = true;
          return;
        }
        activityRef.current = { id, chatId: activityState.chatId };
        lastSerializedRef.current = serialized;
      } catch {
        unavailableRef.current = true;
      }
      return;
    }

    if (lastSerializedRef.current === serialized) return;
    try {
      LiveActivity.updateActivity(activityRef.current.id, toActivityState(activityState));
      lastSerializedRef.current = serialized;
    } catch {
      // The native activity may have been dismissed without an event.
    }
  }, [activityState]);
}

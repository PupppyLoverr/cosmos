import React from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { Link } from 'expo-router';
import ReanimatedSwipeable from 'react-native-gesture-handler/ReanimatedSwipeable';
import type { SwipeableMethods } from 'react-native-gesture-handler/ReanimatedSwipeable';
import * as Haptics from 'expo-haptics';
import Animated, { Extrapolation, interpolate, useAnimatedStyle, type SharedValue } from 'react-native-reanimated';
import { useLargeText } from '../lib/a11y';
import { radii, spacing, type, useTheme } from '../lib/theme';
import { Chat } from '../lib/protocol';
import { Icon, StatusChip, type SessionState } from './bits';
import { HarnessMark, harnessLabel } from './harness-mark';
import { Glass } from './surface';

export type SessionRowIndicator = SessionState;

export interface SessionRowProps {
  chat: Chat;
  title: string;
  indicator: SessionRowIndicator;
  timeAgo: string;
  desktop: string;
  elapsed?: string;
  translucent: boolean;
  onPress: () => void;
  onLongPress?: () => void;
  onRename: () => void;
  onArchive: () => void;
  onDelete: () => void;
}

function SwipeAction({
  icon,
  label,
  color,
  onPress,
  swipeable,
}: {
  icon: string;
  label: string;
  color: string;
  onPress: () => void;
  swipeable: SwipeableMethods;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={() => {
        void Haptics.selectionAsync();
        swipeable.close();
        onPress();
      }}
      style={({ pressed }) => [styles.swipeAction, { backgroundColor: color, opacity: pressed ? 0.75 : 1 }]}
    >
      <Icon name={icon} size={18} color="#fff" weight="semibold" />
      <Text style={[type.caption2, styles.swipeLabel]}>{label}</Text>
    </Pressable>
  );
}

function RightActions({
  progress,
  swipeable,
  archived,
  color,
  danger,
  onArchive,
  onDelete,
}: {
  progress: SharedValue<number>;
  swipeable: SwipeableMethods;
  archived: boolean;
  color: string;
  danger: string;
  onArchive: () => void;
  onDelete: () => void;
}) {
  const style = useAnimatedStyle(() => ({
    opacity: interpolate(progress.value, [0, 0.15], [0, 1], Extrapolation.CLAMP),
  }));

  return (
    <Animated.View style={[styles.actions, style]}>
      <SwipeAction
        icon={archived ? 'tray.and.arrow.up' : 'archivebox'}
        label={archived ? 'Unarchive' : 'Archive'}
        color={color}
        onPress={onArchive}
        swipeable={swipeable}
      />
      <SwipeAction icon="trash" label="Delete" color={danger} onPress={onDelete} swipeable={swipeable} />
    </Animated.View>
  );
}

export const SessionRow = React.memo(function SessionRow({
  chat,
  title,
  indicator,
  timeAgo,
  desktop,
  elapsed,
  translucent,
  onPress,
  onLongPress,
  onRename,
  onArchive,
  onDelete,
}: SessionRowProps) {
  const t = useTheme();
  const largeText = useLargeText();
  const harness = harnessLabel(chat.config?.harness);
  // harness · desktop · time — the running clock replaces "ago" while live.
  const when = indicator === 'working' && elapsed ? elapsed : timeAgo;
  const meta = [harness, desktop, when].filter(Boolean).join(' · ');
  const accessibleLabel = [title, indicator === 'idle' ? undefined : indicator, meta].filter(Boolean).join(', ');
  const href = { pathname: '/session/[id]' as const, params: { id: chat.id, title } };
  const row = (
    <Link asChild href={href}>
      <Link.Trigger>
        <Pressable
          onPress={onPress}
          onLongPress={onLongPress}
          delayLongPress={350}
          accessibilityRole="button"
          accessibilityLabel={accessibleLabel}
          style={({ pressed }) => [
            styles.row,
            {
              backgroundColor: pressed ? t.fill : translucent ? undefined : t.surface,
              borderColor: translucent ? 'transparent' : t.glassStroke,
              opacity: chat.archived ? 0.6 : 1,
            },
          ]}
        >
          <HarnessMark harness={chat.config?.harness} size={30} />
          <View style={styles.copy}>
            <Text style={[type.sessionTitle, { color: t.text }]} numberOfLines={largeText ? 2 : 1}>
              {title}
            </Text>
            <Text style={[type.meta, { color: t.textSecondary }]} numberOfLines={1}>{meta}</Text>
          </View>
          <StatusChip state={indicator} />
        </Pressable>
      </Link.Trigger>
      <Link.Menu title={title}>
        <Link.MenuAction title="Rename" icon="pencil" onPress={onRename} />
        <Link.MenuAction title={chat.archived ? 'Unarchive' : 'Archive'} icon={chat.archived ? 'tray.and.arrow.up' : 'archivebox'} onPress={onArchive} />
        <Link.MenuAction title="Delete" icon="trash" destructive onPress={onDelete} />
      </Link.Menu>
      <Link.Preview />
    </Link>
  );

  return (
    <View style={styles.cellWrap}>
      <ReanimatedSwipeable
        friction={2}
        rightThreshold={40}
        overshootRight={false}
        containerStyle={styles.swipeContainer}
        childrenContainerStyle={styles.swipeChildren}
        renderRightActions={(progress, _, swipeable) => (
          <RightActions
            progress={progress}
            swipeable={swipeable}
            archived={chat.archived}
            color={t.link}
            danger={t.danger}
            onArchive={onArchive}
            onDelete={onDelete}
          />
        )}
      >
        {translucent ? <Glass radius={radii.row}>{row}</Glass> : row}
      </ReanimatedSwipeable>
    </View>
  );
});

const styles = StyleSheet.create({
  cellWrap: { paddingBottom: spacing(2) },
  row: {
    minHeight: 64,
    paddingHorizontal: spacing(3),
    paddingVertical: spacing(3),
    borderRadius: radii.row,
    borderCurve: 'continuous',
    borderWidth: StyleSheet.hairlineWidth,
    flexDirection: 'row',
    alignItems: 'center',
    gap: spacing(3),
  },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  swipeContainer: { borderRadius: radii.row, overflow: 'hidden' },
  swipeChildren: { flex: 1 },
  actions: { flexDirection: 'row', width: 144, borderRadius: radii.row, overflow: 'hidden' },
  swipeAction: { width: 72, alignItems: 'center', justifyContent: 'center', gap: spacing(1) },
  swipeLabel: { color: '#fff', fontWeight: '600' },
});

import React from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { Link } from 'expo-router';
import ReanimatedSwipeable from 'react-native-gesture-handler/ReanimatedSwipeable';
import type { SwipeableMethods } from 'react-native-gesture-handler/ReanimatedSwipeable';
import * as Haptics from 'expo-haptics';
import { useLargeText } from '../lib/a11y';
import { radii, spacing, type, useTheme } from '../lib/theme';
import { Chat } from '../lib/protocol';
import { Icon, StatusPill } from './bits';
import { HarnessMark } from './harness-mark';
import { Glass } from './surface';

export type SessionRowIndicator = 'working' | 'awaitingInput' | 'errored' | 'completed' | 'idle';

export interface SessionRowProps {
  chat: Chat;
  title: string;
  indicator: SessionRowIndicator;
  timeAgo: string;
  desktop: string;
  project: string;
  branch?: string | null;
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

export const SessionRow = React.memo(function SessionRow({
  chat,
  title,
  indicator,
  timeAgo,
  desktop,
  project,
  branch,
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
  const statusLabel = indicator === 'working'
    ? 'Working'
    : indicator === 'awaitingInput'
      ? 'Needs input'
      : indicator === 'errored'
        ? 'Failed'
        : indicator === 'completed'
          ? 'Done'
          : timeAgo;
  const accessibleLabel = `${title}, ${statusLabel}, ${desktop}, ${project}${branch ? `, ${branch}` : ''}`;
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
              opacity: chat.archived ? 0.72 : 1,
            },
          ]}
        >
          <View style={styles.titleLine}>
            <HarnessMark harness={chat.config?.harness} size={14} />
            <Text style={[type.subhead, styles.title, { color: t.text, fontWeight: '600' }]} numberOfLines={largeText ? 2 : 1}>
              {title}
            </Text>
            {indicator === 'idle' ? (
              <Text style={[type.meta, { color: t.textTertiary }]}>{timeAgo}</Text>
            ) : indicator === 'completed' ? (
              <View style={styles.done}>
                <Icon name="checkmark" size={11} color={t.success} weight="bold" />
                <Text style={[type.meta, { color: t.success, fontWeight: '600' }]}>Done</Text>
              </View>
            ) : (
              <StatusPill
                status={indicator}
                label={statusLabel}
                pulse={indicator === 'working'}
              />
            )}
          </View>
          <View style={styles.metaLine}>
            <Text style={[type.footnote, styles.metaText, { color: t.textTertiary }]} numberOfLines={1}>
              {desktop} · {project}
            </Text>
            {branch ? (
              <View style={styles.branch}>
                <Text style={[type.footnote, { color: t.textTertiary }]}> · </Text>
                <Icon name="git.branch" size={11} color={t.textTertiary} />
                <Text style={[type.footnote, styles.branchText, { color: t.textTertiary }]} numberOfLines={1}>{branch}</Text>
              </View>
            ) : null}
            {indicator === 'working' && elapsed ? (
              <Text style={[type.footnote, { color: t.textTertiary }]}> · {elapsed}</Text>
            ) : null}
          </View>
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
        renderRightActions={(_, __, swipeable) => (
          <View style={styles.actions}>
            <SwipeAction
              icon={chat.archived ? 'tray.and.arrow.up' : 'archivebox'}
              label={chat.archived ? 'Unarchive' : 'Archive'}
              color={t.link}
              onPress={onArchive}
              swipeable={swipeable}
            />
            <SwipeAction icon="trash" label="Delete" color={t.danger} onPress={onDelete} swipeable={swipeable} />
          </View>
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
    minHeight: 56,
    paddingHorizontal: spacing(3),
    paddingVertical: spacing(2.5),
    borderRadius: radii.row,
    flexDirection: 'column',
    justifyContent: 'center',
    gap: spacing(1),
  },
  titleLine: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), minWidth: 0 },
  title: { flex: 1, minWidth: 0 },
  done: { flexDirection: 'row', alignItems: 'center', gap: spacing(1) },
  metaLine: { flexDirection: 'row', alignItems: 'center', minWidth: 0, gap: 0 },
  metaText: { flexShrink: 1 },
  branch: { flexDirection: 'row', alignItems: 'center', flexShrink: 1 },
  branchText: { flexShrink: 1, marginLeft: spacing(1) },
  swipeContainer: { borderRadius: radii.row, overflow: 'hidden' },
  swipeChildren: { flex: 1 },
  actions: { flexDirection: 'row', width: 144, borderRadius: radii.row, overflow: 'hidden' },
  swipeAction: { width: 72, alignItems: 'center', justifyContent: 'center', gap: spacing(1) },
  swipeLabel: { color: '#fff', fontWeight: '600' },
});

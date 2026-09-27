import React, { useEffect, useState } from 'react';
import { ActivityIndicator, LayoutAnimation, Pressable, StyleSheet, Text, View } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming, type SharedValue } from 'react-native-reanimated';
import * as Haptics from 'expo-haptics';
import { toolGroupSummary } from '../lib/activity';
import { Arrive, useReduceMotion } from '../lib/motion';
import { SessionMessageEntry, MessagePart, ToolCall, toolCallName } from '../lib/protocol';
import { Theme, alpha, collapseBezier, motion, radii, spacing, type, useTheme } from '../lib/theme';
import { Button, Icon, StatusPill } from './bits';
import { Md } from './markdown';

type ToolPart = Extract<MessagePart, { kind: 'tool' }>;
type InputPart = Extract<MessagePart, { kind: 'input' }>;
type ReasoningPart = Extract<MessagePart, { kind: 'reasoning' }>;

function animateLayout(reduce: boolean) {
  if (reduce) return;
  LayoutAnimation.configureNext({
    duration: motion.collapse,
    update: { type: LayoutAnimation.Types.easeInEaseOut },
    create: { type: LayoutAnimation.Types.easeInEaseOut, property: LayoutAnimation.Properties.opacity },
    delete: { type: LayoutAnimation.Types.easeInEaseOut, property: LayoutAnimation.Properties.opacity },
  });
}

/** iOS disclosure: a right chevron that rotates down. Snaps when Reduce Motion is on. */
function Disclosure({ open, color }: { open: boolean; color: string }) {
  const reduce = useReduceMotion();
  const turn = useSharedValue(open ? 1 : 0);
  useEffect(() => {
    if (reduce) {
      turn.set(open ? 1 : 0);
      return;
    }
    turn.set(withTiming(open ? 1 : 0, {
      duration: motion.collapse,
      easing: Easing.bezier(collapseBezier[0], collapseBezier[1], collapseBezier[2], collapseBezier[3]),
    }));
  }, [open, reduce, turn]);
  const style = useAnimatedStyle(() => ({ transform: [{ rotate: `${turn.get() * 90}deg` }] }));
  return (
    <Animated.View style={style}>
      <Icon name="chevron.right" size={12} color={color} weight="semibold" />
    </Animated.View>
  );
}

// ---------------------------------------------------------------------------
// Tool call presentation

interface ToolFace { icon: string; verb: string; detail: string; computer?: boolean; mono?: boolean }

function humanize(name: string): string {
  return name.replace(/[_-]+/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

function describe(call: ToolCall): ToolFace {
  switch (call.kind) {
    case 'exec': return { icon: 'terminal', verb: 'Ran', detail: call.command, mono: true };
    case 'readFile': return { icon: 'doc.text', verb: 'Read', detail: call.path, mono: true };
    case 'writeFile': return { icon: 'doc.badge.plus', verb: 'Wrote', detail: call.path, mono: true };
    case 'editFile': return { icon: 'square.and.pencil', verb: 'Edited', detail: call.path, mono: true };
    case 'applyPatch': return { icon: 'square.and.pencil', verb: 'Patched', detail: call.path ?? '', mono: true };
    case 'search': return { icon: 'magnifyingglass', verb: 'Searched', detail: call.pattern ?? '', mono: true };
    case 'glob': return { icon: 'folder', verb: 'Listed', detail: call.pattern ?? '', mono: true };
    case 'webFetch': return { icon: 'safari', verb: 'Fetched', detail: call.url };
    case 'webSearch': return { icon: 'globe', verb: 'Searched the web', detail: call.query };
    case 'todo': return { icon: 'checklist', verb: 'Planned', detail: `${call.items?.length ?? 0} steps` };
    case 'mcp': {
      const computer = /computer/i.test(call.server) || /^computer_/.test(call.tool);
      return { icon: computer ? 'display' : 'puzzlepiece.extension', verb: humanize(call.tool.replace(/^computer_/, '')), detail: summarizeInput(call.input), computer };
    }
    case 'unknown': {
      const name = toolCallName(call);
      const computer = /computer/i.test(call.name);
      return { icon: computer ? 'display' : 'wrench.and.screwdriver', verb: humanize(name), detail: summarizeInput(call.input), computer };
    }
  }
}

function summarizeInput(input: unknown): string {
  if (input == null) return '';
  if (typeof input === 'string') return input.slice(0, 120);
  if (typeof input !== 'object') return String(input);
  const obj = input as Record<string, unknown>;
  const pick = obj.command ?? obj.path ?? obj.prompt ?? obj.query ?? obj.file ?? obj.text ?? obj.app ?? obj.title ?? obj.action ?? obj.selector;
  if (typeof pick === 'string') return pick.slice(0, 120);
  const keys = Object.keys(obj);
  return keys.length ? keys.slice(0, 3).map((k) => `${k}: ${String(obj[k]).slice(0, 24)}`).join(' · ') : '';
}

function ToolLine({ part, t }: { part: ToolPart; t: Theme }) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const reduce = useReduceMotion();
  const face = describe(part.call);
  const running = !part.resolved;
  const body = part.output ?? part.subagentTail ?? '';
  const bodyLines = body.trim() ? body.trim().split(/\r?\n/) : [];
  const expandable = body.length > 0 || (part.diffStats?.length ?? 0) > 0;
  const tint = part.isError ? t.danger : running ? t.link : t.textSecondary;

  return (
    <View>
      <Pressable
        onPress={expandable ? () => { animateLayout(reduce); setOpen((v) => !v); } : undefined}
        accessibilityRole={expandable ? 'button' : undefined}
        accessibilityState={expandable ? { expanded: open } : undefined}
        accessibilityLabel={`${face.verb} ${face.detail}${running ? ', running' : part.isError ? ', failed' : ', done'}`}
        style={({ pressed }) => [styles.toolLine, pressed && expandable && { backgroundColor: t.fill }]}
      >
        <View style={styles.toolIcon}>
          {running ? <ActivityIndicator size="small" color={t.link} style={{ transform: [{ scale: 0.7 }] }} /> : part.isError ? <Icon name="exclamationmark.circle.fill" size={14} color={t.danger} /> : <Icon name={face.icon} size={14} color={tint} />}
        </View>
        <Text style={[type.footnote, { color: t.text, flex: 1 }]} numberOfLines={open ? undefined : 1}>
          <Text style={{ fontWeight: '600', color: part.isError ? t.danger : t.text }}>{face.verb}</Text>
          {face.detail ? <Text style={[{ color: t.textSecondary }, face.mono && { fontFamily: type.mono.fontFamily, fontSize: type.mono.fontSize }]}>  {face.detail}</Text> : null}
        </Text>
        {expandable ? <Disclosure open={open} color={t.textTertiary} /> : null}
      </Pressable>
      {open ? (
        <View style={[styles.toolOutput, { backgroundColor: t.codeBg }]}>
          {part.diffStats?.map((d) => (
            <Text key={d.path} style={[type.caption, { color: t.textSecondary }]} numberOfLines={1}>
              {d.path}  <Text style={{ color: t.success }}>+{d.additions}</Text> <Text style={{ color: t.danger }}>−{d.deletions}</Text>
            </Text>
          ))}
          {body ? (
            <>
              <Text style={[type.mono, { color: t.textSecondary }]} numberOfLines={showAll ? 60 : 24} selectable>{body.trim()}</Text>
              {bodyLines.length > 24 && !showAll ? (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`Show all ${bodyLines.length} lines`}
                  onPress={(event) => {
                    event.stopPropagation();
                    setShowAll(true);
                  }}
                >
                  <Text style={[type.caption, { color: t.link }]}>Show all ({bodyLines.length} lines)</Text>
                </Pressable>
              ) : null}
            </>
          ) : null}
          {part.outputBytes && !part.output ? <Text style={[type.caption, { color: t.textTertiary }]}>{Math.round(part.outputBytes / 1024)} KB — full output on the desktop</Text> : null}
        </View>
      ) : null}
    </View>
  );
}

/** Consecutive tool calls fold behind the desktop summary ("Ran 2 commands · 1 failed"). */
function ToolGroup({ parts, t }: { parts: ToolPart[]; t: Theme }) {
  const reduce = useReduceMotion();
  const running = parts.some((p) => !p.resolved);
  const errors = parts.filter((p) => p.isError).length;
  // Null until the user toggles: open while the group is still running, closed once it settles.
  const [pinned, setPinned] = useState<boolean | null>(null);
  const open = pinned ?? running;
  const summary = toolGroupSummary(parts);
  const state = running ? 'Running' : errors ? `${errors} failed` : '';
  const diffStats = parts.reduce(
    (totals, part) => {
      for (const diff of part.diffStats ?? []) {
        totals.additions += diff.additions;
        totals.deletions += diff.deletions;
      }
      return totals;
    },
    { additions: 0, deletions: 0 },
  );
  const hasDiffStats = !running && (diffStats.additions > 0 || diffStats.deletions > 0);

  return (
    <View>
      <Pressable
        onPress={() => { animateLayout(reduce); setPinned(!(pinned ?? running)); }}
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={state ? `${summary}, ${state}` : summary}
        style={({ pressed }) => [styles.toolHead, pressed && { backgroundColor: t.fill }]}
      >
        <Disclosure open={open} color={t.textTertiary} />
        <View style={styles.toolSummary}>
          <Text style={[type.footnote, { color: t.text, flexShrink: 1, fontWeight: '500' }]} numberOfLines={open ? 2 : 1}>{summary}</Text>
          {!open && hasDiffStats ? (
            <Text style={[type.caption, { color: t.textTertiary }]}>
              <Text style={{ color: t.success }}>+{diffStats.additions}</Text>
              {' '}
              <Text style={{ color: t.danger }}>−{diffStats.deletions}</Text>
            </Text>
          ) : null}
        </View>
        {running ? <StatusPill status="working" label="Running" pulse /> : errors > 0 ? <Text style={[type.caption, { color: t.danger }]}>{errors} failed</Text> : null}
      </Pressable>
      {open ? parts.map((p) => <ToolLine key={p.id} part={p} t={t} />) : null}
    </View>
  );
}

// ---------------------------------------------------------------------------

function ReasoningRow({ part, t }: { part: ReasoningPart; t: Theme }) {
  const [open, setOpen] = useState(false);
  const reduce = useReduceMotion();
  const firstLine = part.text.trim().split('\n').find((l) => l.trim()) ?? '';
  return (
    <Pressable
      onPress={() => { animateLayout(reduce); setOpen((v) => !v); }}
      accessibilityRole="button"
      accessibilityState={{ expanded: open }}
      accessibilityLabel={open ? 'Thought process' : firstLine || 'Thought process'}
      style={({ pressed }) => [styles.reasoning, { opacity: pressed ? 0.7 : 1 }]}
    >
      <View style={styles.reasoningHead}>
        <Disclosure open={open} color={t.textTertiary} />
        <Text style={[type.footnote, { color: t.textSecondary, flex: 1 }]} numberOfLines={open ? undefined : 1}>
          {open ? 'Thought process' : firstLine || 'Thought process'}
        </Text>
      </View>
      {open ? <Text style={[type.footnote, styles.reasoningBody, { color: t.textSecondary, borderLeftColor: t.separator }]} selectable>{part.text.trim()}</Text> : null}
    </Pressable>
  );
}

export function ApprovalCard({ part, onAnswer }: {
  part: InputPart;
  onAnswer: (requestId: string, answers: { questionId: string; labels: string[] }[]) => void;
}) {
  const t = useTheme();
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const answered = part.questions.every((q) => (picked[q.id]?.length ?? 0) > 0);

  if (part.resolved) {
    return (
      <View style={[styles.resolved, { backgroundColor: t.fill }]}>
        <Icon name="checkmark.circle.fill" size={14} color={t.success} />
        <Text style={[type.footnote, { color: t.textSecondary }]}>You answered</Text>
      </View>
    );
  }
  return (
    <View style={[styles.approval, { backgroundColor: t.surface, borderColor: alpha(t.warning, 0.6) }]}>
      <View style={styles.approvalHead}>
        <View style={[styles.approvalGlyph, { backgroundColor: alpha(t.warning, 0.18) }]}>
          <Icon name="hand.raised.fill" size={14} color={t.warning} />
        </View>
        <Text style={[type.headline, { color: t.text, flex: 1 }]}>Needs your input</Text>
      </View>
      {part.questions.map((q) => (
        <View key={q.id} style={{ gap: spacing(2.5) }}>
          {q.header && q.question ? <Text style={[type.caption2, { color: t.textTertiary }]}>{q.header.toUpperCase()}</Text> : null}
          <Text style={[type.subhead, { color: t.text }]}>{q.question || q.header}</Text>
          <View style={styles.options}>
            {q.options.map((opt) => {
              const sel = (picked[q.id] ?? []).includes(opt);
              return (
                <Pressable
                  key={opt}
                  accessibilityRole={q.multiSelect ? 'checkbox' : 'radio'}
                  accessibilityState={{ checked: sel }}
                  onPress={() => {
                    void Haptics.selectionAsync();
                    setPicked((p) => {
                      const cur = p[q.id] ?? [];
                      const next = q.multiSelect ? (sel ? cur.filter((x) => x !== opt) : [...cur, opt]) : [opt];
                      return { ...p, [q.id]: next };
                    });
                  }}
                  style={[styles.option, { backgroundColor: sel ? t.accentStrong : t.fill }]}
                >
                  <Text style={[type.subhead, { color: sel ? t.accentContrast : t.text, fontWeight: sel ? '600' : '400' }]}>{opt}</Text>
                </Pressable>
              );
            })}
          </View>
        </View>
      ))}
      <Button
        title="Send Answer"
        size="md"
        disabled={!answered}
        style={{ alignSelf: 'flex-start' }}
        onPress={() => onAnswer(part.requestId, part.questions.map((q) => ({ questionId: q.id, labels: picked[q.id] ?? [] })))}
      />
    </View>
  );
}

/** Three-dot breathing indicator while the assistant is still producing output. */
function Typing({ color }: { color: string }) {
  const reduce = useReduceMotion();
  const phase = useSharedValue(0);
  useEffect(() => {
    if (reduce) {
      phase.set(0);
      return;
    }
    phase.set(withRepeat(withTiming(1, { duration: 1100, easing: Easing.linear }), -1, false));
  }, [phase, reduce]);
  return (
    <View style={styles.typing} accessibilityElementsHidden>
      {[0, 0.33, 0.66].map((offset) => <TypingDot key={offset} color={color} phase={phase} offset={offset} reduce={reduce} />)}
    </View>
  );
}

function TypingDot({ color, phase, offset, reduce }: { color: string; phase: SharedValue<number>; offset: number; reduce: boolean }) {
  const style = useAnimatedStyle(() => ({
    opacity: reduce ? 0.6 : 0.25 + 0.75 * Math.abs(Math.sin((phase.get() + offset) * Math.PI)),
    transform: [{ translateY: reduce ? 0 : -2 * Math.abs(Math.sin((phase.get() + offset) * Math.PI)) }],
  }));
  return <Animated.View style={[styles.typingDot, { backgroundColor: color }, style]} />;
}

// ---------------------------------------------------------------------------

export const MessageView = React.memo(function MessageView({ entry, onAnswer, grouped, arrive }: {
  entry: SessionMessageEntry;
  onAnswer: (requestId: string, answers: { questionId: string; labels: string[] }[]) => void;
  /** True when the previous message has the same role, so vertical spacing tightens. */
  grouped?: boolean;
  /** Play the desktop fade-in once, for a message that just arrived. */
  arrive?: boolean;
}) {
  const t = useTheme();
  const isUser = entry.role === 'user';

  if (isUser) {
    const text = entry.parts.filter((p) => p.kind === 'text').map((p) => (p as { text: string }).text).join('').trim();
    const images = entry.parts.filter((p): p is Extract<MessagePart, { kind: 'image' }> => p.kind === 'image');
    if (!text && !images.length) return null;
    return (
      <Arrive active={!!arrive}>
      <View style={[styles.entry, styles.userEntry, grouped && styles.grouped]}>
        {images.length ? (
          <View style={styles.attachments}>
            {images.map((img) => (
              <View key={img.id} style={[styles.attachment, { backgroundColor: t.fill }]}>
                <Icon name={img.mimeType.startsWith('image/') ? 'photo' : 'doc'} size={13} color={t.textSecondary} />
                <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>{img.name}</Text>
              </View>
            ))}
          </View>
        ) : null}
        {text ? (
          <View style={[styles.userBubble, { backgroundColor: t.surface }]}>
            <Text style={[type.body, { color: t.text }]} selectable>{text}</Text>
          </View>
        ) : null}
      </View>
      </Arrive>
    );
  }

  // Fold runs of tool calls into a single activity card.
  const blocks: (MessagePart | { kind: 'tools'; id: string; parts: ToolPart[] })[] = [];
  for (const part of entry.parts) {
    const prev = blocks[blocks.length - 1];
    if (part.kind === 'tool') {
      if (prev?.kind === 'tools') prev.parts.push(part);
      else blocks.push({ kind: 'tools', id: part.id, parts: [part] });
    } else {
      blocks.push(part);
    }
  }
  const streaming = entry.status === 'streaming';
  const hasText = entry.parts.some((p) => p.kind === 'text' && p.text.trim());

  return (
    <Arrive active={!!arrive}>
    <View style={[styles.entry, grouped && styles.grouped]}>
      <View style={styles.assistant}>
        {blocks.map((b) => {
          switch (b.kind) {
            case 'text':
              return b.text.trim() ? <Md key={b.id} streaming={streaming}>{b.text.trim()}</Md> : null;
            case 'reasoning':
              return b.text.trim() ? <ReasoningRow key={b.id} part={b} t={t} /> : null;
            case 'tools':
              return <ToolGroup key={b.id} parts={b.parts} t={t} />;
            case 'input':
              return <ApprovalCard key={b.id} part={b} onAnswer={onAnswer} />;
            case 'error':
              return (
                <View key={b.id} style={[styles.error, { backgroundColor: alpha(t.danger, 0.1) }]}>
                  <Icon name="exclamationmark.triangle.fill" size={14} color={t.danger} />
                  <Text style={[type.subhead, { color: t.danger, flex: 1 }]} selectable>{b.message}</Text>
                </View>
              );
            case 'image':
              return (
                <View key={b.id} style={[styles.attachment, { backgroundColor: t.fill, alignSelf: 'flex-start' }]}>
                  <Icon name="photo" size={13} color={t.textSecondary} />
                  <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>{b.name}</Text>
                </View>
              );
            default:
              return null;
          }
        })}
        {streaming && !hasText ? (
          <View style={styles.working} accessibilityLabel="Working">
            <Typing color={t.textTertiary} />
            <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '500' }]}>Working</Text>
          </View>
        ) : null}
      </View>
    </View>
    </Arrive>
  );
});

const styles = StyleSheet.create({
  entry: { paddingHorizontal: spacing(4), marginBottom: spacing(4) },
  grouped: { marginTop: -spacing(2) },
  userEntry: { alignItems: 'flex-end', gap: spacing(1.5) },
  userBubble: {
    maxWidth: '80%', borderRadius: 18, borderCurve: 'continuous',
    paddingHorizontal: spacing(3.5), paddingVertical: spacing(2),
  },
  attachments: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', gap: spacing(1.5), maxWidth: '80%' },
  attachment: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(2.5), paddingVertical: spacing(1.5),
    borderRadius: radii.md, borderCurve: 'continuous', maxWidth: 220,
  },
  assistant: { gap: spacing(2.5) },
  assistantText: { lineHeight: 22 },
  working: { flexDirection: 'row', alignItems: 'center', gap: spacing(2) },

  reasoning: { paddingVertical: spacing(0.5) },
  reasoningHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  reasoningBody: { marginTop: spacing(2), paddingLeft: spacing(3), borderLeftWidth: 2, lineHeight: 19 },

  toolHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), minHeight: 44 },
  toolSummary: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'baseline', gap: spacing(2) },
  toolLine: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), paddingLeft: spacing(2), minHeight: 44 },
  toolIcon: { width: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
  toolOutput: { marginHorizontal: spacing(3), marginBottom: spacing(2.5), padding: spacing(2.5), borderRadius: radii.sm, borderCurve: 'continuous', gap: spacing(1) },

  approval: { borderRadius: radii.xl, borderCurve: 'continuous', borderWidth: 1, padding: spacing(4), gap: spacing(3.5) },
  approvalHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(2.5) },
  approvalGlyph: { width: 30, height: 30, borderRadius: 9, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  options: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2) },
  option: { borderRadius: radii.pill, paddingHorizontal: spacing(3.5), paddingVertical: spacing(2), minHeight: 44, justifyContent: 'center' },
  resolved: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), alignSelf: 'flex-start', paddingHorizontal: spacing(2.5), paddingVertical: spacing(1.5), borderRadius: radii.pill },

  error: { flexDirection: 'row', alignItems: 'flex-start', gap: spacing(2), padding: spacing(3), borderRadius: radii.md, borderCurve: 'continuous' },
  typing: { flexDirection: 'row', gap: 5, paddingVertical: spacing(2), paddingHorizontal: spacing(1) },
  typingDot: { width: 7, height: 7, borderRadius: 3.5 },
});

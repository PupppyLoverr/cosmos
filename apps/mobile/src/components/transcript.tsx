import React, { useEffect, useState } from 'react';
import { ActivityIndicator, LayoutAnimation, Pressable, StyleSheet, Text, View } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming, type SharedValue } from 'react-native-reanimated';
import * as Haptics from 'expo-haptics';
import { SessionMessageEntry, MessagePart, ToolCall, toolCallName } from '../lib/protocol';
import { Theme, alpha, radii, spacing, type, useTheme } from '../lib/theme';
import { Button, Icon } from './bits';

type ToolPart = Extract<MessagePart, { kind: 'tool' }>;
type InputPart = Extract<MessagePart, { kind: 'input' }>;
type ReasoningPart = Extract<MessagePart, { kind: 'reasoning' }>;

const animateLayout = () => LayoutAnimation.configureNext(LayoutAnimation.Presets.easeInEaseOut);

// ---------------------------------------------------------------------------
// Tool call presentation

interface ToolFace { icon: string; verb: string; detail: string; computer?: boolean }

function humanize(name: string): string {
  return name.replace(/[_-]+/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

function describe(call: ToolCall): ToolFace {
  switch (call.kind) {
    case 'exec': return { icon: 'terminal', verb: 'Ran', detail: call.command };
    case 'readFile': return { icon: 'doc.text', verb: 'Read', detail: call.path };
    case 'writeFile': return { icon: 'doc.badge.plus', verb: 'Wrote', detail: call.path };
    case 'editFile': return { icon: 'square.and.pencil', verb: 'Edited', detail: call.path };
    case 'applyPatch': return { icon: 'square.and.pencil', verb: 'Patched', detail: call.path ?? '' };
    case 'search': return { icon: 'magnifyingglass', verb: 'Searched', detail: call.pattern ?? '' };
    case 'glob': return { icon: 'folder', verb: 'Listed', detail: call.pattern ?? '' };
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

function ToolLine({ part, t, last }: { part: ToolPart; t: Theme; last: boolean }) {
  const [open, setOpen] = useState(false);
  const face = describe(part.call);
  const running = !part.resolved;
  const body = part.output ?? part.subagentTail ?? '';
  const expandable = body.length > 0 || (part.diffStats?.length ?? 0) > 0;
  const tint = part.isError ? t.danger : face.computer ? t.link : t.textSecondary;

  return (
    <View>
      <Pressable
        onPress={expandable ? () => { animateLayout(); setOpen((v) => !v); } : undefined}
        accessibilityRole={expandable ? 'button' : undefined}
        accessibilityState={expandable ? { expanded: open } : undefined}
        accessibilityLabel={`${face.verb} ${face.detail}${running ? ', running' : part.isError ? ', failed' : ''}`}
        style={({ pressed }) => [styles.toolLine, pressed && expandable && { backgroundColor: t.fill }]}
      >
        <View style={styles.toolIcon}>
          {running ? <ActivityIndicator size="small" color={t.link} style={{ transform: [{ scale: 0.7 }] }} /> : <Icon name={face.icon} size={14} color={tint} />}
        </View>
        <Text style={[type.subhead, { color: t.text, flex: 1 }]} numberOfLines={open ? undefined : 1}>
          <Text style={{ fontWeight: '600', color: part.isError ? t.danger : t.text }}>{face.verb}</Text>
          {face.detail ? <Text style={{ color: t.textSecondary }}>  {face.detail}</Text> : null}
        </Text>
        {part.isError ? <Icon name="exclamationmark.circle.fill" size={14} color={t.danger} /> : null}
        {expandable ? <Icon name="chevron.right" size={10} color={t.textTertiary} weight="semibold" /> : null}
      </Pressable>
      {open ? (
        <View style={[styles.toolOutput, { backgroundColor: t.codeBg }]}>
          {part.diffStats?.map((d) => (
            <Text key={d.path} style={[type.caption, { color: t.textSecondary }]} numberOfLines={1}>
              {d.path}  <Text style={{ color: t.success }}>+{d.additions}</Text> <Text style={{ color: t.danger }}>−{d.deletions}</Text>
            </Text>
          ))}
          {body ? <Text style={[type.mono, { color: t.textSecondary }]} numberOfLines={60} selectable>{body.trim()}</Text> : null}
          {part.outputBytes && !part.output ? <Text style={[type.caption, { color: t.textTertiary }]}>{Math.round(part.outputBytes / 1024)} KB — full output on the desktop</Text> : null}
        </View>
      ) : null}
      {!last ? <View style={[styles.toolSep, { backgroundColor: t.separator }]} /> : null}
    </View>
  );
}

/** Consecutive tool calls fold into one activity card so long runs stay scannable. */
function ToolGroup({ parts, t }: { parts: ToolPart[]; t: Theme }) {
  const [expanded, setExpanded] = useState(false);
  const running = parts.some((p) => !p.resolved);
  const errors = parts.filter((p) => p.isError).length;
  const computer = parts.some((p) => describe(p.call).computer);
  const collapsible = parts.length > 3;
  const shown = collapsible && !expanded ? parts.slice(-3) : parts;
  const hidden = parts.length - shown.length;

  return (
    <View style={[styles.toolCard, { backgroundColor: t.surface, borderColor: t.separator }]}>
      <View style={styles.toolHead}>
        <Icon name={computer ? 'display' : 'gearshape.2'} size={12} color={t.textTertiary} />
        <Text style={[type.caption2, { color: t.textTertiary, flex: 1 }]}>
          {(computer ? 'COMPUTER USE' : 'ACTIVITY')} · {parts.length} {parts.length === 1 ? 'STEP' : 'STEPS'}
        </Text>
        {running ? <Text style={[type.caption2, { color: t.link }]}>RUNNING</Text> : errors ? <Text style={[type.caption2, { color: t.danger }]}>{errors} FAILED</Text> : <Icon name="checkmark" size={11} color={t.success} weight="bold" />}
      </View>
      {hidden > 0 ? (
        <Pressable onPress={() => { animateLayout(); setExpanded(true); }} style={styles.toolMore} accessibilityRole="button">
          <Text style={[type.footnote, { color: t.link }]}>Show {hidden} earlier {hidden === 1 ? 'step' : 'steps'}</Text>
        </Pressable>
      ) : null}
      {shown.map((p, i) => <ToolLine key={p.id} part={p} t={t} last={i === shown.length - 1} />)}
    </View>
  );
}

// ---------------------------------------------------------------------------

function ReasoningRow({ part, t }: { part: ReasoningPart; t: Theme }) {
  const [open, setOpen] = useState(false);
  const firstLine = part.text.trim().split('\n').find((l) => l.trim()) ?? '';
  return (
    <Pressable
      onPress={() => { animateLayout(); setOpen((v) => !v); }}
      accessibilityRole="button"
      accessibilityState={{ expanded: open }}
      style={({ pressed }) => [styles.reasoning, { opacity: pressed ? 0.7 : 1 }]}
    >
      <View style={styles.reasoningHead}>
        <Icon name="sparkle" size={12} color={t.textTertiary} />
        <Text style={[type.footnote, { color: t.textTertiary, flex: 1, fontWeight: '500' }]} numberOfLines={open ? undefined : 1}>
          {open ? 'Thinking' : firstLine || 'Thinking…'}
        </Text>
        <Icon name={open ? 'chevron.up' : 'chevron.down'} size={10} color={t.textTertiary} weight="semibold" />
      </View>
      {open ? <Text style={[type.footnote, styles.reasoningBody, { color: t.textSecondary, borderLeftColor: t.separator }]}>{part.text.trim()}</Text> : null}
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
        <Text style={[type.headline, { color: t.text, flex: 1 }]}>Cosmos needs your input</Text>
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
  const phase = useSharedValue(0);
  useEffect(() => {
    phase.value = withRepeat(withTiming(1, { duration: 1100, easing: Easing.linear }), -1, false);
  }, [phase]);
  return (
    <View style={styles.typing} accessibilityLabel="Cosmos is typing">
      {[0, 0.33, 0.66].map((offset) => <TypingDot key={offset} color={color} phase={phase} offset={offset} />)}
    </View>
  );
}

function TypingDot({ color, phase, offset }: { color: string; phase: SharedValue<number>; offset: number }) {
  const style = useAnimatedStyle(() => ({
    opacity: 0.25 + 0.75 * Math.abs(Math.sin((phase.value + offset) * Math.PI)),
    transform: [{ translateY: -2 * Math.abs(Math.sin((phase.value + offset) * Math.PI)) }],
  }));
  return <Animated.View style={[styles.typingDot, { backgroundColor: color }, style]} />;
}

// ---------------------------------------------------------------------------

export function MessageView({ entry, onAnswer, grouped }: {
  entry: SessionMessageEntry;
  onAnswer: (requestId: string, answers: { questionId: string; labels: string[] }[]) => void;
  /** True when the previous message has the same role, so vertical spacing tightens. */
  grouped?: boolean;
}) {
  const t = useTheme();
  const isUser = entry.role === 'user';

  if (isUser) {
    const text = entry.parts.filter((p) => p.kind === 'text').map((p) => (p as { text: string }).text).join('').trim();
    const images = entry.parts.filter((p): p is Extract<MessagePart, { kind: 'image' }> => p.kind === 'image');
    if (!text && !images.length) return null;
    return (
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
          <View style={[styles.userBubble, { backgroundColor: t.userBubble }]}>
            <Text style={[type.body, { color: t.userBubbleText }]} selectable>{text}</Text>
          </View>
        ) : null}
      </View>
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
    <View style={[styles.entry, grouped && styles.grouped]}>
      <View style={styles.assistant}>
        {blocks.map((b) => {
          switch (b.kind) {
            case 'text':
              return b.text.trim() ? <Text key={b.id} style={[type.body, styles.assistantText, { color: t.text }]} selectable>{b.text.trim()}</Text> : null;
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
        {streaming && !hasText ? <Typing color={t.textTertiary} /> : null}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  entry: { paddingHorizontal: spacing(4), marginBottom: spacing(4) },
  grouped: { marginTop: -spacing(2) },
  userEntry: { alignItems: 'flex-end', gap: spacing(1.5) },
  userBubble: {
    maxWidth: '80%', borderRadius: 20, borderCurve: 'continuous',
    paddingHorizontal: spacing(3.5), paddingVertical: spacing(2.25),
    borderBottomRightRadius: 6,
  },
  attachments: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', gap: spacing(1.5), maxWidth: '80%' },
  attachment: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(2.5), paddingVertical: spacing(1.5),
    borderRadius: radii.md, borderCurve: 'continuous', maxWidth: 220,
  },
  assistant: { gap: spacing(2.5) },
  assistantText: { lineHeight: 24 },

  reasoning: { paddingVertical: spacing(0.5) },
  reasoningHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  reasoningBody: { marginTop: spacing(2), paddingLeft: spacing(3), borderLeftWidth: 2, lineHeight: 19 },

  toolCard: { borderRadius: radii.lg, borderCurve: 'continuous', borderWidth: StyleSheet.hairlineWidth, overflow: 'hidden' },
  toolHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), paddingHorizontal: spacing(3), paddingTop: spacing(2.5), paddingBottom: spacing(1.5) },
  toolMore: { paddingHorizontal: spacing(3), paddingVertical: spacing(1.5) },
  toolLine: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), paddingHorizontal: spacing(3), paddingVertical: spacing(2.25), minHeight: 40 },
  toolIcon: { width: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
  toolSep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(3) + 20 + spacing(2) },
  toolOutput: { marginHorizontal: spacing(3), marginBottom: spacing(2.5), padding: spacing(2.5), borderRadius: radii.sm, borderCurve: 'continuous', gap: spacing(1) },

  approval: { borderRadius: radii.xl, borderCurve: 'continuous', borderWidth: 1, padding: spacing(4), gap: spacing(3.5) },
  approvalHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(2.5) },
  approvalGlyph: { width: 30, height: 30, borderRadius: 9, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  options: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2) },
  option: { borderRadius: radii.pill, paddingHorizontal: spacing(3.5), paddingVertical: spacing(2), minHeight: 36, justifyContent: 'center' },
  resolved: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), alignSelf: 'flex-start', paddingHorizontal: spacing(2.5), paddingVertical: spacing(1.5), borderRadius: radii.pill },

  error: { flexDirection: 'row', alignItems: 'flex-start', gap: spacing(2), padding: spacing(3), borderRadius: radii.md, borderCurve: 'continuous' },
  typing: { flexDirection: 'row', gap: 5, paddingVertical: spacing(2), paddingHorizontal: spacing(1) },
  typingDot: { width: 7, height: 7, borderRadius: 3.5 },
});

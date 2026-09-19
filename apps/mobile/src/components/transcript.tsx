import React, { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { SymbolView } from 'expo-symbols';
import { SessionMessageEntry, MessagePart, ToolCall, toolCallName } from '../lib/protocol';
import { Theme, radii, spacing, useTheme } from '../lib/theme';

function toolIcon(name: string): string {
  const n = name.toLowerCase();
  if (n.includes('computer') || n.includes('screenshot')) return 'display';
  if (n.includes('browser') || n.includes('web')) return 'safari';
  if (n.includes('shell') || n.includes('bash') || n.includes('terminal')) return 'terminal';
  if (n.includes('read') || n.includes('view')) return 'doc.text';
  if (n.includes('write') || n.includes('edit')) return 'square.and.pencil';
  if (n.includes('search') || n.includes('find') || n.includes('grep')) return 'magnifyingglass';
  if (n.includes('composio') || n.includes('tool')) return 'puzzlepiece.extension';
  return 'wrench.and.screwdriver';
}

function ReasoningRow({ part, theme }: { part: Extract<MessagePart, { kind: 'reasoning' }>; theme: Theme }) {
  const [open, setOpen] = useState(false);
  const preview = part.text.trim().split('\n')[0]?.slice(0, 90) ?? '';
  return (
    <Pressable onPress={() => setOpen((v) => !v)} style={[styles.reasoning, { borderColor: theme.separator }]}>
      <View style={styles.reasoningHead}>
        <SymbolView name="brain" size={13} tintColor={theme.textTertiary} />
        <Text style={{ color: theme.textTertiary, fontSize: 13, fontStyle: 'italic', flex: 1 }} numberOfLines={open ? undefined : 1}>
          {open ? 'Thinking' : preview || 'Thinking…'}
        </Text>
        <SymbolView name={open ? 'chevron.up' : 'chevron.down'} size={11} tintColor={theme.textTertiary} />
      </View>
      {open ? <Text style={[styles.reasoningBody, { color: theme.textSecondary }]}>{part.text}</Text> : null}
    </Pressable>
  );
}

// Wire call is a kind-tagged union — the chip name and one-line input
// summary both come from `toolCallName`/`summarizeCall`.
function summarizeCall(call: ToolCall): string {
  switch (call.kind) {
    case 'exec': return call.command;
    case 'readFile': case 'writeFile': case 'editFile': return call.path;
    case 'applyPatch': return call.path ?? '';
    case 'search': case 'glob': return call.pattern ?? '';
    case 'webFetch': return call.url;
    case 'webSearch': return call.query;
    case 'todo': return `${call.items?.length ?? 0} items`;
    case 'mcp': return summarizeInput(call.input);
    case 'unknown': return summarizeInput(call.input);
  }
}

function ToolRow({ part, theme }: { part: Extract<MessagePart, { kind: 'tool' }>; theme: Theme }) {
  const [open, setOpen] = useState(false);
  const name = toolCallName(part.call);
  const inputSummary = summarizeCall(part.call);
  const detail = part.output ?? inputSummary;
  return (
    <View style={[styles.tool, { borderColor: theme.separator, backgroundColor: theme.surface }]}>
      <Pressable onPress={() => setOpen((v) => !v)} style={styles.toolHead}>
        <SymbolView name={toolIcon(name) as never} size={13} tintColor={part.isError ? theme.danger : theme.textSecondary} />
        <Text style={{ color: theme.textSecondary, fontSize: 13, flex: 1 }} numberOfLines={1}>
          {name.replace(/_/g, ' ')}
          {inputSummary ? <Text style={{ color: theme.textTertiary }}>  {inputSummary}</Text> : null}
        </Text>
        {!part.resolved ? <Text style={{ color: theme.accent, fontSize: 12 }}>running</Text> : null}
        {part.isError ? <SymbolView name="exclamationmark.triangle" size={12} tintColor={theme.danger} /> : null}
        {detail ? <SymbolView name={open ? 'chevron.up' : 'chevron.down'} size={11} tintColor={theme.textTertiary} /> : null}
      </Pressable>
      {open && detail ? (
        <Text style={[styles.toolDetail, { color: theme.textSecondary, backgroundColor: theme.codeBg }]} numberOfLines={40}>
          {detail}
        </Text>
      ) : null}
    </View>
  );
}

function summarizeInput(input: unknown): string {
  if (input == null) return '';
  if (typeof input === 'string') return input.slice(0, 80);
  try {
    const obj = input as Record<string, unknown>;
    const pick = obj.command ?? obj.path ?? obj.prompt ?? obj.query ?? obj.file ?? obj.tool ?? obj.app ?? obj.action;
    return typeof pick === 'string' ? pick.slice(0, 80) : '';
  } catch {
    return '';
  }
}

export function ApprovalCard({ part, onAnswer }: {
  part: Extract<MessagePart, { kind: 'input' }>;
  onAnswer: (requestId: string, answers: { questionId: string; labels: string[] }[]) => void;
}) {
  const t = useTheme();
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  if (part.resolved) {
    return (
      <View style={[styles.approval, { borderColor: t.separator, backgroundColor: t.surface }]}>
        <Text style={{ color: theme_ok(t), fontSize: 13 }}>Answered</Text>
      </View>
    );
  }
  return (
    <View style={[styles.approval, { borderColor: t.warning, backgroundColor: t.surface }]}>
      {part.questions.map((q) => (
        <View key={q.id} style={{ gap: spacing(2) }}>
          <Text style={{ color: t.text, fontSize: 14, fontWeight: '600' }}>{q.question || q.header}</Text>
          <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2) }}>
            {q.options.map((opt) => {
              const sel = (picked[q.id] ?? []).includes(opt);
              return (
                <Pressable
                  key={opt}
                  onPress={() =>
                    setPicked((p) => {
                      const cur = p[q.id] ?? [];
                      const next = q.multiSelect
                        ? (sel ? cur.filter((x) => x !== opt) : [...cur, opt])
                        : [opt];
                      return { ...p, [q.id]: next };
                    })
                  }
                  style={[styles.option, { borderColor: sel ? t.accent : t.separator, backgroundColor: sel ? t.accentMuted : 'transparent' }]}
                >
                  <Text style={{ color: sel ? t.accent : t.text, fontSize: 14 }}>{opt}</Text>
                </Pressable>
              );
            })}
          </View>
        </View>
      ))}
      <Pressable
        style={[styles.approveBtn, { backgroundColor: t.accent }]}
        onPress={() => onAnswer(part.requestId, part.questions.map((q) => ({ questionId: q.id, labels: picked[q.id] ?? [] })))}
      >
        <Text style={{ color: '#fff', fontWeight: '600' }}>Send answer</Text>
      </Pressable>
    </View>
  );
}

const theme_ok = (t: Theme) => t.success;

export function MessageView({ entry, onAnswer }: {
  entry: SessionMessageEntry;
  onAnswer: (requestId: string, answers: { questionId: string; labels: string[] }[]) => void;
}) {
  const t = useTheme();
  const isUser = entry.role === 'user';
  const text = entry.parts.filter((p) => p.kind === 'text').map((p) => (p as { text: string }).text).join('');
  const others = entry.parts.filter((p) => p.kind !== 'text' || !isUser);

  if (isUser && !text) return null;
  return (
    <View style={[styles.entry, isUser ? { alignItems: 'flex-end' } : null]}>
      {isUser ? (
        <View style={[styles.userBubble, { backgroundColor: t.userBubble }]}>
          <Text style={{ color: '#fff', fontSize: 16, lineHeight: 22 }}>{text}</Text>
        </View>
      ) : (
        <View style={styles.assistant}>
          {others.map((part) => {
            switch (part.kind) {
              case 'text':
                return part.text ? <Text key={part.id} style={{ color: t.text, fontSize: 16, lineHeight: 23 }}>{part.text}</Text> : null;
              case 'reasoning':
                return <ReasoningRow key={part.id} part={part} theme={t} />;
              case 'tool':
                return <ToolRow key={part.id} part={part} theme={t} />;
              case 'input':
                return <ApprovalCard key={part.id} part={part} onAnswer={onAnswer} />;
              case 'error':
                return (
                  <View key={part.id} style={[styles.errorBox, { borderColor: t.danger }]}>
                    <Text style={{ color: t.danger, fontSize: 13 }}>{part.message}</Text>
                  </View>
                );
              case 'image':
                return <Text key={part.id} style={{ color: t.textTertiary, fontSize: 13 }}>Attachment: {part.name}</Text>;
              default:
                return null;
            }
          })}
          {entry.status === 'streaming' ? <Text style={{ color: t.textTertiary, fontSize: 12, marginTop: 2 }}>●</Text> : null}
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  entry: { paddingHorizontal: spacing(4), marginBottom: spacing(3) },
  userBubble: {
    maxWidth: '85%', borderRadius: radii.xl, borderBottomRightRadius: radii.sm,
    paddingHorizontal: spacing(4), paddingVertical: spacing(2.5),
  },
  assistant: { gap: spacing(2) },
  reasoning: { borderLeftWidth: 2, paddingLeft: spacing(3), paddingVertical: spacing(1) },
  reasoningHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  reasoningBody: { fontSize: 13, lineHeight: 19, marginTop: spacing(1), fontStyle: 'italic' },
  tool: {
    borderWidth: StyleSheet.hairlineWidth, borderRadius: radii.md, overflow: 'hidden',
  },
  toolHead: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), paddingHorizontal: spacing(3), paddingVertical: spacing(2) },
  toolDetail: {
    fontSize: 12, fontFamily: 'Menlo', paddingHorizontal: spacing(3), paddingVertical: spacing(2),
    borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: 'rgba(128,128,128,0.2)',
  },
  approval: {
    borderWidth: 1, borderRadius: radii.lg, padding: spacing(4), gap: spacing(3),
  },
  option: {
    borderWidth: 1, borderRadius: radii.pill, paddingHorizontal: spacing(3.5), paddingVertical: spacing(1.75),
  },
  approveBtn: { alignSelf: 'flex-start', borderRadius: radii.pill, paddingHorizontal: spacing(4), paddingVertical: spacing(2) },
  errorBox: { borderWidth: 1, borderRadius: radii.md, padding: spacing(3) },
});

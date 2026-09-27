import React, { useMemo } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import * as Haptics from 'expo-haptics';
import Markdown, { sealIncompleteMarkdown } from '@ronradtke/react-native-markdown-display';
import type { ASTNode, MarkdownStyleMap, RenderRules } from '@ronradtke/react-native-markdown-display';
import { alpha, radii, spacing, type, useTheme, type Theme } from '../lib/theme';
import { Icon } from './bits';

/**
 * Themed markdown for assistant text. Agent output is markdown — headings,
 * lists, inline code and fences all render with real structure instead of a
 * wall of plain text. Styles come from the active theme, so code surfaces and
 * links match the rest of the transcript.
 */
function stylesFor(t: Theme) {
  return StyleSheet.create({
    body: { ...type.transcript, color: t.text },
    paragraph: { marginTop: 0, marginBottom: spacing(2) },
    heading1: { ...type.title3, color: t.text, marginTop: spacing(2), marginBottom: spacing(1) },
    heading2: { ...type.headline, color: t.text, marginTop: spacing(2), marginBottom: spacing(1) },
    heading3: { ...type.callout, color: t.text, fontWeight: '600', marginTop: spacing(1.5), marginBottom: spacing(0.5) },
    heading4: { ...type.subhead, color: t.text, fontWeight: '600' },
    heading5: { ...type.footnote, color: t.text, fontWeight: '600' },
    heading6: { ...type.footnote, color: t.textSecondary, fontWeight: '600' },
    strong: { fontWeight: '700' },
    em: { fontStyle: 'italic' },
    s: { textDecorationLine: 'line-through', color: t.textSecondary },
    link: { color: t.link, textDecorationLine: 'underline' },
    blocklink: { borderColor: 'transparent' },
    code_inline: {
      fontFamily: type.mono.fontFamily,
      fontSize: 12.5,
      color: t.text,
      backgroundColor: t.codeBg,
      borderRadius: 4,
    },
    code_block: {
      fontFamily: type.mono.fontFamily,
      fontSize: 12.5,
      lineHeight: 17,
      color: t.text,
      backgroundColor: t.codeBg,
      borderRadius: radii.md,
      borderCurve: 'continuous',
      borderWidth: StyleSheet.hairlineWidth,
      borderColor: t.separator,
      padding: spacing(2.5),
      marginVertical: spacing(1.5),
    },
    fence: {
      fontFamily: type.mono.fontFamily,
      fontSize: 12.5,
      lineHeight: 17,
      color: t.text,
      backgroundColor: t.codeBg,
      borderRadius: radii.md,
      borderCurve: 'continuous',
      borderWidth: StyleSheet.hairlineWidth,
      borderColor: t.separator,
      padding: spacing(2.5),
      marginVertical: spacing(1.5),
    },
    blockquote: {
      backgroundColor: 'transparent',
      borderLeftWidth: 2,
      borderLeftColor: alpha(t.textSecondary, 0.5),
      paddingLeft: spacing(2.5),
      marginLeft: 0,
      marginVertical: spacing(1),
    },
    bullet_list: { marginVertical: spacing(1) },
    ordered_list: { marginVertical: spacing(1) },
    list_item: { marginVertical: spacing(0.5), flexDirection: 'row', alignItems: 'flex-start' },
    bullet_list_icon: { color: t.textTertiary, marginLeft: spacing(1), marginRight: spacing(1.5), fontSize: 12, lineHeight: 22 },
    ordered_list_icon: { color: t.textTertiary, marginLeft: spacing(1), marginRight: spacing(1.5), fontSize: 13, lineHeight: 22 },
    hr: { backgroundColor: t.separator, height: StyleSheet.hairlineWidth, marginVertical: spacing(2.5) },
    table: { borderColor: t.separator, borderRadius: radii.sm },
    thead: { backgroundColor: t.fill },
    th: { padding: spacing(1.5), color: t.text, fontWeight: '600' },
    td: { padding: spacing(1.5), color: t.text },
    tr: { borderColor: t.separator },
    image: { borderRadius: radii.md, borderCurve: 'continuous' },
  });
}

function codeLanguage(node: ASTNode): string {
  if (typeof node.sourceInfo !== 'string') return 'code';
  return node.sourceInfo.trim().split(/\s+/)[0]?.toLowerCase() || 'code';
}

/** Assistant-message markdown. `streaming` seals an unclosed fence/inline so a
 *  partial chunk doesn't render as garbage mid-token. */
export function Md({ children, streaming }: { children: string; streaming?: boolean }) {
  const t = useTheme();
  const style = useMemo(() => stylesFor(t), [t]);
  const rules = useMemo<RenderRules>(() => {
    const renderCodeBlock = (node: ASTNode, _children: React.ReactNode[], _parent: ASTNode[], markdownStyles: MarkdownStyleMap) => {
      const code = node.content.replace(/\n+$/, '');
      return (
        <View key={node.key} style={[markdownStyles.fence ?? markdownStyles.code_block, { padding: 0 }]}>
          <View style={[styles.codeHeader, { borderBottomColor: t.separator }]}>
            <Text style={[type.caption, { color: t.textTertiary }]}>{codeLanguage(node)}</Text>
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Copy code"
              hitSlop={8}
              onPress={() => {
                void Clipboard.setStringAsync(node.content);
                void Haptics.selectionAsync();
              }}
            >
              <Icon name="doc.on.doc" size={14} color={t.textTertiary} />
            </Pressable>
          </View>
          <ScrollView horizontal showsHorizontalScrollIndicator={false} nestedScrollEnabled>
            <Text selectable style={[type.mono, styles.codeText, { color: t.text }]}>{code}</Text>
          </ScrollView>
        </View>
      );
    };
    return { fence: renderCodeBlock, code_block: renderCodeBlock };
  }, [t]);
  const text = streaming ? sealIncompleteMarkdown(children) : children;
  return (
    <Markdown
      style={style}
      rules={rules}
      colorScheme={t.scheme}
      mergeStyle
      onLinkPress={(url) => { void Linking.openURL(url).catch(() => {}); return true; }}
    >
      {text}
    </Markdown>
  );
}

const styles = StyleSheet.create({
  codeHeader: {
    minHeight: 32,
    paddingHorizontal: spacing(2.5),
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  codeText: {
    fontSize: 12.5,
    lineHeight: 17,
    padding: spacing(2.5),
  },
});

import React from 'react';
import { Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { Stack } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import * as Haptics from 'expo-haptics';
import * as ImagePicker from 'expo-image-picker';
import * as FileSystem from 'expo-file-system/legacy';
import { Image } from 'expo-image';
import { LinearGradient } from 'expo-linear-gradient';
import { Host, Slider } from '@expo/ui';
import {
  accentOptions,
  accentSwatch,
  backdropPresets,
  families,
  liquidGlassSupported,
  radii,
  spacing,
  type,
  useFamilySeeds,
  useTheme,
  useThemePrefs,
  type AccentId,
  type BackdropPresetId,
  type SurfacePref,
  type ThemeFamily,
} from '../../../lib/theme';
import { Group, Row, SectionLabel, Segmented } from '../../../components/bits';
import { Glass, Screen } from '../../../components/surface';

const PHOTO_DIR = `${FileSystem.documentDirectory ?? ''}backdrop/`;

async function importPhoto(uri: string): Promise<string> {
  await FileSystem.makeDirectoryAsync(PHOTO_DIR, { intermediates: true }).catch(() => undefined);
  const dest = `${PHOTO_DIR}${Date.now()}.jpg`;
  await FileSystem.copyAsync({ from: uri, to: dest });
  return dest;
}

export default function AppearanceScreen() {
  const t = useTheme();
  const [prefs, setPrefs] = useThemePrefs();
  const seeds = useFamilySeeds();
  const isPhoto = prefs.backdrop.kind === 'photo';
  const [picking, setPicking] = React.useState(false);

  const pickPhoto = async () => {
    setPicking(true);
    try {
      const res = await ImagePicker.launchImageLibraryAsync({ mediaTypes: ['images'], quality: 0.85, allowsEditing: false });
      const asset = res.assets?.[0];
      if (res.canceled || !asset) return;
      const previous = prefs.backdrop.kind === 'photo' ? prefs.backdrop.uri : null;
      const uri = await importPhoto(asset.uri);
      setPrefs({ backdrop: { kind: 'photo', uri } });
      if (previous) void FileSystem.deleteAsync(previous, { idempotent: true });
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
    } finally {
      setPicking(false);
    }
  };

  const materialNote = t.material === 'opaque' && prefs.surface !== 'opaque'
    ? 'Reduce Transparency is on — using solid surfaces.'
    : prefs.surface === 'glass' && !liquidGlassSupported
      ? Platform.OS === 'ios' ? 'Liquid Glass needs iOS 26 — showing frosted glass instead.' : 'Liquid Glass is iOS-only — showing tonal surfaces instead.'
      : liquidGlassSupported && t.material === 'glass'
        ? 'Apple Liquid Glass'
        : undefined;

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Appearance', headerLargeTitle: false }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content}>
        <Preview />

        <SectionLabel>Mode</SectionLabel>
        <Group>
          <View style={styles.pad}>
            <Segmented
              value={prefs.appearance}
              onChange={(appearance) => setPrefs({ appearance })}
              options={[{ id: 'system', label: 'System' }, { id: 'light', label: 'Light' }, { id: 'dark', label: 'Dark' }]}
            />
          </View>
        </Group>

        <SectionLabel>Theme</SectionLabel>
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.rail} style={styles.railWrap}>
          {families.map((f) => (
            <FamilyCard key={f.id} family={f} selected={prefs.family === f.id} onPress={() => setPrefs({ family: f.id })} />
          ))}
        </ScrollView>

        <SectionLabel>Accent</SectionLabel>
        <Group>
          <View style={[styles.pad, styles.wrap]}>
            {accentOptions.map((a) => (
              <AccentDot
                key={a.id}
                id={a.id}
                label={a.label}
                color={accentSwatch(a.id, seeds, t.scheme)}
                selected={prefs.accent === a.id}
                onPress={() => setPrefs({ accent: a.id })}
              />
            ))}
          </View>
        </Group>

        <SectionLabel>Background</SectionLabel>
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.rail} style={styles.railWrap}>
          <BackdropTile
            label="None"
            selected={prefs.backdrop.kind === 'none'}
            onPress={() => setPrefs({ backdrop: { kind: 'none' } })}
          >
            <View style={[styles.fill, { backgroundColor: t.background }]} />
          </BackdropTile>
          {backdropPresets.map((p) => (
            <BackdropTile
              key={p.id}
              label={p.name}
              selected={prefs.backdrop.kind === 'preset' && prefs.backdrop.id === p.id}
              onPress={() => setPrefs({ backdrop: { kind: 'preset', id: p.id as BackdropPresetId } })}
            >
              <LinearGradient colors={p[t.scheme] as [string, string, ...string[]]} start={{ x: 0.1, y: 0 }} end={{ x: 0.9, y: 1 }} style={styles.fill} />
            </BackdropTile>
          ))}
          <BackdropTile label={isPhoto ? 'Photo' : 'Choose photo'} selected={isPhoto} onPress={() => void pickPhoto()} busy={picking}>
            {isPhoto && prefs.backdrop.kind === 'photo' ? (
              <Image source={{ uri: prefs.backdrop.uri }} style={styles.fill} contentFit="cover" />
            ) : (
              <View style={[styles.fill, styles.center, { backgroundColor: t.fill }]}>
                <SymbolView name="photo.on.rectangle" size={22} tintColor={t.link} />
              </View>
            )}
          </BackdropTile>
        </ScrollView>
        {t.hasBackdrop ? (
          <Group>
            <Row
              title="Dim"
              subtitle="Keeps text readable over the background"
              leading={<SymbolView name="circle.lefthalf.filled" size={18} tintColor={t.textSecondary} />}
            />
            <View style={[styles.pad, { paddingTop: 0 }]}>
              <Host matchContents style={{ width: '100%' }} colorScheme={t.scheme}>
                <Slider value={prefs.backdropDim} min={0} max={0.85} step={0.05} onValueChange={(backdropDim) => setPrefs({ backdropDim })} />
              </Host>
            </View>
            {isPhoto ? (
              <Row
                title="Remove photo"
                danger
                leading={<SymbolView name="trash" size={18} tintColor={t.danger} />}
                onPress={() => {
                  if (prefs.backdrop.kind === 'photo') void FileSystem.deleteAsync(prefs.backdrop.uri, { idempotent: true });
                  setPrefs({ backdrop: { kind: 'none' } });
                }}
              />
            ) : null}
          </Group>
        ) : null}

        <SectionLabel>Surfaces</SectionLabel>
        <Group>
          <View style={styles.pad}>
            <Segmented<SurfacePref>
              value={prefs.surface}
              onChange={(surface) => setPrefs({ surface })}
              options={[
                { id: 'auto', label: 'Auto' },
                { id: 'glass', label: 'Glass' },
                { id: 'frosted', label: 'Frosted' },
                { id: 'opaque', label: 'Solid' },
              ]}
            />
            {materialNote ? (
              <Text style={[type.footnote, { color: t.textSecondary, marginTop: spacing(2.5) }]}>{materialNote}</Text>
            ) : null}
          </View>
        </Group>
      </ScrollView>
    </Screen>
  );
}

/** Live sample of the current theme: a mini session card and composer. */
function Preview() {
  const t = useTheme();
  return (
    <View style={styles.preview}>
      <Glass radius={radii.xl} style={styles.previewCard}>
        <View style={{ flexDirection: 'row', alignItems: 'center', gap: spacing(3) }}>
          <View style={[styles.previewMark, { backgroundColor: t.accentMuted }]}>
            <SymbolView name="sparkles" size={16} tintColor={t.link} />
          </View>
          <View style={{ flex: 1 }}>
            <Text style={[type.callout, { color: t.text, fontWeight: '600' }]}>{t.familyName}</Text>
            <Text style={[type.footnote, { color: t.textSecondary }]}>
              {t.material === 'glass' ? 'Liquid Glass' : t.material === 'frosted' ? 'Frosted' : 'Solid'} · {t.scheme === 'dark' ? 'Dark' : 'Light'}
            </Text>
          </View>
          <View style={[styles.previewPill, { backgroundColor: t.accentStrong }]}>
            <Text style={[type.caption, { color: t.accentContrast, fontWeight: '600' }]}>Send</Text>
          </View>
        </View>
      </Glass>
    </View>
  );
}

function FamilyCard({ family, selected, onPress }: { family: ThemeFamily; selected: boolean; onPress: () => void }) {
  const t = useTheme();
  const seeds = family[t.scheme] ?? family.dark ?? family.light!;
  const swatchScheme = family[t.scheme] ? t.scheme : 'dark';
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      accessibilityLabel={`${family.name} theme`}
      style={({ pressed }) => ({ opacity: pressed ? 0.8 : 1 })}
    >
      <View style={[styles.family, { borderColor: selected ? t.link : 'transparent', backgroundColor: seeds.background }]}>
        <View style={[styles.familyBar, { backgroundColor: seeds.card, borderColor: seeds.raised }]}>
          <View style={[styles.familyDot, { backgroundColor: seeds.accent }]} />
          <View style={[styles.familyLine, { backgroundColor: seeds.text, opacity: 0.85 }]} />
          <View style={[styles.familyLine, { backgroundColor: seeds.muted, width: '40%' }]} />
        </View>
        <View style={{ flexDirection: 'row', gap: 4, paddingHorizontal: spacing(2.5) }}>
          {[seeds.accent, seeds.success, seeds.warning, seeds.danger].map((c, i) => (
            <View key={i} style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: c }} />
          ))}
        </View>
        <Text style={[type.caption, { color: seeds.text, paddingHorizontal: spacing(2.5), fontWeight: '600' }]} numberOfLines={1}>
          {family.name}
        </Text>
        {!family[t.scheme] ? (
          <Text style={[type.caption, { color: seeds.muted, paddingHorizontal: spacing(2.5), marginTop: -4, fontSize: 10 }]}>
            {swatchScheme === 'dark' ? 'Dark only' : 'Light only'}
          </Text>
        ) : null}
      </View>
    </Pressable>
  );
}

function AccentDot({ id, label, color, selected, onPress }: { id: AccentId; label: string; color: string; selected: boolean; onPress: () => void }) {
  const t = useTheme();
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityLabel={`${label} accent`}
      accessibilityState={{ selected }}
      style={({ pressed }) => [styles.accent, { opacity: pressed ? 0.7 : 1 }]}
    >
      <View style={[styles.accentRing, { borderColor: selected ? t.text : 'transparent' }]}>
        <View style={[styles.accentDot, { backgroundColor: color }]}>
          {id === 'theme' ? <SymbolView name="paintpalette.fill" size={13} tintColor={t.accentContrast} /> : null}
          {id === 'mono' ? <SymbolView name="circle.lefthalf.filled" size={13} tintColor={t.background} /> : null}
        </View>
      </View>
      <Text style={[type.caption, { color: selected ? t.text : t.textSecondary }]} numberOfLines={1}>{label}</Text>
    </Pressable>
  );
}

function BackdropTile({ label, selected, onPress, busy, children }: {
  label: string; selected: boolean; onPress: () => void; busy?: boolean; children: React.ReactNode;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      disabled={busy}
      accessibilityRole="button"
      accessibilityLabel={`${label} background`}
      accessibilityState={{ selected, busy: !!busy }}
      style={({ pressed }) => ({ opacity: pressed || busy ? 0.7 : 1, alignItems: 'center', gap: spacing(1.5) })}
    >
      <View style={[styles.tile, { borderColor: selected ? t.link : t.separator }]}>
        {children}
        {selected ? (
          <View style={[styles.tileCheck, { backgroundColor: t.accentStrong }]}>
            <SymbolView name="checkmark" size={10} tintColor={t.accentContrast} weight="bold" />
          </View>
        ) : null}
      </View>
      <Text style={[type.caption, { color: selected ? t.text : t.textSecondary }]}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: spacing(4), paddingBottom: spacing(12) },
  pad: { paddingHorizontal: spacing(4), paddingVertical: spacing(3) },
  wrap: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(3), justifyContent: 'space-between' },
  railWrap: { marginHorizontal: -spacing(4) },
  rail: { paddingHorizontal: spacing(4), gap: spacing(3) },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  center: { alignItems: 'center', justifyContent: 'center' },
  preview: { marginTop: spacing(4) },
  previewCard: { padding: spacing(4) },
  previewMark: { width: 36, height: 36, borderRadius: 12, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  previewPill: { paddingHorizontal: spacing(3), paddingVertical: spacing(1.5), borderRadius: radii.pill },
  family: {
    width: 128, borderRadius: radii.lg, borderCurve: 'continuous', borderWidth: 2,
    paddingTop: spacing(2.5), paddingBottom: spacing(2.5), gap: spacing(2), overflow: 'hidden',
  },
  familyBar: {
    marginHorizontal: spacing(2.5), borderRadius: radii.sm, borderCurve: 'continuous', borderWidth: StyleSheet.hairlineWidth,
    padding: spacing(2), gap: 5,
  },
  familyDot: { width: 10, height: 10, borderRadius: 5, marginBottom: 2 },
  familyLine: { height: 4, borderRadius: 2, width: '70%' },
  accent: { width: 56, alignItems: 'center', gap: spacing(1.5) },
  accentRing: { width: 40, height: 40, borderRadius: 20, borderWidth: 2, padding: 3 },
  accentDot: { flex: 1, borderRadius: 16, alignItems: 'center', justifyContent: 'center' },
  tile: {
    width: 92, height: 140, borderRadius: radii.lg, borderCurve: 'continuous', borderWidth: 2, overflow: 'hidden',
  },
  tileCheck: {
    position: 'absolute', right: 8, bottom: 8, width: 20, height: 20, borderRadius: 10,
    alignItems: 'center', justifyContent: 'center',
  },
});

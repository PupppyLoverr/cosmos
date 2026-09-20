import React from 'react';
import { Alert, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import {
  accentOptions,
  accentSwatch,
  alpha,
  backdropPresets,
  defaultPrefs,
  edge,
  families,
  liquidGlassSupported,
  radii,
  spacing,
  surfaceOptions,
  type,
  useFamilySeeds,
  useTheme,
  useThemePrefs,
  wallpaperEffects,
  type AccentId,
  type BackdropPresetId,
  type SurfacePref,
  type ThemeFamily,
  type Backdrop,
} from '../../../lib/theme';
import { pickWallpaper, removeWallpaper, wallpaperPath } from '../../../lib/wallpaper-store';
import { Caption, Group, Icon, Row, SectionLabel, Segmented, Slider } from '../../../components/bits';
import { Glass, PresetGradient, Screen } from '../../../components/surface';
import { Wallpaper } from '../../../components/wallpaper';

export default function AppearanceScreen() {
  const t = useTheme();
  const [prefs, setPrefs] = useThemePrefs();
  const seeds = useFamilySeeds();
  const photo = prefs.backdrop.kind === 'photo' ? prefs.backdrop : null;
  const photoUri = photo ? wallpaperPath(photo.uri) : null;
  const [picking, setPicking] = React.useState(false);

  const choosePhoto = async () => {
    if (picking) return;
    setPicking(true);
    try {
      const imported = await pickWallpaper();
      if (!imported) return;
      const previous = photo?.uri ?? null;
      setPrefs({ backdrop: { kind: 'photo', uri: imported.name, width: imported.width, height: imported.height } });
      if (previous) void removeWallpaper(previous);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
    } catch (e) {
      Alert.alert("Couldn't use that photo", e instanceof Error ? e.message : String(e));
    } finally {
      setPicking(false);
    }
  };

  /** Switch away from the photo (deleting the stored copy) to a gradient or nothing. */
  const setBackdrop = (backdrop: Exclude<Backdrop, { kind: 'photo' }>) => {
    if (photo) void removeWallpaper(photo.uri);
    setPrefs({ backdrop, effect: 'none' });
  };
  const removePhoto = () => setBackdrop({ kind: 'none' });

  const resetAll = () => {
    Alert.alert('Reset appearance?', 'Theme, accent, material and wallpaper go back to their defaults.', [
      { text: 'Cancel', style: 'cancel' },
      { text: 'Reset', style: 'destructive', onPress: () => { if (photo) void removeWallpaper(photo.uri); setPrefs(defaultPrefs); } },
    ]);
  };

  const materialNote =
    t.material === 'opaque' && prefs.surface !== 'opaque'
      ? 'Reduce Transparency is on, so surfaces are solid.'
      : prefs.surface === 'glass' && !liquidGlassSupported
        ? Platform.OS === 'ios' ? 'Liquid Glass needs iOS 26 or later. Showing Acrylic instead.' : 'Liquid Glass is iOS-only. Showing tonal surfaces instead.'
        : surfaceOptions.find((o) => o.id === prefs.surface)?.description;

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Appearance', headerLargeTitle: false, gestureEnabled: !t.hasBackdrop, fullScreenGestureEnabled: false }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content} keyboardDismissMode="on-drag">
        <Preview />

        <Group header="Wallpaper" footer={photo ? 'Your photo is stored inside Cosmos; the original in Photos is untouched.' : 'Pick a photo from your library or one of the gradients below.'}>
          <Row
            icon="photo.on.rectangle"
            iconTint={t.link}
            title={photo ? 'Change Photo' : 'Choose from Photos'}
            subtitle={photo ? `${photo.width ?? '–'} × ${photo.height ?? '–'}` : undefined}
            onPress={() => void choosePhoto()}
            disabled={picking}
            chevron
          />
          {photo ? <Row icon="trash" iconTint={t.danger} title="Remove Photo" danger onPress={removePhoto} /> : null}
        </Group>

        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.rail} style={styles.railWrap}>
          <Tile label="None" selected={prefs.backdrop.kind === 'none'} onPress={() => setBackdrop({ kind: 'none' })}>
            <View style={[styles.fill, { backgroundColor: t.background }]}>
              <View style={[styles.miniCard, { backgroundColor: t.surface }]} />
            </View>
          </Tile>
          {photoUri ? (
            <Tile label="Photo" selected onPress={() => undefined}>
              <Wallpaper uri={photoUri} effect="none" light={t.scheme === 'light'} width={TILE_W} height={TILE_H} />
            </Tile>
          ) : null}
          {backdropPresets.map((p) => (
            <Tile key={p.id} label={p.name} selected={prefs.backdrop.kind === 'preset' && prefs.backdrop.id === p.id} onPress={() => setBackdrop({ kind: 'preset', id: p.id as BackdropPresetId })}>
              <PresetGradient id={p.id} scheme={t.scheme} />
            </Tile>
          ))}
        </ScrollView>

        {photoUri ? (
          <>
            <SectionLabel>Image effect</SectionLabel>
            <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.rail} style={styles.railWrap}>
              {wallpaperEffects.map((e) => (
                <Tile key={e.id} label={e.label} selected={prefs.effect === e.id} onPress={() => setPrefs({ effect: e.id })} accessibilityHint={e.description}>
                  <Wallpaper uri={photoUri} effect={e.id} light={t.scheme === 'light'} width={TILE_W} height={TILE_H} cellDp={e.id === 'ascii' ? 1 : 0.9} />
                </Tile>
              ))}
            </ScrollView>
            <Caption style={styles.footer}>
              {wallpaperEffects.find((e) => e.id === prefs.effect)?.description}
            </Caption>
          </>
        ) : null}

        {t.hasBackdrop ? (
          <Group header="Dim" footer="Darkens the wallpaper toward the theme background so text stays readable.">
            <View style={styles.sliderRow}>
              <Icon name="sun.min" size={16} color={t.textSecondary} />
              <View style={{ flex: 1 }}>
                <Slider label="Wallpaper dim" value={prefs.backdropDim} min={0} max={0.85} step={0.05} onChange={(backdropDim) => setPrefs({ backdropDim })} />
              </View>
              <Icon name="sun.max.fill" size={18} color={t.textSecondary} />
            </View>
          </Group>
        ) : null}

        <Group header="Material" footer={materialNote}>
          <View style={styles.pad}>
            <Segmented<SurfacePref>
              value={prefs.surface}
              onChange={(surface) => setPrefs({ surface })}
              options={surfaceOptions.map((o) => ({ id: o.id, label: o.label }))}
            />
          </View>
        </Group>

        <Group header="Appearance">
          <View style={styles.pad}>
            <Segmented
              value={prefs.appearance}
              onChange={(appearance) => setPrefs({ appearance })}
              options={[
                { id: 'system', label: 'Automatic', icon: 'circle.lefthalf.filled' },
                { id: 'light', label: 'Light', icon: 'sun.max' },
                { id: 'dark', label: 'Dark', icon: 'moon' },
              ]}
            />
          </View>
        </Group>

        <SectionLabel>Theme</SectionLabel>
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.rail} style={styles.railWrap}>
          {families.map((f) => (
            <FamilyCard key={f.id} family={f} selected={prefs.family === f.id} onPress={() => setPrefs({ family: f.id })} />
          ))}
        </ScrollView>

        <Group header="Accent">
          <View style={[styles.pad, styles.accents]}>
            {accentOptions.map((a) => (
              <AccentDot key={a.id} id={a.id} label={a.label} color={accentSwatch(a.id, seeds, t.scheme)} selected={prefs.accent === a.id} onPress={() => setPrefs({ accent: a.id })} />
            ))}
          </View>
        </Group>

        <Group style={{ marginTop: spacing(6) }}>
          <Row title="Reset Appearance" danger onPress={resetAll} />
        </Group>
      </ScrollView>
    </Screen>
  );
}

// ---------------------------------------------------------------------------

/** Live sample of the current theme: a session row, a short exchange and the composer. */
function Preview() {
  const t = useTheme();
  const material = surfaceOptions.find((o) => o.id === t.material)?.label ?? 'Solid';
  const label = `${t.familyName} · ${t.material === 'glass' ? 'Liquid Glass' : material} · ${t.scheme === 'dark' ? 'Dark' : 'Light'}`;
  return (
    <View style={styles.preview} accessibilityLabel={`Preview: ${label}`}>
      <Glass radius={radii.xl} style={styles.previewCard}>
        <View style={styles.previewRow}>
          <View style={[styles.previewMark, { backgroundColor: t.accentMuted }]}>
            <Icon name="sparkles" size={15} color={t.link} />
          </View>
          <View style={{ flex: 1, minWidth: 0 }}>
            <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]} numberOfLines={1}>Refactor the pairing flow</Text>
            <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>Working · MacBook Pro</Text>
          </View>
          <View style={[styles.previewDot, { backgroundColor: t.success }]} />
        </View>
      </Glass>
      <View style={[styles.previewBubble, { backgroundColor: t.userBubble }]}>
        <Text style={[type.subhead, { color: t.userBubbleText }]}>Make the trust step one tap.</Text>
      </View>
      <Text style={[type.subhead, styles.previewAssistant, { color: t.text }]}>Done — the Trust & Link button now confirms inline.</Text>
      <Glass radius={radii.pill} style={styles.previewComposer}>
        <Text style={[type.subhead, { color: t.textTertiary, flex: 1 }]}>Message</Text>
        <View style={[styles.previewSend, { backgroundColor: t.accentStrong }]}>
          <Icon name="arrow.up" size={13} color={t.accentContrast} weight="bold" />
        </View>
      </Glass>
      <Text style={[type.caption, { color: t.textSecondary, textAlign: 'center', marginTop: spacing(2) }]}>{label}</Text>
    </View>
  );
}

const TILE_W = 96;
const TILE_H = 150;

function Tile({ label, selected, onPress, children, accessibilityHint }: {
  label: string; selected: boolean; onPress: () => void; children: React.ReactNode; accessibilityHint?: string;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityHint={accessibilityHint}
      accessibilityState={{ selected }}
      style={({ pressed }) => ({ opacity: pressed ? 0.75 : 1, alignItems: 'center', gap: spacing(1.5) })}
    >
      <View style={[styles.tile, { borderColor: selected ? t.link : 'transparent', backgroundColor: t.fill }]}>
        <View style={styles.tileClip}>{children}</View>
        {selected ? (
          <View style={[styles.tileCheck, { backgroundColor: t.link }]}>
            <Icon name="checkmark" size={10} color={t.accentContrast} weight="bold" />
          </View>
        ) : null}
      </View>
      <Caption style={[type.caption, { fontWeight: selected ? '600' : '400', opacity: selected || t.hasBackdrop ? 1 : 0.85 }]}>{label}</Caption>
    </Pressable>
  );
}

function FamilyCard({ family, selected, onPress }: { family: ThemeFamily; selected: boolean; onPress: () => void }) {
  const t = useTheme();
  const seeds = family[t.scheme] ?? family.dark ?? family.light!;
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      accessibilityLabel={`${family.name} theme`}
      style={({ pressed }) => ({ opacity: pressed ? 0.8 : 1 })}
    >
      <View style={[styles.family, { borderColor: selected ? t.link : 'transparent', backgroundColor: seeds.background }]}>
        <View style={[styles.familyBar, { backgroundColor: seeds.card }]}>
          <View style={[styles.familyDot, { backgroundColor: seeds.accent }]} />
          <View style={[styles.familyLine, { backgroundColor: seeds.text, opacity: 0.85 }]} />
          <View style={[styles.familyLine, { backgroundColor: seeds.muted, width: '45%' }]} />
        </View>
        <View style={styles.familyFoot}>
          <Text style={[type.caption, { color: seeds.text, fontWeight: '600', flex: 1 }]} numberOfLines={1}>{family.name}</Text>
          {!family[t.scheme] ? <Icon name={family.dark ? 'moon.fill' : 'sun.max.fill'} size={10} color={seeds.muted} /> : null}
        </View>
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
          {id === 'theme' ? <Icon name="paintpalette" size={13} color={t.accentContrast} /> : null}
          {id === 'mono' ? <View style={{ width: 12, height: 12, borderRadius: 6, backgroundColor: alpha(t.background, 0.9) }} /> : null}
        </View>
      </View>
      <Text style={[type.caption, { color: selected ? t.text : t.textSecondary }]} numberOfLines={1}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(16) },
  pad: { paddingHorizontal: edge, paddingVertical: spacing(3) },
  footer: { marginTop: spacing(2), marginHorizontal: edge },
  railWrap: { marginHorizontal: -edge, marginTop: spacing(3) },
  rail: { paddingHorizontal: edge, gap: spacing(3) },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  miniCard: { position: 'absolute', left: 10, right: 10, top: 24, height: 28, borderRadius: 8, borderCurve: 'continuous' },
  sliderRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingHorizontal: edge, paddingVertical: spacing(2.5) },

  preview: { marginTop: spacing(4), gap: spacing(2.5) },
  previewCard: { paddingHorizontal: spacing(3.5), paddingVertical: spacing(3) },
  previewRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(3) },
  previewMark: { width: 34, height: 34, borderRadius: 11, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  previewDot: { width: 8, height: 8, borderRadius: 4 },
  previewBubble: { alignSelf: 'flex-end', maxWidth: '78%', paddingHorizontal: spacing(3.5), paddingVertical: spacing(2.25), borderRadius: radii.lg + 2, borderCurve: 'continuous' },
  previewAssistant: { paddingHorizontal: spacing(1) },
  previewComposer: { flexDirection: 'row', alignItems: 'center', paddingLeft: spacing(4), paddingRight: spacing(1.25), height: 44, gap: spacing(2) },
  previewSend: { width: 32, height: 32, borderRadius: 16, alignItems: 'center', justifyContent: 'center' },

  tile: { width: TILE_W, height: TILE_H, borderRadius: radii.lg, borderCurve: 'continuous', borderWidth: 2.5, padding: 1.5 },
  tileClip: { flex: 1, borderRadius: radii.lg - 4, borderCurve: 'continuous', overflow: 'hidden' },
  tileCheck: { position: 'absolute', right: 8, bottom: 8, width: 20, height: 20, borderRadius: 10, alignItems: 'center', justifyContent: 'center' },

  family: { width: 140, borderRadius: radii.lg, borderCurve: 'continuous', borderWidth: 2.5, padding: spacing(2.5), gap: spacing(2), overflow: 'hidden' },
  familyBar: { borderRadius: radii.sm + 2, borderCurve: 'continuous', padding: spacing(2), gap: 5 },
  familyDot: { width: 10, height: 10, borderRadius: 5, marginBottom: 2 },
  familyLine: { height: 4, borderRadius: 2, width: '70%' },
  familyFoot: { flexDirection: 'row', alignItems: 'center', gap: spacing(1) },

  accents: { flexDirection: 'row', flexWrap: 'wrap', rowGap: spacing(3), justifyContent: 'space-between' },
  accent: { width: 56, alignItems: 'center', gap: spacing(1.5) },
  accentRing: { width: 40, height: 40, borderRadius: 20, borderWidth: 2, padding: 3 },
  accentDot: { flex: 1, borderRadius: 16, alignItems: 'center', justifyContent: 'center' },
});

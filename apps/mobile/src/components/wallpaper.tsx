// Photo wallpaper with the desktop's image treatments (Dither, Halftone,
// ASCII, Scanlines) ported from crates/ui/src/new_thread_background_effects.rs
// to SkSL runtime shaders so they run on the GPU at full resolution.

import React, { useEffect, useMemo, useState } from 'react';
import { PixelRatio, StyleSheet, View, useWindowDimensions } from 'react-native';
import { Canvas, Fill, ImageShader, Shader, Skia, type SkImage } from '@shopify/react-native-skia';
import * as FileSystem from 'expo-file-system/legacy';
import type { WallpaperEffect } from '../lib/theme';

const imageCache = new Map<string, SkImage>();

/** Decodes a local photo once and shares the SkImage across every canvas showing it. */
export function usePhotoImage(uri: string | null): SkImage | null {
  const [decoded, setDecoded] = useState<{ uri: string; image: SkImage | null } | null>(null);
  useEffect(() => {
    if (!uri || imageCache.has(uri)) return;
    let live = true;
    (async () => {
      let img: SkImage | null = null;
      try {
        const b64 = await FileSystem.readAsStringAsync(uri, { encoding: 'base64' });
        img = Skia.Image.MakeImageFromEncoded(Skia.Data.fromBase64(b64));
      } catch {
        img = null;
      }
      if (img) {
        imageCache.clear();
        imageCache.set(uri, img);
      }
      if (live) setDecoded({ uri, image: img });
    })();
    return () => { live = false; };
  }, [uri]);
  if (!uri) return null;
  return imageCache.get(uri) ?? (decoded?.uri === uri ? decoded.image : null);
}

const COMMON = `
uniform shader image;
uniform float cell;   // dp per effect cell (snapped to whole device pixels)
uniform float light;  // 1 = light appearance (paper is white), 0 = dark
`;

// 2x2 dots quantised against a 4x4 Bayer matrix on the peak channel.
const DITHER = `${COMMON}
float bayer(float2 p) {
  float x = mod(floor(p.x), 4.0); float y = mod(floor(p.y), 4.0);
  float v = 0.0;
  if (y == 0.0) v = x == 0.0 ? 0.0 : x == 1.0 ? 8.0 : x == 2.0 ? 2.0 : 10.0;
  else if (y == 1.0) v = x == 0.0 ? 12.0 : x == 1.0 ? 4.0 : x == 2.0 ? 14.0 : 6.0;
  else if (y == 2.0) v = x == 0.0 ? 3.0 : x == 1.0 ? 11.0 : x == 2.0 ? 1.0 : 9.0;
  else v = x == 0.0 ? 15.0 : x == 1.0 ? 7.0 : x == 2.0 ? 13.0 : 5.0;
  return v;
}
half4 main(float2 xy) {
  float2 cellXY = floor(xy / cell);
  float2 sampleXY = cellXY * cell + cell * 0.5;
  half4 c = image.eval(sampleXY);
  float peak = max(c.r, max(c.g, c.b));
  float threshold = (bayer(cellXY) + 0.5) / 16.0;
  float gain = peak > threshold ? 1.0 / max(peak, 0.004) : 0.08;
  return half4(c.rgb * half(gain), 1.0);
}
`;

// Colored print dots on paper, 60/40 blended with the source photo.
const HALFTONE = `${COMMON}
half4 main(float2 xy) {
  float size = cell * 2.0;
  float2 cellOrigin = floor(xy / size) * size;
  float2 centre = cellOrigin + size * 0.5;
  half4 dotColor = image.eval(centre);
  float luma = dot(dotColor.rgb, half3(0.299, 0.587, 0.114));
  if (light > 0.5) luma = 1.0 - luma;
  float radius = size * 0.5 * (0.3 + 0.7 * sqrt(luma));
  float dist = length(xy - centre);
  float coverage = clamp(radius + 0.5 - dist, 0.0, 1.0);
  half3 paper = half3(half(light));
  half4 src = image.eval(xy);
  half3 dotLayer = mix(paper, dotColor.rgb, half(coverage));
  return half4(src.rgb * half(0.6) + dotLayer * half(0.4), 1.0);
}
`;

// 5x7 bitmap glyphs in 6x8 cells, brightness → glyph density, ink in the
// sampled colour over paper, 60/40 blended with the source.
const ASCII = `${COMMON}
float glyphRow(float g, float r) {
  // GLYPHS[g][r] from the desktop implementation; rows are 5-bit masks.
  if (g < 0.5) return 0.0;
  if (g < 1.5) return r == 5.0 ? 4.0 : 0.0;
  if (g < 2.5) return (r == 1.0 || r == 4.0) ? 4.0 : 0.0;
  if (g < 3.5) return r == 3.0 ? 14.0 : 0.0;
  if (g < 4.5) return (r == 2.0 || r == 4.0) ? 14.0 : 0.0;
  if (g < 5.5) return r == 3.0 ? 31.0 : (r == 0.0 || r == 6.0) ? 0.0 : 4.0;
  if (g < 6.5) return r == 3.0 ? 31.0 : (r == 1.0 || r == 5.0) ? 21.0 : (r == 2.0 || r == 4.0) ? 14.0 : 0.0;
  if (g < 7.5) return (r == 2.0 || r == 4.0) ? 31.0 : 10.0;
  if (g < 8.5) return (r == 0.0 || r == 6.0) ? 17.0 : r == 1.0 ? 2.0 : (r == 2.0 || r == 3.0) ? 4.0 : r == 4.0 ? 8.0 : 16.0;
  return (r == 0.0 || r == 6.0) ? 14.0 : r == 1.0 ? 17.0 : (r == 2.0 || r == 4.0) ? 23.0 : r == 3.0 ? 21.0 : 16.0;
}
bool bit(float mask, float col) {
  // col 0 is the left-most (bit 4)
  float shifted = floor(mask / pow(2.0, 4.0 - col));
  return mod(shifted, 2.0) >= 1.0;
}
half4 main(float2 xy) {
  float2 cellSize = float2(cell * 6.0, cell * 8.0);
  float2 origin = floor(xy / cellSize) * cellSize;
  float2 centre = origin + cellSize * 0.5;
  half4 sampled = image.eval(centre);
  float luma = dot(sampled.rgb, half3(0.299, 0.587, 0.114));
  float ink = light > 0.5 ? 1.0 - luma : luma;
  float index = floor(sqrt(ink) * 9.0 + 0.0001);
  float2 local = floor((xy - origin) / cell);
  bool on = local.x < 5.0 && local.y < 7.0 && bit(glyphRow(index, local.y), local.x);
  half3 paper = half3(half(light));
  half4 src = image.eval(xy);
  half3 glyph = on ? sampled.rgb : paper;
  return half4(src.rgb * half(0.6) + glyph * half(0.4), 1.0);
}
`;

const SCANLINES = `${COMMON}
half4 main(float2 xy) {
  half4 c = image.eval(xy);
  float row = mod(floor(xy.y / cell), 3.0);
  float gain = row == 0.0 ? 0.52 : 1.0;
  half3 shaded = light > 0.5 ? c.rgb + (half3(1.0) - c.rgb) * half(1.0 - gain) : c.rgb * half(gain);
  return half4(shaded, 1.0);
}
`;

const SOURCES: Record<Exclude<WallpaperEffect, 'none'>, string> = {
  dither: DITHER,
  halftone: HALFTONE,
  ascii: ASCII,
  scanlines: SCANLINES,
};

const effectCache = new Map<string, ReturnType<typeof Skia.RuntimeEffect.Make>>();
function runtimeEffect(effect: Exclude<WallpaperEffect, 'none'>) {
  let fx = effectCache.get(effect);
  if (fx === undefined) {
    fx = Skia.RuntimeEffect.Make(SOURCES[effect]);
    if (!fx) console.warn(`[wallpaper] shader for "${effect}" failed to compile`);
    effectCache.set(effect, fx);
  }
  return fx;
}

export interface WallpaperProps {
  uri: string;
  effect: WallpaperEffect;
  light: boolean;
  /** Fixed size in dp; defaults to the window. */
  width?: number;
  height?: number;
  /** Effect cell size in dp; snapped to whole device pixels. */
  cellDp?: number;
}

/**
 * Renders the photo `cover`-fitted into the given box with the selected
 * treatment. Uses one Skia canvas — a static draw, so it costs nothing after
 * the first frame.
 */
export function Wallpaper({ uri, effect, light, width, height, cellDp = 1.5 }: WallpaperProps) {
  const win = useWindowDimensions();
  const w = width ?? win.width;
  const h = height ?? win.height;
  const image = usePhotoImage(uri);
  const fx = effect === 'none' ? null : runtimeEffect(effect);
  // Skia canvases draw in dp, so cells are authored in dp; snap to whole
  // device pixels so dots stay crisp on 2x/3x displays.
  const cell = Math.max(1, Math.round(cellDp * PixelRatio.get())) / PixelRatio.get();
  const uniforms = useMemo(() => ({ cell, light: light ? 1 : 0 }), [cell, light]);

  if (!image) return <View style={{ width: w, height: h }} />;
  const rect = { x: 0, y: 0, width: w, height: h };
  return (
    <Canvas style={{ width: w, height: h }} pointerEvents="none">
      <Fill>
        {fx ? (
          <Shader source={fx} uniforms={uniforms}>
            <ImageShader image={image} fit="cover" rect={rect} />
          </Shader>
        ) : (
          <ImageShader image={image} fit="cover" rect={rect} />
        )}
      </Fill>
    </Canvas>
  );
}

export const wallpaperStyles = StyleSheet.create({
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
});

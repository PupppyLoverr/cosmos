// Photo wallpapers live in <documents>/wallpaper/. Prefs store the file name
// only — the documents container path changes between installs on iOS.

import * as FileSystem from 'expo-file-system/legacy';
import * as ImagePicker from 'expo-image-picker';
import { ImageManipulator, SaveFormat } from 'expo-image-manipulator';

const DIR = `${FileSystem.documentDirectory ?? ''}wallpaper/`;
/** Longest edge kept for the stored copy; enough for any phone at 3x. */
const MAX_EDGE = 2400;

export interface ImportedPhoto {
  /** File name inside the wallpaper directory. */
  name: string;
  width: number;
  height: number;
}

export function wallpaperPath(name: string): string {
  return name.includes('/') ? name : `${DIR}${name}`;
}

/** Let the user choose a photo, then store a downscaled JPEG copy. */
export async function pickWallpaper(): Promise<ImportedPhoto | null> {
  const perm = await ImagePicker.requestMediaLibraryPermissionsAsync();
  if (!perm.granted) return null;
  const res = await ImagePicker.launchImageLibraryAsync({
    mediaTypes: ['images'],
    quality: 1,
    allowsEditing: false,
    exif: false,
  });
  const asset = res.assets?.[0];
  if (res.canceled || !asset) return null;

  await FileSystem.makeDirectoryAsync(DIR, { intermediates: true }).catch(() => undefined);

  const longest = Math.max(asset.width, asset.height);
  const ctx = ImageManipulator.manipulate(asset.uri);
  if (longest > MAX_EDGE) {
    const scale = MAX_EDGE / longest;
    ctx.resize({ width: Math.round(asset.width * scale), height: Math.round(asset.height * scale) });
  }
  const ref = await ctx.renderAsync();
  const out = await ref.saveAsync({ format: SaveFormat.JPEG, compress: 0.9 });
  ref.release();

  const name = `${Date.now()}.jpg`;
  await FileSystem.moveAsync({ from: out.uri, to: `${DIR}${name}` });
  return { name, width: out.width, height: out.height };
}

export async function removeWallpaper(name: string): Promise<void> {
  await FileSystem.deleteAsync(wallpaperPath(name), { idempotent: true }).catch(() => undefined);
}

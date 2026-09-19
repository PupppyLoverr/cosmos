// Attachments: Camera / Photos / Files / clipboard → base64 chunks → engine
// UploadChunk/UploadCommit → durable path the run request references.

import * as ImagePicker from 'expo-image-picker';
import * as DocumentPicker from 'expo-document-picker';
import * as Clipboard from 'expo-clipboard';
import * as FileSystem from 'expo-file-system/legacy';
import { ImageManipulator, SaveFormat } from 'expo-image-manipulator';

const CHUNK = 256 * 1024;

export interface PickedFile {
  uri: string;
  name: string;
  mimeType?: string;
  size?: number;
}

export async function pickPhoto(useCamera: boolean): Promise<PickedFile | null> {
  if (useCamera) {
    const perm = await ImagePicker.requestCameraPermissionsAsync();
    if (!perm.granted) throw new Error('camera permission denied');
    const res = await ImagePicker.launchCameraAsync({ quality: 0.8 });
    if (res.canceled || !res.assets[0]) return null;
    const a = res.assets[0];
    return { uri: a.uri, name: a.fileName ?? 'photo.jpg', mimeType: a.mimeType ?? 'image/jpeg', size: a.fileSize };
  }
  const perm = await ImagePicker.requestMediaLibraryPermissionsAsync();
  if (!perm.granted) throw new Error('photos permission denied');
  const res = await ImagePicker.launchImageLibraryAsync({ quality: 0.8 });
  if (res.canceled || !res.assets[0]) return null;
  const a = res.assets[0];
  return { uri: a.uri, name: a.fileName ?? 'image', mimeType: a.mimeType ?? 'image/jpeg', size: a.fileSize };
}

export async function pickDocument(): Promise<PickedFile | null> {
  const res = await DocumentPicker.getDocumentAsync({ copyToCacheDirectory: true });
  if (res.canceled || !res.assets[0]) return null;
  const a = res.assets[0];
  return { uri: a.uri, name: a.name, mimeType: a.mimeType ?? undefined, size: a.size };
}

export async function pickFromClipboard(): Promise<PickedFile | null> {
  const img = await Clipboard.getImageAsync({ format: 'png' });
  if (!img?.data) return null;
  // Clipboard images arrive as base64 — stage them to a cache file so the
  // same read-and-chunk path covers every source.
  const uri = `${FileSystem.cacheDirectory}clipboard-${Date.now()}.png`;
  await FileSystem.writeAsStringAsync(uri, img.data, { encoding: FileSystem.EncodingType.Base64 });
  return { uri, name: 'clipboard.png', mimeType: 'image/png' };
}

// Harnesses can't decode HEIC — transcode to JPEG before upload.
export async function ensureDecodable(file: PickedFile): Promise<PickedFile> {
  const lower = (file.mimeType ?? file.name).toLowerCase();
  if (!/hei[cf]|heif/.test(lower)) return file;
  const ctx = ImageManipulator.manipulate(file.uri);
  const img = await ctx.renderAsync();
  const res = await img.saveAsync({ format: SaveFormat.JPEG, compress: 0.85 });
  return { uri: res.uri, name: file.name.replace(/\.hei[cf]$/i, '.jpg'), mimeType: 'image/jpeg' };
}

export async function fileToBase64Chunks(file: PickedFile): Promise<string[]> {
  const normalized = await ensureDecodable(file);
  const b64 = await FileSystem.readAsStringAsync(normalized.uri, { encoding: FileSystem.EncodingType.Base64 });
  const chunks: string[] = [];
  for (let i = 0; i < b64.length; i += CHUNK) chunks.push(b64.slice(i, i + CHUNK));
  if (chunks.length === 0) chunks.push('');
  return chunks;
}

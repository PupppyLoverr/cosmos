import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
const source = readFileSync(new URL('./public/downloads.js', import.meta.url), 'utf8');
const html = readFileSync(new URL('./public/index.html', import.meta.url), 'utf8');
const BASE = 'https://github.com/PupppyLoverr/cosmos-releases/raw/main/artifacts/v0.2.1-alpha';
const FOLDER = 'https://github.com/PupppyLoverr/cosmos-releases/tree/main/artifacts/v0.2.1-alpha';
async function load(navigator, version = '0.2.80', fail = false) {
  const links = Object.fromEntries(['nav-download', 'hero-download', 'closing-download', 'ver'].map(id => [id, { href: '#downloads', setAttribute() {} }]));
  const choices = ['macos', 'windows', 'linux', 'linux-arm'].map(platformDownload => ({ dataset: { platformDownload } }));
  runInNewContext(source, { navigator, document: { getElementById: id => links[id], querySelectorAll: () => choices }, fetch: () => fail ? Promise.reject() : Promise.resolve({ ok: true, text: () => Promise.resolve(version) }) });
  await new Promise(resolve => setImmediate(resolve));
  return { links, choices };
}
for (const [platform, userAgent, expected] of [
  ['MacIntel', 'Macintosh', `${BASE}/cosmos-0.2.80-macos-arm64.dmg`],
  ['Win32', 'Windows NT', FOLDER],
  ['Linux x86_64', 'Linux', FOLDER],
  ['Linux aarch64', 'Linux', FOLDER],
]) test(`desktop download: ${platform}`, async () => {
  const { links, choices } = await load({ platform, userAgent });
  for (const id of ['nav-download', 'hero-download', 'closing-download']) assert.equal(links[id].href, expected);
  assert.equal(choices.length, 4);
});
for (const navigator of [{ platform: 'Linux', userAgent: 'Android' }, { platform: 'MacIntel', maxTouchPoints: 5 }, { platform: 'iPhone' }, {}]) test(`mobile/unknown stays at chooser: ${JSON.stringify(navigator)}`, async () => {
  const { links } = await load(navigator);
  assert.equal(links['hero-download'].href, '#downloads');
});
for (const [version, fail] of [['<script>alert(1)</script>', false], ['0.2.65', false], ['', true]]) test(`safe fallback: ${version}`, async () => {
  const { links } = await load({ platform: 'Win32' }, version, fail);
  assert.equal(links['hero-download'].href, FOLDER);
});
test('HTML retains explicit downloads without JavaScript', () => {
  assert.ok(html.includes(`${BASE}/cosmos-0.2.79-macos-arm64.dmg`));
  assert.ok(html.includes('id="downloads"'));
  assert.ok(html.includes('href="#downloads">All downloads'));
});

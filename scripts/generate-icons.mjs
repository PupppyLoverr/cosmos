#!/usr/bin/env node
// Regenerate every platform app-icon asset from a single square source PNG.
//
//   node scripts/generate-icons.mjs <source.png>
//
// Self-contained — no npm deps, no ffmpeg/ImageMagick. Implements PNG decode
// (8-bit rgb/rgba, non-interlaced), area-average downscale, a superellipse
// ("squircle") alpha mask, a soft drop shadow, PNG encode, and ICO container
// assembly (PNG-compressed entries, Vista+).
//
// Outputs:
//   dist/zeron.png                                   Linux hicolor icon (1024, squircle, transparent corners)
//   dist/macos/icon-1024.png                         macOS icns source: 824 squircle + margins + shadow
//   dist/windows/zeron.ico                           multi-size ico embedded into zeron.exe (build.rs) + Inno
//   apps/ios/.../AppIcon1024.png                     iOS app icon (1024, opaque — Apple applies the mask)
//   apps/landing/public/assets/zeron-app-icon.png    site logo mark (128)
//   apps/landing/public/assets/zeron-favicon-v3.png  site favicon (128)
//   dist/zeron.svg                                   source-art slot, re-rendered with the raster embedded
//   dist/zeron-tray.png                              embedded tray/status-item icon (64) — Linux SNI pixmap + unbundled macOS fallback

import { deflateSync, inflateSync } from "node:zlib";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

// ---------- PNG decode ----------

const CRC_TABLE = (() => {
    const t = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        t[n] = c;
    }
    return t;
})();

function crc32(buf) {
    let c = 0xffffffff;
    for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
}

function paeth(a, b, c) {
    const p = a + b - c;
    const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

/** Decode an 8-bit non-interlaced PNG (color types 0/2/6) into {w,h,data:RGBA}. */
function decodePng(buf) {
    if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error("not a png");
    let pos = 8;
    let ihdr, idat = [];
    while (pos < buf.length) {
        const len = buf.readUInt32BE(pos);
        const type = buf.toString("ascii", pos + 4, pos + 8);
        const body = buf.subarray(pos + 8, pos + 8 + len);
        if (type === "IHDR") ihdr = body;
        else if (type === "IDAT") idat.push(body);
        else if (type === "IEND") break;
        pos += 12 + len;
    }
    const w = ihdr.readUInt32BE(0), h = ihdr.readUInt32BE(4);
    const depth = ihdr[8], colorType = ihdr[9], interlace = ihdr[12];
    if (depth !== 8) throw new Error(`unsupported bit depth ${depth}`);
    if (interlace !== 0) throw new Error("interlaced png not supported");
    const channels = { 0: 1, 2: 3, 6: 4 }[colorType];
    if (!channels) throw new Error(`unsupported color type ${colorType}`);
    const raw = inflateSync(Buffer.concat(idat));
    const stride = w * channels;
    const out = Buffer.alloc(w * h * 4);
    let prev = Buffer.alloc(stride);
    for (let y = 0; y < h; y++) {
        const filter = raw[y * (stride + 1)];
        const row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
        const cur = Buffer.alloc(stride);
        for (let x = 0; x < stride; x++) {
            const a = x >= channels ? cur[x - channels] : 0;
            const b = prev[x];
            const c = x >= channels ? prev[x - channels] : 0;
            let v = row[x];
            switch (filter) {
                case 0: break;
                case 1: v += a; break;
                case 2: v += b; break;
                case 3: v += (a + b) >> 1; break;
                case 4: v += paeth(a, b, c); break;
                default: throw new Error(`bad filter ${filter}`);
            }
            cur[x] = v & 0xff;
        }
        for (let x = 0; x < w; x++) {
            const s = x * channels, d = (y * w + x) * 4;
            out[d] = cur[s];
            out[d + 1] = colorType === 0 ? cur[s] : cur[s + 1];
            out[d + 2] = colorType === 0 ? cur[s] : cur[s + 2];
            out[d + 3] = channels === 4 ? cur[s + 3] : 255;
        }
        prev = cur;
    }
    return { w, h, data: out };
}

// ---------- PNG encode ----------

function chunk(type, body) {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(body.length, 0);
    head.write(type, 4);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])));
    return Buffer.concat([head, body, crc]);
}

/** Encode {w,h,data:RGBA} as a non-interlaced rgba8 PNG. `opaque` drops alpha. */
function encodePng(img, { opaque = false } = {}) {
    const { w, h, data } = img;
    const channels = opaque ? 3 : 4;
    const raw = Buffer.alloc(h * (w * channels + 1));
    for (let y = 0; y < h; y++) {
        const ro = y * (w * channels + 1);
        raw[ro] = 0;
        for (let x = 0; x < w; x++) {
            const s = (y * w + x) * 4, d = ro + 1 + x * channels;
            raw[d] = data[s]; raw[d + 1] = data[s + 1]; raw[d + 2] = data[s + 2];
            if (!opaque) raw[d + 3] = data[s + 3];
        }
    }
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4);
    ihdr[8] = 8; ihdr[9] = opaque ? 2 : 6;
    return Buffer.concat([
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        chunk("IHDR", ihdr),
        chunk("IDAT", deflateSync(raw, { level: 9 })),
        chunk("IEND", Buffer.alloc(0)),
    ]);
}

// ---------- pixel ops ----------

/** Fractional area-average resize (correct for large downscales). */
function resize(img, w2, h2 = w2) {
    const { w: W, h: H, data: src } = img;
    const out = Buffer.alloc(w2 * h2 * 4);
    for (let dy = 0; dy < h2; dy++) {
        const sy0 = (dy * H) / h2, sy1 = ((dy + 1) * H) / h2;
        for (let dx = 0; dx < w2; dx++) {
            const sx0 = (dx * W) / w2, sx1 = ((dx + 1) * W) / w2;
            let r = 0, g = 0, b = 0, a = 0, tot = 0;
            for (let sy = Math.floor(sy0); sy < Math.ceil(sy1); sy++) {
                const wy = Math.min(sy1, sy + 1) - Math.max(sy0, sy);
                if (wy <= 0) continue;
                for (let sx = Math.floor(sx0); sx < Math.ceil(sx1); sx++) {
                    const wx = Math.min(sx1, sx + 1) - Math.max(sx0, sx);
                    if (wx <= 0) continue;
                    const wt = wx * wy, s = (Math.min(sy, H - 1) * W + Math.min(sx, W - 1)) * 4;
                    r += src[s] * wt; g += src[s + 1] * wt; b += src[s + 2] * wt; a += src[s + 3] * wt;
                    tot += wt;
                }
            }
            const d = (dy * w2 + dx) * 4;
            out[d] = r / tot; out[d + 1] = g / tot; out[d + 2] = b / tot; out[d + 3] = a / tot;
        }
    }
    return { w: w2, h: h2, data: out };
}

/**
 * Multiply alpha by a superellipse (squircle) mask covering the whole image.
 * n≈4.5 approximates Apple's continuous-corner shape; ~1.5px AA edge.
 */
function applySquircle(img, n = 4.5) {
    const { w, h, data } = img;
    const cx = (w - 1) / 2, cy = (h - 1) / 2, S = w / 2;
    const ramp = S / (n * 1.5); // ~1.5px coverage transition at the edge midpoint
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            const f = Math.pow(Math.abs((x - cx) / S), n) + Math.pow(Math.abs((y - cy) / S), n);
            const cov = Math.min(1, Math.max(0, (1 - f) * ramp));
            data[(y * w + x) * 4 + 3] = Math.round(data[(y * w + x) * 4 + 3] * cov);
        }
    }
    return img;
}

/** Alpha-over composite of src onto dst at (ox,oy). */
function composite(dst, src, ox, oy) {
    for (let y = 0; y < src.h; y++) {
        for (let x = 0; x < src.w; x++) {
            const dx = x + ox, dy = y + oy;
            if (dx < 0 || dy < 0 || dx >= dst.w || dy >= dst.h) continue;
            const s = (y * src.w + x) * 4, d = (dy * dst.w + dx) * 4;
            const sa = src.data[s + 3] / 255, da = dst.data[d + 3] / 255;
            const oa = sa + da * (1 - sa);
            if (oa === 0) continue;
            for (let c = 0; c < 3; c++) {
                dst.data[d + c] = (src.data[s + c] * sa + dst.data[d + c] * da * (1 - sa)) / oa;
            }
            dst.data[d + 3] = oa * 255;
        }
    }
    return dst;
}

function canvas(w, h = w) {
    return { w, h, data: Buffer.alloc(w * h * 4) };
}

/** Separable box blur of the alpha channel (3 passes ≈ gaussian). */
function boxBlurAlpha(img, r, passes = 3) {
    const { w, h, data } = img;
    for (let p = 0; p < passes; p++) {
        const tmp = Buffer.alloc(data.length);
        for (let y = 0; y < h; y++) {
            let acc = 0;
            for (let x = -r; x <= r; x++) acc += data[(y * w + Math.min(Math.max(x, 0), w - 1)) * 4 + 3];
            for (let x = 0; x < w; x++) {
                tmp[(y * w + x) * 4 + 3] = acc / (2 * r + 1);
                const xo = Math.max(x - r, 0), xn = Math.min(x + r + 1, w - 1);
                acc += data[(y * w + xn) * 4 + 3] - data[(y * w + xo) * 4 + 3];
            }
        }
        for (let x = 0; x < w; x++) {
            let acc = 0;
            for (let y = -r; y <= r; y++) acc += tmp[(Math.min(Math.max(y, 0), h - 1) * w + x) * 4 + 3];
            for (let y = 0; y < h; y++) {
                data[(y * w + x) * 4 + 3] = acc / (2 * r + 1);
                const yo = Math.max(y - r, 0), yn = Math.min(y + r + 1, h - 1);
                acc += tmp[(yn * w + x) * 4 + 3] - tmp[(yo * w + x) * 4 + 3];
            }
        }
    }
    return img;
}

/** Black squircle silhouette (for the macOS drop shadow). */
function squircleSilhouette(size, alpha = 140) {
    const img = canvas(size);
    for (let i = 0; i < size * size; i++) img.data[i * 4 + 3] = alpha;
    return applySquircle(img);
}

// ---------- ICO ----------

function buildIco(pngs /* [{size, buf}] */) {
    const head = Buffer.alloc(6);
    head.writeUInt16LE(1, 2); // type: icon
    head.writeUInt16LE(pngs.length, 4);
    const entries = [];
    let offset = 6 + pngs.length * 16;
    for (const { size, buf } of pngs) {
        const e = Buffer.alloc(16);
        e[0] = size >= 256 ? 0 : size; // width (0 = 256)
        e[1] = size >= 256 ? 0 : size; // height
        e[4] = 1; e[5] = 0;            // planes
        e.writeUInt16LE(32, 6);        // bpp
        e.writeUInt32LE(buf.length, 8);
        e.writeUInt32LE(offset, 12);
        entries.push(e);
        offset += buf.length;
    }
    return Buffer.concat([head, ...entries, ...pngs.map(p => p.buf)]);
}

// ---------- pipeline ----------

const srcPath = process.argv[2];
if (!srcPath) {
    console.error("usage: node scripts/generate-icons.mjs <source.png>");
    process.exit(1);
}
const src = decodePng(readFileSync(srcPath));
console.log(`source: ${src.w}x${src.h}`);

// The source art is a cutout on transparency — flatten it onto black first,
// matching the brand's black-tile icon treatment.
function overBlack(img) {
    const bg = canvas(img.w, img.h);
    for (let i = 0; i < img.w * img.h; i++) bg.data[i * 4 + 3] = 255;
    return composite(bg, img, 0, 0);
}

// Full-bleed black squircle tile — shared by Linux, Windows ico, favicon, site logo.
const tile1024 = applySquircle(overBlack(resize(src, 1024)));

const write = (rel, buf) => {
    const p = join(ROOT, rel);
    mkdirSync(dirname(p), { recursive: true });
    writeFileSync(p, buf);
    console.log(`wrote ${rel} (${(buf.length / 1024).toFixed(0)}KB)`);
};

// Linux hicolor icon.
write("dist/zeron.png", encodePng(tile1024));

// macOS: 824px squircle tile centered at (100,100) on a transparent 1024
// canvas, over a soft drop shadow — the pre-masked input iconutil expects.
const TILE = 824, MARGIN = (1024 - TILE) / 2;
const macIcon = canvas(1024);
const shadow = boxBlurAlpha(squircleSilhouette(TILE), 9);
composite(macIcon, shadow, MARGIN, MARGIN + 12);
composite(macIcon, applySquircle(overBlack(resize(src, TILE))), MARGIN, MARGIN);
write("dist/macos/icon-1024.png", encodePng(macIcon));

// Windows .ico — embedded into zeron.exe by apps/zeron/build.rs and used as
// the Inno Setup installer icon.
const icoSizes = [16, 24, 32, 48, 64, 128, 256];
const icoPngs = icoSizes.map(s => ({ size: s, buf: encodePng(resize(tile1024, s)) }));
write("dist/windows/zeron.ico", buildIco(icoPngs));

// iOS — opaque square; SpringBoard applies its own mask.
write(
    "apps/ios/Zeron/Assets.xcassets/AppIcon.appiconset/AppIcon1024.png",
    encodePng(overBlack(resize(src, 1024)), { opaque: true })
);

// Landing site logo mark + favicon.
write("apps/landing/public/assets/zeron-app-icon.png", encodePng(resize(tile1024, 128)));
write("apps/landing/public/assets/zeron-favicon-v3.png", encodePng(resize(tile1024, 128)));

// Tray/status-item icon — embedded into the binary by the tray backends
// (crates/ui/src/tray): the Linux SNI pixmap and the unbundled macOS fallback.
write("dist/zeron-tray.png", encodePng(resize(tile1024, 64)));

// dist/zeron.svg — the "source artwork" slot. The mark is raster now, so the
// svg wraps a 512px render (keeps the file truthful for anyone opening it).
const embedded = encodePng(resize(tile1024, 512)).toString("base64");
write("dist/zeron.svg", Buffer.from(
    `<svg width="1024" height="1024" viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">\n` +
    `<image width="1024" height="1024" href="data:image/png;base64,${embedded}"/>\n</svg>\n`
));

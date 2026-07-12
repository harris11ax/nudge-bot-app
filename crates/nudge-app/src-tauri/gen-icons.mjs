// One-shot: generate the minimal icon set Tauri needs (solid rounded-blue tile,
// matching the tray motif) so `tauri dev`/`build` have valid icons without the
// `tauri icon` pipeline. Pure Node (zlib) — no image deps. Re-run any time:
//   node gen-icons.mjs
// Replace with `npm run tauri icon <art.png>` once real branding art exists.
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "icons");
mkdirSync(OUT, { recursive: true });

const BLUE = [37, 99, 235, 255]; // #2563eb, opaque
const CORNER = 0.18; // corner-radius fraction → rounded tile, transparent corners

function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return (~c) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td), 0);
  return Buffer.concat([len, td, crc]);
}

// Solid rounded RGBA tile of side `n`.
function rgba(n) {
  const r = CORNER * n;
  const px = Buffer.alloc(n * n * 4);
  const inside = (x, y) => {
    // rounded-rect: outside only in the four corner quadrants beyond radius r.
    const cx = x < r ? r - x : x > n - 1 - r ? x - (n - 1 - r) : 0;
    const cy = y < r ? r - y : y > n - 1 - r ? y - (n - 1 - r) : 0;
    return cx * cx + cy * cy <= r * r;
  };
  for (let y = 0; y < n; y++)
    for (let x = 0; x < n; x++) {
      const o = (y * n + x) * 4;
      if (inside(x, y)) {
        px[o] = BLUE[0]; px[o + 1] = BLUE[1]; px[o + 2] = BLUE[2]; px[o + 3] = 255;
      } // else stays transparent (all zero)
    }
  return px;
}

function pngBytes(n) {
  const raw = rgba(n);
  // Prepend filter byte (0) per scanline.
  const stride = n * 4;
  const filtered = Buffer.alloc((stride + 1) * n);
  for (let y = 0; y < n; y++) {
    filtered[y * (stride + 1)] = 0;
    raw.copy(filtered, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(n, 0);
  ihdr.writeUInt32BE(n, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 6;  // color type RGBA
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  return Buffer.concat([
    sig,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(filtered)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ICO wrapping PNG-encoded images (ICO supports embedded PNG, size byte 0 = 256).
function ico(sizes) {
  const imgs = sizes.map((s) => pngBytes(s));
  const count = imgs.length;
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(count, 4);
  const dir = Buffer.alloc(16 * count);
  let offset = 6 + 16 * count;
  sizes.forEach((s, i) => {
    const b = i * 16;
    dir[b] = s >= 256 ? 0 : s;
    dir[b + 1] = s >= 256 ? 0 : s;
    dir[b + 2] = 0; dir[b + 3] = 0;
    dir.writeUInt16LE(1, b + 4);
    dir.writeUInt16LE(32, b + 6);
    dir.writeUInt32LE(imgs[i].length, b + 8);
    dir.writeUInt32LE(offset, b + 12);
    offset += imgs[i].length;
  });
  return Buffer.concat([header, dir, ...imgs]);
}

const targets = {
  "32x32.png": 32,
  "128x128.png": 128,
  "128x128@2x.png": 256,
  "icon.png": 512,
};
for (const [name, n] of Object.entries(targets)) {
  writeFileSync(join(OUT, name), pngBytes(n));
  console.log("wrote", name);
}
writeFileSync(join(OUT, "icon.ico"), ico([16, 32, 48, 256]));
console.log("wrote icon.ico");

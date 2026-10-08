'use strict';
// Reading one file out of a release zip (stored or deflated, zip64 too),
// with its CRC-32 checked. The whole zip is in memory: a release is a few MB.

const zlib = require('zlib');

const SIG_EOCD = 0x06054b50;
const SIG_ZIP64_LOCATOR = 0x07064b50;
const SIG_ZIP64_EOCD = 0x06064b50;
const SIG_CENTRAL = 0x02014b50;
const SIG_LOCAL = 0x04034b50;

let crcTable = null;
function crc32(buf) {
  if (!crcTable) {
    crcTable = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c >>> 0;
    }
  }
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) crc = crcTable[(crc ^ buf[i]) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function bad(why) {
  return new Error(`the downloaded zip is damaged (${why})`);
}

function u64(buf, at) {
  const v = buf.readBigUInt64LE(at);
  if (v > BigInt(Number.MAX_SAFE_INTEGER)) throw bad('a size too large');
  return Number(v);
}

/** Every file in the zip's central directory: {name, method, crc, ...}. */
function entries(buf) {
  let eocd = -1;
  for (let i = buf.length - 22; i >= Math.max(0, buf.length - 22 - 0xffff); i--) {
    if (buf.readUInt32LE(i) === SIG_EOCD) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw bad('no end of central directory');
  let count = buf.readUInt16LE(eocd + 10);
  let cdOffset = buf.readUInt32LE(eocd + 16);
  if (count === 0xffff || cdOffset === 0xffffffff) {
    const loc = eocd - 20;
    if (loc < 0 || buf.readUInt32LE(loc) !== SIG_ZIP64_LOCATOR) throw bad('no zip64 locator');
    const z = u64(buf, loc + 8);
    if (z + 56 > buf.length || buf.readUInt32LE(z) !== SIG_ZIP64_EOCD) throw bad('no zip64 end record');
    count = u64(buf, z + 32);
    cdOffset = u64(buf, z + 48);
  }
  const list = [];
  let p = cdOffset;
  for (let i = 0; i < count; i++) {
    if (p + 46 > buf.length || buf.readUInt32LE(p) !== SIG_CENTRAL) throw bad('a broken directory entry');
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    const e = {
      flags: buf.readUInt16LE(p + 8),
      method: buf.readUInt16LE(p + 10),
      crc: buf.readUInt32LE(p + 16),
      compressedSize: buf.readUInt32LE(p + 20),
      size: buf.readUInt32LE(p + 24),
      offset: buf.readUInt32LE(p + 42),
      name: buf.toString('utf8', p + 46, p + 46 + nameLen),
    };
    // Sizes and offset too large for 32 bits are in the zip64 extra field,
    // in this order, each only when its 32-bit field is all ones.
    let x = p + 46 + nameLen;
    const xEnd = x + extraLen;
    while (x + 4 <= xEnd) {
      const id = buf.readUInt16LE(x);
      const len = buf.readUInt16LE(x + 2);
      if (id === 0x0001) {
        let q = x + 4;
        for (const k of ['size', 'compressedSize', 'offset']) {
          if (e[k] === 0xffffffff && q + 8 <= x + 4 + len) {
            e[k] = u64(buf, q);
            q += 8;
          }
        }
      }
      x += 4 + len;
    }
    list.push(e);
    p += 46 + nameLen + extraLen + commentLen;
  }
  return list;
}

/** The bytes of one entry, checked against its CRC-32. */
function read(buf, e) {
  if (e.flags & 1) throw bad(`${e.name} is encrypted`);
  if (e.offset + 30 > buf.length || buf.readUInt32LE(e.offset) !== SIG_LOCAL) throw bad(`no header for ${e.name}`);
  const start = e.offset + 30 + buf.readUInt16LE(e.offset + 26) + buf.readUInt16LE(e.offset + 28);
  if (start + e.compressedSize > buf.length) throw bad(`${e.name} is cut short`);
  const raw = buf.subarray(start, start + e.compressedSize);
  let data;
  if (e.method === 0) data = Buffer.from(raw);
  else if (e.method === 8) {
    try {
      data = zlib.inflateRawSync(raw);
    } catch (err) {
      throw bad(`${e.name} does not unpack: ${err.message}`);
    }
  } else throw bad(`${e.name} is packed with method ${e.method}`);
  if (data.length !== e.size) throw bad(`${e.name} has ${data.length} bytes, not ${e.size}`);
  if (crc32(data) !== e.crc) throw bad(`${e.name} fails its CRC-32`);
  return data;
}

/**
 * The file called `fileName` (in any folder; the least deep one wins), or
 * null when the zip has none.
 */
function extractFile(buf, fileName) {
  const found = entries(buf)
    .map((e) => ({ e, parts: e.name.replace(/\\/g, '/').replace(/^\.\//, '').split('/') }))
    .filter(({ e, parts }) => !e.name.endsWith('/') && parts[parts.length - 1] === fileName)
    .sort((a, b) => a.parts.length - b.parts.length);
  return found.length ? read(buf, found[0].e) : null;
}

module.exports = { crc32, entries, extractFile };

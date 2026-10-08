'use strict';
// Versions: reading them (from the plugin's manifest, from the installed
// program) and putting them in order.

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const VERSION_RE = /(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/;

/** "v5.0.1-preview" → {major, minor, patch, pre: "preview"}, or null. */
function parse(text) {
  const m = VERSION_RE.exec(String(text || ''));
  if (!m) return null;
  return { major: +m[1], minor: +m[2], patch: +m[3], pre: m[4] || '', text: m[0] };
}

function comparePre(a, b) {
  // A release comes after its pre-releases.
  if (a === b) return 0;
  if (!a) return 1;
  if (!b) return -1;
  const x = a.split('.');
  const y = b.split('.');
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if (x[i] === undefined) return -1;
    if (y[i] === undefined) return 1;
    const nx = /^\d+$/.test(x[i]);
    const ny = /^\d+$/.test(y[i]);
    if (nx && ny) {
      if (+x[i] !== +y[i]) return +x[i] < +y[i] ? -1 : 1;
    } else if (nx !== ny) {
      return nx ? -1 : 1;
    } else if (x[i] !== y[i]) {
      return x[i] < y[i] ? -1 : 1;
    }
  }
  return 0;
}

/** <0, 0 or >0 as a is older than, the same as, or newer than b. */
function compare(a, b) {
  const x = typeof a === 'string' ? parse(a) : a;
  const y = typeof b === 'string' ? parse(b) : b;
  for (const k of ['major', 'minor', 'patch']) {
    if (x[k] !== y[k]) return x[k] < y[k] ? -1 : 1;
  }
  return comparePre(x.pre, y.pre);
}

/** The version in the plugin's .claude-plugin/plugin.json, or null. */
function pluginVersion(pluginRoot) {
  try {
    const manifest = JSON.parse(
      fs.readFileSync(path.join(pluginRoot, '.claude-plugin', 'plugin.json'), 'utf8'),
    );
    return parse(manifest.version) ? manifest.version : null;
  } catch {
    return null;
  }
}

/**
 * Runs `<bin> --version` and returns the version it prints, or
 * {error} when the file is missing or doesn't run.
 */
function askVersion(bin) {
  const r = spawnSync(bin, ['--version'], {
    encoding: 'utf8',
    timeout: 20000,
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (r.error) return { error: r.error.code === 'ENOENT' ? 'not installed' : r.error.message };
  const v = parse(r.stdout);
  if (r.status !== 0 || !v) {
    const said = `${r.stdout || ''}${r.stderr || ''}`.trim().split('\n').slice(-3).join(' | ');
    return { error: `\`${path.basename(bin)} --version\` failed (exit ${r.status})${said ? ': ' + said : ''}` };
  }
  return { version: v.text };
}

/**
 * The installed program's version, without running it when it is the same
 * file as last time (its size and time are kept in a stamp next to it).
 */
function installedVersion(bin) {
  const st = fs.statSync(bin, { throwIfNoEntry: false });
  if (!st || !st.isFile()) return { error: 'not installed' };
  const stampFile = path.join(path.dirname(bin), '.launcher-stamp.json');
  try {
    const stamp = JSON.parse(fs.readFileSync(stampFile, 'utf8'));
    if (stamp.size === st.size && stamp.mtimeMs === st.mtimeMs && parse(stamp.version)) {
      return { version: stamp.version };
    }
  } catch {
    // No stamp yet, or an unreadable one: ask the program.
  }
  const asked = askVersion(bin);
  if (asked.version) writeStamp(bin, asked.version);
  return asked;
}

function writeStamp(bin, version) {
  try {
    const st = fs.statSync(bin);
    fs.writeFileSync(
      path.join(path.dirname(bin), '.launcher-stamp.json'),
      JSON.stringify({ size: st.size, mtimeMs: st.mtimeMs, version }) + '\n',
    );
  } catch {
    // Only saves a `--version` next time.
  }
}

module.exports = { parse, compare, pluginVersion, askVersion, installedVersion, writeStamp };

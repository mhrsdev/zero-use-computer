'use strict';
// Installing the program: one installer at a time (a lock folder), the zip
// downloaded (resumed when cut off), checked against GitHub's SHA-256, the
// program taken out of it, run once to see it works here, and only then
// moved into place — never written over a program that is running.

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const { log } = require('./log.cjs');
const { download } = require('./net.cjs');
const { findAsset } = require('./github.cjs');
const { extractFile } = require('./zip.cjs');
const { askVersion, writeStamp, parse } = require('./version.cjs');
const { IS_WINDOWS, BIN_NAME, homeDir, binDir } = require('./platform.cjs');

/** An installer that has said nothing for this long is taken as gone. */
const STALE_LOCK_MS = 30 * 60 * 1000;

function statusFile() {
  return path.join(homeDir(), 'launcher-status.json');
}

/** What the installer is doing, for the launcher that waits on it. */
function writeStatus(status) {
  const file = statusFile();
  const tmp = `${file}.${process.pid}.tmp`;
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(tmp, JSON.stringify({ ...status, pid: process.pid, at: Date.now() }) + '\n');
    fs.renameSync(tmp, file);
  } catch {
    try {
      fs.rmSync(tmp, { force: true });
    } catch {
      // Only progress is lost.
    }
  }
}

function readStatus() {
  try {
    return JSON.parse(fs.readFileSync(statusFile(), 'utf8'));
  } catch {
    return null;
  }
}

function pidAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === 'EPERM';
  }
}

function lockDir() {
  return path.join(binDir(), '.install.lock');
}

/** Takes the install lock; false when a live installer holds it. */
function tryLock() {
  const dir = lockDir();
  fs.mkdirSync(path.dirname(dir), { recursive: true });
  for (let i = 0; i < 2; i++) {
    try {
      fs.mkdirSync(dir);
      fs.writeFileSync(path.join(dir, 'owner.json'), JSON.stringify({ pid: process.pid, at: Date.now() }));
      return true;
    } catch (e) {
      if (e.code !== 'EEXIST') throw e;
    }
    let owner = null;
    try {
      owner = JSON.parse(fs.readFileSync(path.join(dir, 'owner.json'), 'utf8'));
    } catch {
      // Being written, or left half-written by an installer that died.
    }
    const age = Date.now() - (owner?.at || fs.statSync(dir, { throwIfNoEntry: false })?.mtimeMs || 0);
    if (owner ? pidAlive(owner.pid) && age < STALE_LOCK_MS : age < 10000) return false;
    log(`taking over the install lock of an installer that is gone (pid ${owner?.pid ?? '?'})`);
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return false;
}

function unlock() {
  try {
    const owner = JSON.parse(fs.readFileSync(path.join(lockDir(), 'owner.json'), 'utf8'));
    if (owner.pid !== process.pid) return;
  } catch {
    return;
  }
  fs.rmSync(lockDir(), { recursive: true, force: true });
}

async function waitForUnlock() {
  while (fs.existsSync(lockDir())) {
    if (tryLock()) {
      unlock();
      return;
    }
    await new Promise((r) => setTimeout(r, 500));
  }
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function renameSoon(from, to, tries = 20) {
  // Windows: a virus scanner may hold a new program for a moment.
  for (let i = 1; ; i++) {
    try {
      fs.renameSync(from, to);
      return;
    } catch (e) {
      if (!IS_WINDOWS || i >= tries || !['EPERM', 'EBUSY', 'EACCES'].includes(e.code)) throw e;
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
    }
  }
}

/** Moves `tmp` to `final`, setting a running (locked) program aside on Windows. */
function place(tmp, final) {
  const dir = path.dirname(final);
  for (const f of fs.readdirSync(dir)) {
    if (f.startsWith(`${BIN_NAME}.old-`)) {
      try {
        fs.rmSync(path.join(dir, f), { force: true });
      } catch {
        // Still running: removed another time.
      }
    }
  }
  try {
    renameSoon(tmp, final, 4);
  } catch (e) {
    if (!IS_WINDOWS || !fs.existsSync(final)) throw e;
    // A running server locks its program, but it may be renamed: the
    // new one is used from the next start.
    renameSoon(final, `${final}.old-${Date.now()}`);
    renameSoon(tmp, final);
  }
}

/**
 * Installs release v<wanted> (or the latest, when there is no such
 * release) for `targetName`. Resolves with {bin, version}.
 */
async function install({ wanted, targetName }) {
  const dir = binDir();
  const final = path.join(dir, BIN_NAME);
  fs.mkdirSync(dir, { recursive: true });

  writeStatus({ phase: 'looking', wanted });
  const asset = await findAsset(wanted, targetName);
  if (!asset.exact) log(`there is no release v${wanted} on GitHub (yet): installing ${asset.tag}, the latest`);
  log(`downloading ${asset.name} of ${asset.tag}`);

  const downloads = path.join(homeDir(), 'downloads');
  fs.mkdirSync(downloads, { recursive: true });
  const part = path.join(downloads, `${asset.tag}-${asset.name}.part`);
  let lastWrite = 0;
  await download(asset.url, part, {
    onProgress: (have, total) => {
      const now = Date.now();
      if (now - lastWrite < 400 && have !== total) return;
      lastWrite = now;
      writeStatus({ phase: 'downloading', version: asset.version, have, total });
    },
  });

  writeStatus({ phase: 'checking', version: asset.version });
  const got = sha256(part);
  if (got !== asset.sha256) {
    fs.rmSync(part, { force: true });
    throw new Error(`the download's SHA-256 is ${got}, the release says ${asset.sha256}: not installed`);
  }
  const program = extractFile(fs.readFileSync(part), BIN_NAME);
  if (!program) throw new Error(`${asset.name} has no ${BIN_NAME} in it`);

  // Windows runs only a file named .exe.
  const tmp = path.join(dir, IS_WINDOWS ? `computer-use-mcp.new-${process.pid}.exe` : `${BIN_NAME}.new-${process.pid}`);
  fs.writeFileSync(tmp, program, { mode: 0o755 });
  fs.chmodSync(tmp, 0o755);
  const asked = askVersion(tmp);
  if (!asked.version || parse(asked.version).text !== parse(asset.version).text) {
    fs.rmSync(tmp, { force: true });
    const hint =
      process.platform === 'linux'
        ? ' (it needs the D-Bus and XCB libraries: libdbus-1-3, libxcb1)'
        : '';
    throw new Error(
      asked.version
        ? `the downloaded program says it is ${asked.version}, not ${asset.version}`
        : `the downloaded program does not run on this computer${hint}: ${asked.error}`,
    );
  }

  writeStatus({ phase: 'installing', version: asset.version });
  place(tmp, final);
  writeStamp(final, asset.version);
  fs.rmSync(part, { force: true });
  for (const f of fs.readdirSync(downloads)) {
    if (f.endsWith('.part') && f.includes(asset.name)) fs.rmSync(path.join(downloads, f), { force: true });
  }
  log(`installed ${final} (${asset.version})`);
  writeStatus({ phase: 'done', version: asset.version });
  return { bin: final, version: asset.version };
}

/**
 * The installer as its own process (`main.cjs --install`): takes the lock,
 * or waits for the installer that holds it. Resolves with the exit code.
 */
async function runInstaller({ wanted, targetName }) {
  if (!tryLock()) {
    log('another installer is at work: waiting for it');
    await waitForUnlock();
    return 0;
  }
  const release = () => unlock();
  process.on('exit', release);
  try {
    await install({ wanted, targetName });
    return 0;
  } catch (e) {
    log(`install failed: ${e.message}`);
    writeStatus({ phase: 'failed', error: e.message });
    return 1;
  } finally {
    release();
  }
}

module.exports = { install, runInstaller, readStatus, writeStatus, lockDir, pidAlive };

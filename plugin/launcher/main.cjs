#!/usr/bin/env node
'use strict';
// Zero Use Computer's launcher: what the plugin's .mcp.json starts.
//
// The MCP server is a native program (computer-use-mcp, written in Rust),
// one build per system, published on this repository's GitHub releases.
// This launcher, plain Node with no dependencies, makes sure it is
// installed and then runs it:
//
//   1. The program in ~/.computer-use/bin (or $COMPUTER_USE_HOME/bin; the
//      folder install.sh / install.cmd and the program's own updater use)
//      is asked its version. If it is at least this plugin's version, it is
//      run at once, with the client's stdin and stdout handed straight to it.
//   2. If it is older, it still runs now, and the plugin's version is
//      installed alongside, in the background, for the next start.
//   3. If it is missing, it is installed: the release zip for this system
//      is downloaded from GitHub, checked against the SHA-256 GitHub gives
//      for it, unpacked, run once (`--version`) and moved into place. While
//      that runs, a stand-in answers the client (see shim.cjs).
//
// Settings (environment):
//   COMPUTER_USE_MCP_BIN   run this program instead; nothing is installed
//   COMPUTER_USE_HOME      the program's folder (default ~/.computer-use)
//   HTTPS_PROXY, NO_PROXY  a proxy for GitHub (http:// or https://, no password)
//
// `node main.cjs --install` installs (or updates) the program and exits.

const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

const MIN_NODE_MAJOR = 18;
const major = Number(process.versions.node.split('.')[0]);
if (major < MIN_NODE_MAJOR) {
  process.stderr.write(
    `Zero Use Computer's launcher needs Node.js ${MIN_NODE_MAJOR} or newer; this is ${process.version}. ` +
      'Update Node.js, or install the program by hand (see the plugin README).\n',
  );
  process.exit(1);
}

const { log, setLogFile } = require('./log.cjs');
const { target, homeDir, binPath } = require('./platform.cjs');
const { compare, pluginVersion, installedVersion } = require('./version.cjs');
const { runInstaller, readStatus, lockDir, pidAlive } = require('./install.cjs');
const { runShim } = require('./shim.cjs');

const PLUGIN_ROOT = path.resolve(__dirname, '..');
const LAUNCHER_VERSION = pluginVersion(PLUGIN_ROOT) || '0.0.0';
/** How long the client is kept waiting before the stand-in answers it. */
const GRACE_MS = Number(process.env.ZERO_LAUNCHER_GRACE_MS) || 12000;

function forwardSignals(child) {
  for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    try {
      process.on(sig, () => {
        try {
          child.kill(sig);
        } catch {
          // Already gone.
        }
      });
    } catch {
      // A signal this system doesn't have.
    }
  }
}

/** Runs the program with the client's stdin/stdout/stderr, and ends with it. */
function runProgram(bin, args) {
  const child = spawn(bin, args, { stdio: 'inherit', windowsHide: true });
  forwardSignals(child);
  child.on('error', (e) => {
    log(`could not start ${bin}: ${e.message}`);
    process.exit(1);
  });
  child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
}

/**
 * Starts `main.cjs --install` as its own process, so an install the client
 * gave up waiting for still finishes (and the next start finds it done).
 * Resolves when it ends.
 */
function startInstaller(wanted) {
  const args = [__filename, '--install'];
  if (wanted) args.push(wanted);
  const child = spawn(process.execPath, args, {
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  });
  child.unref();
  return new Promise((resolve) => {
    child.on('error', (e) => resolve({ code: 1, error: `the installer did not start: ${e.message}` }));
    child.on('exit', (code) => resolve({ code }));
  });
}

function mb(bytes) {
  return (bytes / 1048576).toFixed(1);
}

/** setup_status's text, from the installer's status file. */
function statusText(bin, failure) {
  const s = readStatus();
  const where = `The program goes to ${bin}; the launcher's log is ${path.join(homeDir(), 'launcher.log')}.`;
  const byHand =
    'To install it by hand: download computer-use-mcp-<your system>.zip from ' +
    'https://github.com/mhrsdev/zero-use-computer/releases/latest, unzip it, run install.cmd -NoRegister ' +
    '(Windows) or ./install.sh --no-register (macOS, Linux), then reconnect this server (/mcp).';
  if (failure || s?.phase === 'failed') {
    return { failed: true, text: `The install failed: ${failure || s.error}\n\n${byHand}\n${where}` };
  }
  switch (s?.phase) {
    case 'downloading': {
      const pct = s.total ? ` (${Math.floor((100 * s.have) / s.total)}%)` : '';
      return { text: `Downloading computer-use-mcp ${s.version}: ${mb(s.have)} of ${mb(s.total)} MB${pct}. ${where}` };
    }
    case 'checking':
      return { text: `Checking the download of computer-use-mcp ${s.version} against GitHub's SHA-256. ${where}` };
    case 'installing':
    case 'done':
      return { text: `Installing computer-use-mcp ${s.version}; its tools appear in a moment. ${where}` };
    default:
      return { text: `Looking up the release to install on GitHub. ${where}` };
  }
}

async function serve(args) {
  const override = process.env.COMPUTER_USE_MCP_BIN;
  if (override) return runProgram(override, args);

  const bin = binPath();
  const wanted = pluginVersion(PLUGIN_ROOT);
  const have = installedVersion(bin);
  const build = target();

  if (have.version) {
    if (!wanted || compare(have.version, wanted) >= 0) return runProgram(bin, args);
    if (build.name && !installerAtWork()) {
      log(`the installed program is ${have.version}, this plugin is ${wanted}: updating it in the background`);
      startInstaller(wanted);
    }
    return runProgram(bin, args);
  }

  log(`computer-use-mcp is not ready here (${have.error}): installing it`);
  const installed = build.name
    ? startInstaller(wanted).then(({ error }) => {
        const now = installedVersion(bin);
        if (now.version) return { ok: true };
        return { ok: false, error: error || readStatus()?.error || now.error };
      })
    : Promise.resolve({ ok: false, error: build.reason });
  runShim({
    start: () => {
      const child = spawn(bin, args, { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true });
      forwardSignals(child);
      return child;
    },
    installed,
    status: (failure) => statusText(bin, failure),
    graceMs: GRACE_MS,
    version: LAUNCHER_VERSION,
  });
}

/** True while a live installer holds the lock (another chat installing). */
function installerAtWork() {
  try {
    const owner = JSON.parse(fs.readFileSync(path.join(lockDir(), 'owner.json'), 'utf8'));
    return pidAlive(owner.pid);
  } catch {
    return false;
  }
}

async function main() {
  setLogFile(path.join(homeDir(), 'launcher.log'));
  const argv = process.argv.slice(2);
  if (argv[0] === '--install') {
    const build = target();
    if (!build.name) {
      log(build.reason);
      process.exit(1);
    }
    process.exit(await runInstaller({ wanted: argv[1] || pluginVersion(PLUGIN_ROOT), targetName: build.name }));
  }
  await serve(argv);
}

main().catch((e) => {
  log(`launcher error: ${e.stack || e.message}`);
  process.exit(1);
});

'use strict';
// Which build of the program this computer runs, and where it lives.
// The folder is the one install.sh / install.cmd and the program's own
// updater use: $COMPUTER_USE_HOME, else ~/.computer-use (as the program
// reads it: empty counts as unset, a leading ~ is the user's home, a
// relative path is taken in the home).

const os = require('os');
const path = require('path');

const IS_WINDOWS = process.platform === 'win32';
const BIN_NAME = IS_WINDOWS ? 'computer-use-mcp.exe' : 'computer-use-mcp';

/** True when this Node runs as x64 on an Apple-silicon Mac (Rosetta). */
function underRosetta() {
  if (process.platform !== 'darwin' || process.arch !== 'x64') return false;
  try {
    const cpus = os.cpus();
    return cpus.length > 0 && /apple/i.test(cpus[0].model);
  } catch {
    return false;
  }
}

/**
 * The release build for this computer ("windows-x64", "macos-arm64", ...),
 * or null with the reason when there is none.
 */
function target(platform = process.platform, arch = process.arch, rosetta = underRosetta()) {
  if (platform === 'win32') {
    // Windows on Arm runs x64 programs through its own emulation.
    if (arch === 'x64' || arch === 'arm64') return { name: 'windows-x64' };
  } else if (platform === 'darwin') {
    if (arch === 'arm64' || rosetta) return { name: 'macos-arm64' };
    if (arch === 'x64') return { name: 'macos-x64' };
  } else if (platform === 'linux') {
    if (arch === 'x64') return { name: 'linux-x64' };
  }
  return {
    name: null,
    reason:
      `there is no build of computer-use-mcp for ${platform}-${arch} ` +
      '(there are Windows x64, macOS arm64 and x64, Linux x64); build it from source: ' +
      'cargo build --release -p computer-use-mcp, and set COMPUTER_USE_MCP_BIN to it',
  };
}

/** $COMPUTER_USE_HOME as the program reads it, else ~/.computer-use. */
function homeDir(env = process.env, userHome = os.homedir()) {
  const user = userHome || os.tmpdir();
  const value = env.COMPUTER_USE_HOME;
  if (!value) return path.join(user, '.computer-use');
  let p = value;
  if (p === '~') p = user;
  else if (p.startsWith('~/') || p.startsWith('~\\')) p = path.join(user, p.slice(2));
  return path.isAbsolute(p) ? p : path.join(user, p);
}

function binDir(env) {
  return path.join(homeDir(env), 'bin');
}

function binPath(env) {
  return path.join(binDir(env), BIN_NAME);
}

module.exports = { IS_WINDOWS, BIN_NAME, target, homeDir, binDir, binPath };

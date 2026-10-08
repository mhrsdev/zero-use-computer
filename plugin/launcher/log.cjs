'use strict';
// Messages go to stderr (stdout carries MCP's JSON-RPC and nothing else) and
// to a small log file in the program's folder, so a failed first start can
// be looked at afterwards.

const fs = require('fs');
const path = require('path');

const MAX_LOG_BYTES = 512 * 1024;
let logFile = null;

function setLogFile(file) {
  logFile = file;
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const st = fs.statSync(file, { throwIfNoEntry: false });
    if (st && st.size > MAX_LOG_BYTES) fs.renameSync(file, file + '.old');
  } catch {
    // A log that can't be written never stops the launcher.
  }
}

function log(message) {
  const line = `[zero-launcher ${process.pid}] ${message}`;
  try {
    process.stderr.write(line + '\n');
  } catch {
    // stderr closed: nothing to do.
  }
  if (!logFile) return;
  try {
    fs.appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`);
  } catch {
    // As above.
  }
}

module.exports = { log, setLogFile };

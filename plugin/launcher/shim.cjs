'use strict';
// While the program is installed for the first time, Claude must not wait
// for an answer it may give up on (an MCP server has about 30 s to start).
// So this stands in for the server: it answers `initialize` itself, offers
// one tool that tells how far the install is, and as soon as the program is
// ready it starts it, introduces it with the client's own `initialize`, and
// from then on only passes bytes both ways. The client is told the tools
// (and prompts, and resources) changed, and asks for the real ones.
//
// When the program is ready within the first seconds, nothing stands in at
// all: what the client sent so far is handed to the program as it came.

const { spawn } = require('child_process');
const { log } = require('./log.cjs');

const PROTOCOL_VERSIONS = ['2025-06-18', '2025-03-26', '2024-11-05'];
const HANDOVER_ID = 'zero-launcher-handover';
const STATUS_TOOL = {
  name: 'setup_status',
  title: 'Zero Use Computer: install progress',
  description:
    "Tells how far the first install of Zero Use Computer's program (computer-use-mcp) is. " +
    "The desktop tools take this tool's place as soon as the program is ready.",
  inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  annotations: { readOnlyHint: true, openWorldHint: false },
};
const INSTRUCTIONS =
  "Zero Use Computer's program (computer-use-mcp) is being downloaded and installed on this computer " +
  'for the first time. Its desktop tools appear in this server\'s tool list as soon as it is ready, ' +
  'usually within a minute; until then the only tool is setup_status, which says how far the install is ' +
  'and what went wrong if it failed. If the tools have not appeared after it says the install is done, ' +
  'reconnect the server (/mcp).';

function negotiate(requested) {
  return PROTOCOL_VERSIONS.includes(requested) ? requested : PROTOCOL_VERSIONS[0];
}

/** Splits a byte stream into lines (MCP's stdio framing). */
class Lines {
  constructor() {
    this.rest = Buffer.alloc(0);
  }
  push(chunk) {
    const buf = this.rest.length ? Buffer.concat([this.rest, chunk]) : chunk;
    const lines = [];
    let start = 0;
    for (let i = buf.indexOf(10); i !== -1; i = buf.indexOf(10, start)) {
      lines.push(buf.subarray(start, i).toString('utf8').replace(/\r$/, ''));
      start = i + 1;
    }
    this.rest = Buffer.from(buf.subarray(start));
    return lines;
  }
}

/**
 * Runs until the client goes away or the program ends.
 *  - start(): spawns the program (args included); returns the child.
 *  - installed: a promise for {ok, error}, when the installer is done.
 *  - status(failure): {text, failed} for setup_status; `failure` is why
 *    the program is not running, once the stand-in knows.
 *  - graceMs: how long to keep the client waiting before standing in.
 *  - version: this launcher's version, for serverInfo.
 */
function runShim({ start, installed, status, graceMs, version }) {
  let mode = 'waiting'; // waiting → (shim → handover →) piped
  const raw = []; // Everything the client sent while waiting.
  let lines = new Lines();
  let held = []; // Raw bytes that arrived during the handover.
  let initParams = null;
  let clientInitialized = false;
  let child = null;
  let failure = null; // Why the program is not running, once known.

  const send = (msg) => {
    try {
      process.stdout.write(JSON.stringify(msg) + '\n');
    } catch {
      process.exit(0);
    }
  };
  process.stdout.on('error', () => process.exit(0));

  function onClientData(chunk) {
    if (mode === 'piped') {
      child.stdin.write(chunk);
    } else if (mode === 'waiting') {
      raw.push(chunk);
    } else if (mode === 'handover') {
      held.push(chunk);
    } else {
      for (const line of lines.push(chunk)) handleLine(line);
    }
  }

  process.stdin.on('data', onClientData);
  process.stdin.on('end', () => {
    if (mode === 'piped' || mode === 'handover') child.stdin.end();
    else process.exit(0);
  });

  function handleLine(line) {
    if (!line.trim()) return;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      send({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'parse error' } });
      return;
    }
    if (Array.isArray(msg)) {
      const replies = msg.map(answer).filter(Boolean);
      if (replies.length) send(replies);
    } else {
      const reply = answer(msg);
      if (reply) send(reply);
    }
  }

  function answer(msg) {
    if (!msg || typeof msg !== 'object' || typeof msg.method !== 'string') return null;
    const isRequest = msg.id !== undefined && msg.id !== null;
    const ok = (result) => (isRequest ? { jsonrpc: '2.0', id: msg.id, result } : null);
    switch (msg.method) {
      case 'initialize':
        initParams = { ...(msg.params || {}), protocolVersion: negotiate(msg.params?.protocolVersion) };
        return ok({
          protocolVersion: initParams.protocolVersion,
          capabilities: {
            tools: { listChanged: true },
            prompts: { listChanged: true },
            resources: { subscribe: false, listChanged: true },
          },
          serverInfo: { name: 'computer-use', title: 'computer-use (mhrsdev), installing', version },
          instructions: INSTRUCTIONS,
        });
      case 'notifications/initialized':
        clientInitialized = true;
        return null;
      case 'ping':
      case 'logging/setLevel':
        return ok({});
      case 'tools/list':
        return ok({ tools: [STATUS_TOOL] });
      case 'tools/call': {
        const s = status(failure);
        if (msg.params?.name === STATUS_TOOL.name) {
          return ok({ content: [{ type: 'text', text: s.text }], isError: !!s.failed });
        }
        return ok({
          content: [{ type: 'text', text: `${msg.params?.name} is not available yet. ${s.text}` }],
          isError: true,
        });
      }
      case 'prompts/list':
        return ok({ prompts: [] });
      case 'resources/list':
        return ok({ resources: [] });
      case 'resources/templates/list':
        return ok({ resourceTemplates: [] });
      default:
        return isRequest
          ? {
              jsonrpc: '2.0',
              id: msg.id,
              error: { code: -32601, message: `${msg.method} is not available while the program is installed` },
            }
          : null;
    }
  }

  /** From waiting or shim mode to the program's own answers. */
  function startProgram() {
    try {
      child = start();
    } catch (e) {
      return fail(`the program did not start: ${e.message}`);
    }
    child.on('error', (e) => {
      log(`the program could not be started: ${e.message}`);
      if (mode === 'piped') process.exit(1);
      fail(`the program could not be started: ${e.message}`);
    });
    child.on('exit', (code, signal) => {
      if (mode === 'piped') process.exit(code ?? (signal ? 1 : 0));
      else if (mode === 'handover') fail(`the program ended at once (exit ${code ?? signal})`);
    });
    child.stdin.on('error', () => {});

    if (mode === 'waiting') {
      // The client heard nothing from us: it talks to the program directly.
      mode = 'piped';
      for (const chunk of raw) child.stdin.write(chunk);
      raw.length = 0;
      child.stdout.on('data', (c) => process.stdout.write(c));
      return;
    }

    // The client spoke to the stand-in: introduce it to the program.
    mode = 'handover';
    held = lines.rest.length ? [lines.rest] : [];
    lines = new Lines();
    const out = new Lines();
    let introduced = false;
    child.stdout.on('data', (chunk) => {
      if (introduced) {
        process.stdout.write(chunk);
        return;
      }
      // Up to the program's answer to our initialize, line by line: that
      // answer is ours, anything before it is the client's.
      for (const line of out.push(chunk)) {
        if (introduced) {
          process.stdout.write(line + '\n');
          continue;
        }
        let msg = null;
        try {
          msg = JSON.parse(line);
        } catch {
          // Not JSON: passed on as it is.
        }
        if (msg && msg.id === HANDOVER_ID) {
          if (msg.error) {
            child.kill();
            fail(`the program refused the handover: ${msg.error.message}`);
            return;
          }
          introduced = true;
          finishHandover();
        } else if (line.trim()) {
          process.stdout.write(line + '\n');
        }
      }
      if (introduced && out.rest.length) process.stdout.write(out.rest);
    });
    child.stdin.write(
      JSON.stringify({ jsonrpc: '2.0', id: HANDOVER_ID, method: 'initialize', params: initParams || {} }) + '\n',
    );
  }

  function finishHandover() {
    if (clientInitialized) child.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n');
    for (const chunk of held) child.stdin.write(chunk);
    held = [];
    mode = 'piped';
    for (const what of ['tools', 'prompts', 'resources']) {
      send({ jsonrpc: '2.0', method: `notifications/${what}/list_changed` });
    }
    log('the program is ready and has taken over');
  }

  /** Back to standing in, now telling what went wrong. */
  function fail(why) {
    failure = why;
    log(why);
    if (mode === 'piped') process.exit(1);
    const pending = mode === 'waiting' ? raw.splice(0) : held.splice(0);
    mode = 'shim';
    for (const chunk of pending) for (const line of lines.push(chunk)) handleLine(line);
  }

  function standIn() {
    if (mode !== 'waiting') return;
    log('standing in for the program while it is installed');
    mode = 'shim';
    const pending = raw.splice(0);
    for (const chunk of pending) for (const line of lines.push(chunk)) handleLine(line);
  }

  const grace = setTimeout(standIn, graceMs);
  installed.then((result) => {
    clearTimeout(grace);
    if (result.ok) startProgram();
    else {
      failure = result.error;
      standIn();
    }
  });
}

module.exports = { runShim, negotiate, STATUS_TOOL, HANDOVER_ID, Lines };

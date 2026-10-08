'use strict';
// A stand-in for computer-use-mcp in the launcher's tests: a tiny MCP
// server over stdio that says which version it is and writes down every
// message it gets (FAKE_LOG), so the tests can see what reached it.

const fs = require('fs');

const version = process.argv[2];
const logFile = process.env.FAKE_LOG;
let rest = '';

function note(msg) {
  if (logFile) fs.appendFileSync(logFile, JSON.stringify(msg) + '\n');
}

function send(msg) {
  process.stdout.write(JSON.stringify(msg) + '\n');
}

process.stdin.on('data', (chunk) => {
  rest += chunk.toString('utf8');
  let i;
  while ((i = rest.indexOf('\n')) !== -1) {
    const line = rest.slice(0, i);
    rest = rest.slice(i + 1);
    if (!line.trim()) continue;
    const msg = JSON.parse(line);
    note(msg);
    if (msg.id === undefined) continue;
    switch (msg.method) {
      case 'initialize':
        send({
          jsonrpc: '2.0',
          id: msg.id,
          result: {
            protocolVersion: msg.params.protocolVersion,
            capabilities: { tools: { listChanged: true } },
            serverInfo: { name: 'computer-use', version },
            instructions: 'the real program',
          },
        });
        break;
      case 'tools/list':
        send({ jsonrpc: '2.0', id: msg.id, result: { tools: [{ name: 'click', inputSchema: { type: 'object' } }] } });
        break;
      default:
        send({ jsonrpc: '2.0', id: msg.id, result: { echo: msg.method, args: process.argv.slice(3) } });
    }
  }
});
process.stdin.on('end', () => process.exit(0));

'use strict';
// Tests of the plugin's launcher (plugin/launcher/): its parts on their own,
// then the whole of it against a fake GitHub on this computer and a fake
// program (fake-program.cjs) in the release zips.
//
//   node --test scripts/claude-code/launcher-test/
//
// The end-to-end tests need a POSIX shell (the fake program is a script)
// and python3 (to make zips with another implementation than ours).

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const http = require('http');
const crypto = require('crypto');
const zlib = require('zlib');
const { spawn, spawnSync } = require('child_process');

const ROOT = path.resolve(__dirname, '../../..');
const LAUNCHER = path.join(ROOT, 'plugin', 'launcher');
const FAKE_PROGRAM = path.join(__dirname, 'fake-program.cjs');
const REPO = 'mhrsdev/zero-use-computer';
const TARGET = require(path.join(LAUNCHER, 'platform.cjs')).target().name;
const POSIX = process.platform !== 'win32';

const { parse, compare } = require(path.join(LAUNCHER, 'version.cjs'));
const { target, homeDir } = require(path.join(LAUNCHER, 'platform.cjs'));
const { proxyFor } = require(path.join(LAUNCHER, 'net.cjs'));
const { crc32, extractFile } = require(path.join(LAUNCHER, 'zip.cjs'));
const { Lines, negotiate } = require(path.join(LAUNCHER, 'shim.cjs'));

function tmpdir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'zero-launcher-'));
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(what, check, timeoutMs = 20000) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

// ---- zips -----------------------------------------------------------------

/** A zip made by Python's zipfile (another implementation than ours). */
function pythonZip(files, { stored = false } = {}) {
  const out = path.join(tmpdir(), 'made.zip');
  const script = `
import json, sys, zipfile, base64
files = json.load(sys.stdin)
method = zipfile.ZIP_STORED if ${stored ? 'True' : 'False'} else zipfile.ZIP_DEFLATED
with zipfile.ZipFile(sys.argv[1], 'w', compression=method) as z:
    for f in files:
        info = zipfile.ZipInfo(f['name'])
        info.external_attr = (0o755 << 16)
        info.compress_type = method
        z.writestr(info, base64.b64decode(f['data']))
`;
  const r = spawnSync('python3', ['-I', '-c', script, out], {
    input: JSON.stringify(files.map((f) => ({ name: f.name, data: Buffer.from(f.data).toString('base64') }))),
  });
  if (r.status !== 0) throw new Error(`python3 failed: ${r.stderr}`);
  return fs.readFileSync(out);
}

/** A zip written here, with every size and offset in zip64 fields. */
function zip64(files) {
  const locals = [];
  const central = [];
  let offset = 0;
  for (const f of files) {
    const data = Buffer.from(f.data);
    const packed = zlib.deflateRawSync(data);
    const name = Buffer.from(f.name);
    const crc = crc32(data);
    const localExtra = Buffer.alloc(20);
    localExtra.writeUInt16LE(1, 0);
    localExtra.writeUInt16LE(16, 2);
    localExtra.writeBigUInt64LE(BigInt(data.length), 4);
    localExtra.writeBigUInt64LE(BigInt(packed.length), 12);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(45, 4);
    local.writeUInt16LE(8, 8);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(0xffffffff, 18);
    local.writeUInt32LE(0xffffffff, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(localExtra.length, 28);
    locals.push(local, name, localExtra, packed);
    const extra = Buffer.alloc(28);
    extra.writeUInt16LE(1, 0);
    extra.writeUInt16LE(24, 2);
    extra.writeBigUInt64LE(BigInt(data.length), 4);
    extra.writeBigUInt64LE(BigInt(packed.length), 12);
    extra.writeBigUInt64LE(BigInt(offset), 20);
    const c = Buffer.alloc(46);
    c.writeUInt32LE(0x02014b50, 0);
    c.writeUInt16LE(45, 4);
    c.writeUInt16LE(45, 6);
    c.writeUInt16LE(8, 10);
    c.writeUInt32LE(crc, 16);
    c.writeUInt32LE(0xffffffff, 20);
    c.writeUInt32LE(0xffffffff, 24);
    c.writeUInt16LE(name.length, 28);
    c.writeUInt16LE(extra.length, 30);
    c.writeUInt32LE(0xffffffff, 42);
    central.push(c, name, extra);
    offset += local.length + name.length + localExtra.length + packed.length;
  }
  const cd = Buffer.concat(central);
  const z64 = Buffer.alloc(56);
  z64.writeUInt32LE(0x06064b50, 0);
  z64.writeBigUInt64LE(44n, 4);
  z64.writeUInt16LE(45, 12);
  z64.writeUInt16LE(45, 14);
  z64.writeBigUInt64LE(BigInt(files.length), 24);
  z64.writeBigUInt64LE(BigInt(files.length), 32);
  z64.writeBigUInt64LE(BigInt(cd.length), 40);
  z64.writeBigUInt64LE(BigInt(offset), 48);
  const loc = Buffer.alloc(20);
  loc.writeUInt32LE(0x07064b50, 0);
  loc.writeBigUInt64LE(BigInt(offset + cd.length), 8);
  loc.writeUInt32LE(1, 16);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0xffff, 8);
  eocd.writeUInt16LE(0xffff, 10);
  eocd.writeUInt32LE(0xffffffff, 12);
  eocd.writeUInt32LE(0xffffffff, 16);
  return Buffer.concat([...locals, cd, z64, loc, eocd]);
}

function programScript(version) {
  return (
    '#!/bin/sh\n' +
    `if [ "$1" = "--version" ]; then echo "computer-use-mcp ${version}"; exit 0; fi\n` +
    `exec "${process.execPath}" "${FAKE_PROGRAM}" "${version}" "$@"\n`
  );
}

/** A release zip as CI makes it: the program among the package's files. */
function releaseZip(version) {
  return pythonZip([
    { name: 'README.md', data: 'readme' },
    { name: 'skills/computer-use/SKILL.md', data: 'skill' },
    { name: 'computer-use-mcp', data: programScript(version) },
    // Incompressible padding, so a download takes a while when throttled.
    { name: 'padding.bin', data: crypto.randomBytes(400 * 1024) },
  ]);
}

// ---- a fake GitHub ---------------------------------------------------------

class FakeGitHub {
  constructor() {
    this.releases = new Map(); // tag → {zip, sidecar, digest}
    this.latest = null;
    this.apiStatus = 200;
    this.throttleMs = 0;
    this.cutNext = false;
    this.requests = [];
  }

  add(version, { sidecar = true, digest = true, wrongSha = false } = {}) {
    const zip = releaseZip(version);
    let sha = crypto.createHash('sha256').update(zip).digest('hex');
    if (wrongSha) sha = 'f'.repeat(64);
    this.releases.set(`v${version}`, { zip, sha, sidecar, digest });
    this.latest = `v${version}`;
    return this;
  }

  async start() {
    this.server = http.createServer((req, res) => this.handle(req, res));
    await new Promise((r) => this.server.listen(0, '127.0.0.1', r));
    this.origin = `http://127.0.0.1:${this.server.address().port}`;
    return this;
  }

  stop() {
    this.server.closeAllConnections?.();
    return new Promise((r) => this.server.close(r));
  }

  handle(req, res) {
    const url = decodeURIComponent(req.url);
    this.requests.push({ url, range: req.headers.range || null });
    const asset = `computer-use-mcp-${TARGET}.zip`;
    let m;
    if (url === `/${REPO}/releases/latest`) {
      res.writeHead(302, { Location: `${this.origin}/${REPO}/releases/tag/${this.latest}` }).end();
    } else if ((m = new RegExp(`^/${REPO}/releases/download/([^/]+)/(.+)$`).exec(url))) {
      const rel = this.releases.get(m[1]);
      if (rel && m[2] === asset) {
        res.writeHead(302, { Location: `/blob/${m[1]}` }).end();
      } else if (rel && m[2] === `${asset}.sha256` && rel.sidecar) {
        res.writeHead(200).end(`${rel.sha}  ${asset}\n`);
      } else {
        res.writeHead(404).end('Not Found');
      }
    } else if ((m = /^\/blob\/([^/]+)$/.exec(url))) {
      this.serveBlob(req, res, this.releases.get(m[1]).zip);
    } else if ((m = new RegExp(`^/repos/${REPO}/releases/tags/(.+)$`).exec(url))) {
      const rel = this.releases.get(m[1]);
      if (this.apiStatus !== 200) {
        res.writeHead(this.apiStatus, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ message: 'API rate limit exceeded for 127.0.0.1.' }));
      } else if (!rel) {
        res.writeHead(404).end('{"message":"Not Found"}');
      } else {
        const assets = [{ name: asset, digest: rel.digest ? `sha256:${rel.sha}` : null }];
        res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({ tag_name: m[1], assets }));
      }
    } else {
      res.writeHead(404).end();
    }
  }

  async serveBlob(req, res, zip) {
    let start = 0;
    const range = /^bytes=(\d+)-$/.exec(req.headers.range || '');
    if (range) {
      start = Number(range[1]);
      if (start >= zip.length) {
        res.writeHead(416, { 'Content-Range': `bytes */${zip.length}` }).end();
        return;
      }
      res.writeHead(206, {
        'Content-Range': `bytes ${start}-${zip.length - 1}/${zip.length}`,
        'Content-Length': zip.length - start,
      });
    } else {
      res.writeHead(200, { 'Content-Length': zip.length });
    }
    const cut = this.cutNext && !range;
    if (cut) this.cutNext = false;
    for (let at = start; at < zip.length; at += 16384) {
      if (cut && at - start > zip.length / 2) {
        res.destroy();
        return;
      }
      if (!res.write(zip.subarray(at, Math.min(at + 16384, zip.length)))) {
        await new Promise((r) => res.once('drain', r));
      }
      if (this.throttleMs) await sleep(this.throttleMs);
    }
    res.end();
  }

  fullDownloads() {
    return this.requests.filter((r) => r.url.startsWith('/blob/') && !r.range).length;
  }
}

// ---- the launcher as Claude Code runs it ------------------------------------

/** A copy of the plugin's launcher with its own plugin.json version. */
function pluginCopy(version) {
  const dir = tmpdir();
  fs.mkdirSync(path.join(dir, 'launcher'));
  for (const f of fs.readdirSync(LAUNCHER)) fs.copyFileSync(path.join(LAUNCHER, f), path.join(dir, 'launcher', f));
  fs.mkdirSync(path.join(dir, '.claude-plugin'));
  fs.writeFileSync(path.join(dir, '.claude-plugin', 'plugin.json'), JSON.stringify({ name: 'computer-use', version }));
  return dir;
}

class Client {
  constructor({ plugin, home, origin, graceMs = 15000, fakeLog }) {
    const env = { ...process.env, COMPUTER_USE_HOME: home, ZERO_LAUNCHER_GITHUB: origin };
    env.ZERO_LAUNCHER_GRACE_MS = String(graceMs);
    if (fakeLog) env.FAKE_LOG = fakeLog;
    for (const k of ['HTTPS_PROXY', 'https_proxy', 'ALL_PROXY', 'all_proxy', 'COMPUTER_USE_MCP_BIN']) delete env[k];
    this.child = spawn(process.execPath, [path.join(plugin, 'launcher', 'main.cjs'), 'serve', '--instructions', 'short'], {
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    this.stderr = '';
    this.child.stderr.on('data', (c) => (this.stderr += c));
    this.messages = [];
    this.nextId = 1;
    const lines = new Lines();
    this.child.stdout.on('data', (c) => {
      for (const line of lines.push(c)) if (line.trim()) this.messages.push(JSON.parse(line));
    });
    this.exited = new Promise((r) => this.child.on('exit', r));
  }

  send(msg) {
    this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...msg }) + '\n');
  }

  waitFor(what, pred, timeoutMs) {
    return until(what, () => this.messages.find(pred), timeoutMs).catch((e) => {
      throw new Error(`${e.message}; stderr: ${this.stderr}`);
    });
  }

  async request(method, params = {}, timeoutMs) {
    const id = this.nextId++;
    this.send({ id, method, params });
    const reply = await this.waitFor(`the answer to ${method}`, (m) => m.id === id, timeoutMs);
    if (reply.error) throw new Error(`${method}: ${reply.error.message}`);
    return reply.result;
  }

  initialize(timeoutMs) {
    return this.request(
      'initialize',
      { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'launcher-test', version: '1' } },
      timeoutMs,
    );
  }

  async close() {
    this.child.stdin.end();
    await Promise.race([this.exited, sleep(5000)]);
    this.child.kill();
  }
}

function binOf(home) {
  return path.join(home, 'bin', 'computer-use-mcp');
}

function programVersion(home) {
  const r = spawnSync(binOf(home), ['--version'], { encoding: 'utf8' });
  return r.status === 0 ? parse(r.stdout)?.text : null;
}

// ---- the parts -----------------------------------------------------------

test('versions are put in order, pre-releases before their release', () => {
  assert.equal(parse('v5.0.1-preview').text, '5.0.1-preview');
  assert.equal(parse('computer-use-mcp 5.0.2\n').text, '5.0.2');
  assert.equal(parse('nothing'), null);
  assert.ok(compare('5.0.2', '5.0.1') > 0);
  assert.ok(compare('5.0.1', '5.0.1-preview') > 0);
  assert.ok(compare('5.0.1-preview.2', '5.0.1-preview.10') < 0);
  assert.ok(compare('5.10.0', '5.9.9') > 0);
  assert.equal(compare('5.0.1', 'v5.0.1'), 0);
});

test('each system gets its build, and the others a reason', () => {
  assert.equal(target('win32', 'x64').name, 'windows-x64');
  assert.equal(target('win32', 'arm64').name, 'windows-x64');
  assert.equal(target('darwin', 'arm64').name, 'macos-arm64');
  assert.equal(target('darwin', 'x64', false).name, 'macos-x64');
  assert.equal(target('darwin', 'x64', true).name, 'macos-arm64');
  assert.equal(target('linux', 'x64').name, 'linux-x64');
  assert.equal(target('linux', 'arm64').name, null);
  assert.match(target('linux', 'arm64').reason, /no build/);
});

test('the program folder is read as the program reads it', () => {
  const user = path.resolve('/home/u');
  assert.equal(homeDir(undefined, user), path.join(user, '.computer-use'));
  assert.equal(homeDir('', user), path.join(user, '.computer-use'));
  assert.equal(homeDir('~/cu', user), path.join(user, 'cu'));
  assert.equal(homeDir('rel', user), path.join(user, 'rel'));
  assert.equal(homeDir(path.resolve('/opt/cu'), user), path.resolve('/opt/cu'));
});

test('proxies: from their settings, NO_PROXY honoured, only http(s) ones without a password', () => {
  const env = { HTTPS_PROXY: 'http://proxy.local:8080', NO_PROXY: 'example.com, .internal' };
  assert.equal(proxyFor('https://github.com/x', env).host, 'proxy.local:8080');
  assert.equal(proxyFor('https://api.example.com/x', env), null);
  assert.equal(proxyFor('https://a.internal/x', env), null);
  assert.equal(proxyFor('http://127.0.0.1:8/x', env), null);
  assert.equal(proxyFor('https://github.com/x', {}), null);
  assert.equal(proxyFor('https://github.com/x', { https_proxy: 'proxy.local:3128' }).host, 'proxy.local:3128');
  assert.equal(proxyFor('https://github.com/x', { ...env, NO_PROXY: '*' }), null);
  assert.throws(() => proxyFor('https://github.com/x', { ALL_PROXY: 'socks5://p:1080' }), /not supported/);
  // The launcher reads no credential: a proxy that wants one is refused, by name.
  assert.throws(
    () => proxyFor('https://github.com/x', { HTTPS_PROXY: 'http://user:secret@proxy.local:8080' }),
    (e) => /user name and password/.test(e.message) && !e.message.includes('secret'),
  );
});

test('MCP lines are split however the bytes arrive', () => {
  const l = new Lines();
  assert.deepEqual(l.push(Buffer.from('{"a":1}\r\n{"b"')), ['{"a":1}']);
  assert.deepEqual(l.push(Buffer.from(':2}\n\n')), ['{"b":2}', '']);
  const euro = Buffer.from('"€"\n');
  assert.deepEqual(l.push(euro.subarray(0, 2)), []);
  assert.deepEqual(l.push(euro.subarray(2)), ['"€"']);
  assert.equal(negotiate('2025-03-26'), '2025-03-26');
  assert.equal(negotiate('2099-01-01'), '2025-06-18');
});

test('zips: deflated, stored and zip64 ones are read and checked', { skip: !hasPython() }, () => {
  const program = crypto.randomBytes(100000);
  const files = [
    { name: 'docs/computer-use-mcp', data: 'deeper, so not this one' },
    { name: 'computer-use-mcp', data: program },
  ];
  assert.deepEqual(extractFile(pythonZip(files), 'computer-use-mcp'), program);
  assert.deepEqual(extractFile(pythonZip(files, { stored: true }), 'computer-use-mcp'), program);
  assert.deepEqual(extractFile(zip64(files), 'computer-use-mcp'), program);
  assert.equal(extractFile(pythonZip(files), 'other'), null);

  const damaged = pythonZip(files, { stored: true });
  const at = damaged.indexOf(program.subarray(0, 32));
  damaged[at + 10] ^= 0xff;
  assert.throws(() => extractFile(damaged, 'computer-use-mcp'), /CRC-32/);
  assert.throws(() => extractFile(Buffer.from('not a zip at all, not even close to one'), 'x'), /damaged/);
});

test('zips made by Info-ZIP (as CI makes the releases)', { skip: !hasCommand('zip') }, () => {
  const dir = tmpdir();
  const program = crypto.randomBytes(50000);
  fs.writeFileSync(path.join(dir, 'computer-use-mcp'), program);
  fs.mkdirSync(path.join(dir, '.claude-plugin'));
  fs.writeFileSync(path.join(dir, '.claude-plugin', 'plugin.json'), '{}');
  const r = spawnSync('zip', ['-qr', path.join(dir, '..', path.basename(dir) + '.zip'), '.'], { cwd: dir });
  assert.equal(r.status, 0);
  const zip = fs.readFileSync(path.join(dir, '..', path.basename(dir) + '.zip'));
  assert.deepEqual(extractFile(zip, 'computer-use-mcp'), program);
});

function hasCommand(cmd) {
  return spawnSync(cmd, ['-v'], { stdio: 'ignore' }).error === undefined;
}

function hasPython() {
  return hasCommand('python3');
}

// ---- the whole launcher ----------------------------------------------------

const e2e = { skip: !POSIX || !hasPython() || !TARGET ? 'needs a POSIX shell, python3 and a supported system' : false };

test('a fast install: the client talks to the program from the start', e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0').start();
  const home = tmpdir();
  const plugin = pluginCopy('7.1.0');
  const c = new Client({ plugin, home, origin: gh.origin });
  try {
    const init = await c.initialize();
    assert.equal(init.serverInfo.version, '7.1.0');
    assert.equal(init.instructions, 'the real program');
    c.send({ method: 'notifications/initialized' });
    assert.deepEqual((await c.request('tools/list')).tools.map((t) => t.name), ['click']);
    const call = await c.request('tools/call', { name: 'click', arguments: {} });
    assert.deepEqual(call.args, ['serve', '--instructions', 'short']);
    assert.equal(c.messages.filter((m) => m.method?.endsWith('list_changed')).length, 0);
  } finally {
    await c.close();
    await gh.stop();
  }
  assert.equal(programVersion(home), '7.1.0');
  assert.equal(JSON.parse(fs.readFileSync(path.join(home, 'launcher-status.json'))).phase, 'done');
  assert.deepEqual(fs.readdirSync(path.join(home, 'downloads')), []);
  assert.equal(fs.existsSync(path.join(home, 'bin', '.install.lock')), false);

  // Installed: the next start needs no network at all.
  const offline = new Client({ plugin, home, origin: 'http://127.0.0.1:9' });
  try {
    assert.equal((await offline.initialize(5000)).serverInfo.version, '7.1.0');
  } finally {
    await offline.close();
  }
});

test('a slow install: a stand-in answers, then hands over to the program', e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0').start();
  gh.throttleMs = 60;
  const home = tmpdir();
  const fakeLog = path.join(home, 'fake.log');
  const c = new Client({ plugin: pluginCopy('7.1.0'), home, origin: gh.origin, graceMs: 200, fakeLog });
  try {
    const init = await c.initialize();
    assert.match(init.serverInfo.title, /installing/);
    assert.equal(init.capabilities.tools.listChanged, true);
    c.send({ method: 'notifications/initialized' });
    assert.deepEqual((await c.request('tools/list')).tools.map((t) => t.name), ['setup_status']);
    const status = await c.request('tools/call', { name: 'setup_status', arguments: {} });
    assert.equal(status.isError, false);
    assert.match(status.content[0].text, /Looking|Downloading|Checking|Installing/);
    const early = await c.request('tools/call', { name: 'click', arguments: {} });
    assert.equal(early.isError, true);
    assert.equal((await c.request('ping')).constructor, Object);

    await c.waitFor('tools/list_changed', (m) => m.method === 'notifications/tools/list_changed', 30000);
    assert.deepEqual((await c.request('tools/list')).tools.map((t) => t.name), ['click']);
  } finally {
    await c.close();
    await gh.stop();
  }
  const got = fs.readFileSync(fakeLog, 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(got[0].method, 'initialize');
  assert.equal(got[0].params.clientInfo.name, 'launcher-test');
  assert.equal(got[1].method, 'notifications/initialized');
  assert.equal(got[2].method, 'tools/list');
});

test('a download cut off is resumed where it stopped', e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0').start();
  gh.cutNext = true;
  const home = tmpdir();
  const c = new Client({ plugin: pluginCopy('7.1.0'), home, origin: gh.origin });
  try {
    assert.equal((await c.initialize()).serverInfo.version, '7.1.0');
  } finally {
    await c.close();
    await gh.stop();
  }
  assert.ok(gh.requests.some((r) => r.url.startsWith('/blob/') && r.range), 'a Range request was made');
  assert.equal(gh.fullDownloads(), 1);
});

test('a zip that does not match its SHA-256 is not installed', e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0', { wrongSha: true }).start();
  const home = tmpdir();
  const c = new Client({ plugin: pluginCopy('7.1.0'), home, origin: gh.origin, graceMs: 200 });
  try {
    await c.initialize();
    const status = await until('the failure', async () => {
      const s = await c.request('tools/call', { name: 'setup_status', arguments: {} });
      return s.isError && s;
    });
    assert.match(status.content[0].text, /SHA-256/);
    assert.match(status.content[0].text, /install it by hand/);
  } finally {
    await c.close();
    await gh.stop();
  }
  assert.equal(fs.existsSync(binOf(home)), false);
});

test("without a .sha256 file, GitHub API's digest is used", e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0', { sidecar: false }).start();
  const home = tmpdir();
  const c = new Client({ plugin: pluginCopy('7.1.0'), home, origin: gh.origin });
  try {
    assert.equal((await c.initialize()).serverInfo.version, '7.1.0');
  } finally {
    await c.close();
    await gh.stop();
  }
  assert.ok(gh.requests.some((r) => r.url.startsWith('/repos/')));
});

test("without a .sha256 file and with the API refusing, nothing is installed", e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0', { sidecar: false }).start();
  gh.apiStatus = 403;
  const home = tmpdir();
  const c = new Client({ plugin: pluginCopy('7.1.0'), home, origin: gh.origin, graceMs: 200 });
  try {
    await c.initialize();
    const status = await until('the failure', async () => {
      const s = await c.request('tools/call', { name: 'setup_status', arguments: {} });
      return s.isError && s;
    });
    assert.match(status.content[0].text, /60 calls an hour/);
  } finally {
    await c.close();
    await gh.stop();
  }
  assert.equal(fs.existsSync(binOf(home)), false);
});

test('with no release of the plugin version yet, the latest is installed', e2e, async () => {
  const gh = await new FakeGitHub().add('7.0.0').add('7.1.0').start();
  const home = tmpdir();
  const c = new Client({ plugin: pluginCopy('7.2.0'), home, origin: gh.origin });
  try {
    assert.equal((await c.initialize()).serverInfo.version, '7.1.0');
  } finally {
    await c.close();
    await gh.stop();
  }
});

test('an older program runs at once and is updated for the next start', e2e, async () => {
  const gh = await new FakeGitHub().add('7.0.0').add('7.1.0').start();
  const home = tmpdir();
  const first = new Client({ plugin: pluginCopy('7.0.0'), home, origin: gh.origin });
  try {
    assert.equal((await first.initialize()).serverInfo.version, '7.0.0');
  } finally {
    await first.close();
  }
  const second = new Client({ plugin: pluginCopy('7.1.0'), home, origin: gh.origin });
  try {
    assert.equal((await second.initialize()).serverInfo.version, '7.0.0');
    await until('the update', () => programVersion(home) === '7.1.0');
  } finally {
    await second.close();
    await gh.stop();
  }
});

test('two chats starting together install once', e2e, async () => {
  const gh = await new FakeGitHub().add('7.1.0').start();
  gh.throttleMs = 20;
  const home = tmpdir();
  const plugin = pluginCopy('7.1.0');
  const a = new Client({ plugin, home, origin: gh.origin });
  const b = new Client({ plugin, home, origin: gh.origin });
  try {
    const [x, y] = await Promise.all([a.initialize(), b.initialize()]);
    assert.equal(x.serverInfo.version, '7.1.0');
    assert.equal(y.serverInfo.version, '7.1.0');
  } finally {
    await a.close();
    await b.close();
    await gh.stop();
  }
  assert.equal(gh.fullDownloads(), 1);
});

test('COMPUTER_USE_MCP_BIN runs that program and installs nothing', e2e, async () => {
  const dir = tmpdir();
  const bin = path.join(dir, 'my-build');
  fs.writeFileSync(bin, programScript('0.0.1-dev'), { mode: 0o755 });
  const home = tmpdir();
  const env = { ...process.env, COMPUTER_USE_HOME: home, COMPUTER_USE_MCP_BIN: bin };
  const child = spawn(process.execPath, [path.join(LAUNCHER, 'main.cjs'), 'serve'], { env, stdio: 'pipe' });
  const out = new Lines();
  const got = [];
  child.stdout.on('data', (d) => got.push(...out.push(d)));
  child.stdin.write('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}\n');
  const line = await until('the answer', () => got.find((l) => l.includes('"id":1')));
  assert.equal(JSON.parse(line).result.serverInfo.version, '0.0.1-dev');
  child.stdin.end();
  await new Promise((r) => child.on('exit', r));
  assert.equal(fs.existsSync(path.join(home, 'bin')), false);
});

'use strict';
// HTTPS with nothing but Node's own modules: redirects, timeouts, retries,
// resumed downloads, and the proxy in HTTPS_PROXY / ALL_PROXY (an http://
// proxy, tunnelled with CONNECT; NO_PROXY is honoured). Certificates are
// checked as Node always does (NODE_EXTRA_CA_CERTS adds a company's CA).
// Plain http:// is refused, except to this computer (the tests' server).

const fs = require('fs');
const http = require('http');
const https = require('https');
const tls = require('tls');

const USER_AGENT = 'zero-use-computer-launcher';
const IDLE_TIMEOUT_MS = 30000;
const MAX_REDIRECTS = 8;

class HttpError extends Error {
  constructor(status, url, body) {
    super(`HTTP ${status} from ${new URL(url).host}${body ? ': ' + body : ''}`);
    this.status = status;
  }
}

function isLocal(host) {
  return host === '127.0.0.1' || host === 'localhost' || host === '[::1]' || host === '::1';
}

function envValue(env, name) {
  return env[name.toLowerCase()] || env[name.toUpperCase()] || '';
}

/** The proxy to reach `url` through, as a URL, or null. */
function proxyFor(url, env = process.env) {
  const u = new URL(url);
  if (isLocal(u.hostname)) return null;
  const noProxy = envValue(env, 'no_proxy');
  if (noProxy) {
    const host = u.hostname.toLowerCase();
    for (let entry of noProxy.split(/[,\s]+/)) {
      entry = entry.trim().toLowerCase();
      if (!entry) continue;
      if (entry === '*') return null;
      entry = entry.replace(/:\d+$/, '').replace(/^\*?\./, '');
      if (host === entry || host.endsWith('.' + entry)) return null;
    }
  }
  const raw =
    u.protocol === 'https:'
      ? envValue(env, 'https_proxy') || envValue(env, 'all_proxy')
      : envValue(env, 'http_proxy') || envValue(env, 'all_proxy');
  if (!raw) return null;
  let proxy;
  try {
    proxy = new URL(raw.includes('://') ? raw : 'http://' + raw);
  } catch {
    throw new Error(`the proxy setting "${raw}" is not a URL`);
  }
  if (proxy.protocol !== 'http:' && proxy.protocol !== 'https:') {
    throw new Error(
      `the proxy ${proxy.protocol}//${proxy.host} is not supported (only http:// and https:// proxies); ` +
        'install the program by hand instead (see the plugin README)',
    );
  }
  return proxy;
}

/** A TLS socket to host:port through the proxy's CONNECT tunnel. */
function tunnel(proxy, host, port) {
  return new Promise((resolve, reject) => {
    const headers = { Host: `${host}:${port}`, 'User-Agent': USER_AGENT };
    if (proxy.username) {
      const cred = `${decodeURIComponent(proxy.username)}:${decodeURIComponent(proxy.password)}`;
      headers['Proxy-Authorization'] = 'Basic ' + Buffer.from(cred).toString('base64');
    }
    const lib = proxy.protocol === 'https:' ? https : http;
    const req = lib.request({
      host: proxy.hostname,
      port: proxy.port || (proxy.protocol === 'https:' ? 443 : 80),
      method: 'CONNECT',
      path: `${host}:${port}`,
      headers,
      timeout: IDLE_TIMEOUT_MS,
    });
    req.on('connect', (res, socket) => {
      if (res.statusCode !== 200) {
        socket.destroy();
        reject(new Error(`the proxy ${proxy.host} refused the tunnel to ${host} (HTTP ${res.statusCode})`));
        return;
      }
      const secure = tls.connect({ socket, servername: host }, () => resolve(secure));
      secure.on('error', reject);
    });
    req.on('timeout', () => req.destroy(new Error(`the proxy ${proxy.host} did not answer`)));
    req.on('error', (e) => reject(new Error(`the proxy ${proxy.host}: ${e.message}`)));
    req.end();
  });
}

/** One request, no redirects: resolves with the response once headers came. */
async function requestOnce(url, headers) {
  const u = new URL(url);
  if (u.protocol !== 'https:' && !(u.protocol === 'http:' && isLocal(u.hostname))) {
    throw new Error(`refusing ${u.protocol}//${u.host}: only https is used`);
  }
  const lib = u.protocol === 'https:' ? https : http;
  const port = u.port || (u.protocol === 'https:' ? 443 : 80);
  const options = {
    method: 'GET',
    host: u.hostname,
    port,
    path: u.pathname + u.search,
    headers: { 'User-Agent': USER_AGENT, ...headers },
  };
  const proxy = proxyFor(url);
  if (proxy) {
    const socket = await tunnel(proxy, u.hostname, port);
    options.createConnection = () => socket;
    options.agent = false;
  }
  return new Promise((resolve, reject) => {
    const req = lib.request(options, resolve);
    req.setTimeout(IDLE_TIMEOUT_MS, () => req.destroy(new Error(`no answer from ${u.host} for ${IDLE_TIMEOUT_MS / 1000} s`)));
    req.on('error', reject);
    req.end();
  });
}

/**
 * A GET that follows redirects (unless `follow` is false); the response's
 * stream is left unread, its final URL in `res.url`.
 */
async function request(url, headers = {}, { follow = true } = {}) {
  let current = url;
  for (let i = 0; i <= MAX_REDIRECTS; i++) {
    const res = await requestOnce(current, headers);
    if (follow && [301, 302, 303, 307, 308].includes(res.statusCode) && res.headers.location) {
      res.resume();
      current = new URL(res.headers.location, current).toString();
      continue;
    }
    res.url = current;
    return res;
  }
  throw new Error(`too many redirects from ${new URL(url).host}`);
}

function readBody(res, limit = 4 * 1024 * 1024) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    res.on('data', (c) => {
      size += c.length;
      if (size > limit) res.destroy(new Error('answer too large'));
      else chunks.push(c);
    });
    res.on('end', () => resolve(Buffer.concat(chunks)));
    res.on('error', reject);
  });
}

/** GET JSON; a non-2xx answer throws HttpError (its status kept). */
async function getJson(url, headers = {}) {
  const res = await request(url, { Accept: 'application/json', ...headers });
  const body = await readBody(res);
  if (res.statusCode < 200 || res.statusCode > 299) {
    let message = '';
    try {
      message = JSON.parse(body.toString('utf8')).message || '';
    } catch {
      // Not JSON: the status says enough.
    }
    throw new HttpError(res.statusCode, url, message);
  }
  return JSON.parse(body.toString('utf8'));
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Downloads `url` into `file`, going on from what is already there (a
 * download cut off before, in this run or an earlier one). `onProgress`
 * gets (bytesHave, bytesTotal; 0 when not known yet). Network failures are
 * retried with a growing wait; an answer that can't be right (404, ...) is
 * not. Whether the bytes are the right ones is for the caller's SHA-256.
 */
async function download(url, file, { onProgress = () => {}, attempts = 5, retryWaitMs = 1000 } = {}) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      await downloadOnce(url, file, onProgress);
      return;
    } catch (e) {
      lastError = e;
      if (e instanceof HttpError && e.status >= 400 && e.status < 500 && e.status !== 408 && e.status !== 429) {
        throw e;
      }
      if (attempt < attempts) await sleep(retryWaitMs * 2 ** (attempt - 1));
    }
  }
  throw lastError;
}

async function downloadOnce(url, file, onProgress) {
  const have = fs.statSync(file, { throwIfNoEntry: false })?.size || 0;
  const res = await request(url, have > 0 ? { Range: `bytes=${have}-` } : {});
  let start;
  let total;
  if (res.statusCode === 200) {
    start = 0; // The whole file again: it starts over.
    total = Number(res.headers['content-length']) || 0;
  } else if (res.statusCode === 206) {
    const m = /^bytes (\d+)-\d+\/(\d+|\*)$/.exec(res.headers['content-range'] || '');
    if (!m || Number(m[1]) !== have) {
      res.resume();
      fs.rmSync(file, { force: true });
      throw new Error('the server resumed the download at the wrong place; starting over');
    }
    start = have;
    total = m[2] === '*' ? 0 : Number(m[2]);
  } else if (res.statusCode === 416 && have > 0) {
    // Nothing after what we have: it is all here (the SHA-256 will tell).
    res.resume();
    return;
  } else {
    const body = (await readBody(res, 4096).catch(() => Buffer.alloc(0))).toString('utf8').slice(0, 200);
    throw new HttpError(res.statusCode, res.url, body);
  }
  const out = fs.createWriteStream(file, { flags: start > 0 ? 'a' : 'w' });
  await new Promise((resolve, reject) => {
    let got = start;
    res.on('data', (c) => {
      got += c.length;
      onProgress(got, total);
    });
    res.on('error', reject);
    res.on('aborted', () => reject(new Error('the download was cut off')));
    out.on('error', reject);
    out.on('finish', resolve);
    res.pipe(out);
  });
  const now = fs.statSync(file).size;
  if (total && now !== total) throw new Error(`the download stopped at ${now} of ${total} bytes`);
}

module.exports = { HttpError, proxyFor, request, getJson, download, isLocal };

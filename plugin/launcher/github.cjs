'use strict';
// Which release to install, and its SHA-256. The program comes only from
// this repository's GitHub releases, and is only installed when its
// SHA-256 matches the one the release gives:
//   - <zip>.sha256, the file CI publishes next to each zip (from v5.0.2);
//   - else the "digest" GitHub's API gives for the file (the check the
//     program's own updater makes).
// The first needs no API call: GitHub's API allows 60 calls an hour from
// one address, which a shared address (an office, a VPN) may have used up.
// The release is found the same way, from github.com's own links.

const { request, getJson, HttpError, isLocal } = require('./net.cjs');
const { parse } = require('./version.cjs');

const REPO = 'mhrsdev/zero-use-computer';

/**
 * github.com and its API, or (for the launcher's tests only) a server on
 * this computer named by ZERO_LAUNCHER_GITHUB, which serves both.
 */
function bases(env = process.env) {
  const override = env.ZERO_LAUNCHER_GITHUB;
  if (override) {
    const u = new URL(override);
    if (!isLocal(u.hostname)) throw new Error('ZERO_LAUNCHER_GITHUB may only name a server on this computer');
    return { web: u.origin, api: u.origin };
  }
  return { web: 'https://github.com', api: 'https://api.github.com' };
}

function downloadUrl(tag, name) {
  return `${bases().web}/${REPO}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`;
}

/** True when release `tag` has a file `name` (GitHub redirects to it). */
async function exists(tag, name) {
  const res = await request(downloadUrl(tag, name), {}, { follow: false });
  res.resume();
  if (res.statusCode === 404) return false;
  if (res.statusCode >= 200 && res.statusCode < 400) return true;
  throw new HttpError(res.statusCode, res.url);
}

/** The latest release's tag, from where github.com/…/releases/latest leads. */
async function latestTag() {
  const res = await request(`${bases().web}/${REPO}/releases/latest`, {}, { follow: false });
  res.resume();
  const m = /\/releases\/tag\/([^/?#]+)/.exec(res.headers.location || '');
  if (res.statusCode >= 300 && res.statusCode < 400 && m) return decodeURIComponent(m[1]);
  throw new Error(`could not find ${REPO}'s latest release on GitHub (HTTP ${res.statusCode})`);
}

/** The SHA-256 in release `tag`'s <name>.sha256, or null when it has none. */
async function sidecarSha(tag, name) {
  const res = await request(downloadUrl(tag, `${name}.sha256`));
  const chunks = [];
  for await (const c of res) {
    chunks.push(c);
    if (chunks.length > 64) break;
  }
  if (res.statusCode === 404) return null;
  if (res.statusCode !== 200) throw new HttpError(res.statusCode, res.url);
  const m = /\b([0-9a-f]{64})\b/i.exec(Buffer.concat(chunks).toString('utf8'));
  if (!m) throw new Error(`${name}.sha256 of ${tag} has no SHA-256 in it`);
  return m[1].toLowerCase();
}

/** The SHA-256 GitHub's API gives for release `tag`'s file `name`. */
async function apiSha(tag, name) {
  let release;
  try {
    release = await getJson(`${bases().api}/repos/${REPO}/releases/tags/${encodeURIComponent(tag)}`, {
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
    });
  } catch (e) {
    if (e instanceof HttpError && (e.status === 403 || e.status === 429)) {
      throw new Error(
        `release ${tag} has no ${name}.sha256 and GitHub's API refused for now (${e.message}); ` +
          'it allows 60 calls an hour from one address, so try again later',
      );
    }
    throw e;
  }
  const a = (release.assets || []).find((x) => x.name === name);
  const m = /^sha256:([0-9a-f]{64})$/i.exec(String(a?.digest || ''));
  if (!m) throw new Error(`GitHub gives no SHA-256 for ${name} of ${tag}, so it is not installed`);
  return m[1].toLowerCase();
}

/**
 * The zip to install for `targetName`: release v<wanted> when it has one,
 * else the latest release (null `wanted` means the latest).
 * Resolves with {name, url, sha256, version, tag, exact}.
 */
async function findAsset(wanted, targetName) {
  const name = `computer-use-mcp-${targetName}.zip`;
  let tag = wanted ? `v${wanted}` : null;
  if (tag && !(await exists(tag, name))) tag = null;
  const exact = !!tag || !wanted;
  if (!tag) {
    tag = await latestTag();
    if (!(await exists(tag, name))) throw new Error(`release ${tag} has no ${name}`);
  }
  const version = parse(tag);
  if (!version) throw new Error(`the release "${tag}" has no version in its tag`);
  const sha256 = (await sidecarSha(tag, name)) || (await apiSha(tag, name));
  return { name, url: downloadUrl(tag, name), sha256, version: version.text, tag, exact };
}

module.exports = { REPO, findAsset };

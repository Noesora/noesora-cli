import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64'];
const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const packageManifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
const assetNames = targets.map((target) => `noesora-${target}${target === 'win32-x64' ? '.exe' : ''}`);

function parseChecksums(text) {
  const entries = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (line === '') continue;
    const match = /^([a-f0-9]{64}) {2}([A-Za-z0-9._-]+)$/.exec(line);
    if (!match) throw new Error(`Release checksum manifest contains an invalid line: ${line}`);
    if (!assetNames.includes(match[2])) throw new Error(`Release checksum manifest contains an unexpected asset: ${match[2]}`);
    if (entries.has(match[2])) throw new Error(`Release checksum manifest has duplicate entries for ${match[2]}.`);
    entries.set(match[2], match[1]);
  }
  for (const assetName of assetNames) {
    if (!entries.has(assetName)) throw new Error(`Release checksum manifest has no entry for ${assetName}.`);
  }
  if (entries.size !== assetNames.length) throw new Error('Release checksum manifest must contain exactly five assets.');
  return entries;
}

function resolveBaseUrl(baseUrl) {
  const defaultUrl = `https://github.com/Noesora/noesora-cli/releases/download/v${packageManifest.version}`;
  if (baseUrl === undefined) return defaultUrl;

  const url = new URL(baseUrl);
  const loopback = url.hostname === '127.0.0.1' || url.hostname === 'localhost' || url.hostname === '[::1]';
  if (!loopback || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
    throw new Error('baseUrl overrides are reserved for loopback test fixtures; normal release checks use GitHub HTTPS.');
  }
  return `${url.origin}${url.pathname}`.replace(/\/+$/, '');
}

export async function checkReleaseAssets({ baseUrl, fetchImpl = fetch } = {}) {
  // Baseline trust is GitHub HTTPS plus a same-origin SHA256SUMS manifest; no separate signature is claimed.
  const base = resolveBaseUrl(baseUrl);
  const checksumUrl = `${base}/SHA256SUMS`;
  const checksumResponse = await fetchImpl(checksumUrl);
  if (!checksumResponse.ok) {
    throw new Error(`Release checksum manifest is unavailable (HTTP ${checksumResponse.status}): ${checksumUrl}`);
  }
  const checksums = parseChecksums(await checksumResponse.text());

  for (const assetName of assetNames) {
    const assetUrl = `${base}/${assetName}`;
    const response = await fetchImpl(assetUrl);
    if (!response.ok) throw new Error(`Release asset is unavailable (HTTP ${response.status}): ${assetUrl}`);

    const bytes = Buffer.from(await response.arrayBuffer());
    if (bytes.length === 0) throw new Error(`Release asset is empty: ${assetName}.`);
    const actualHash = createHash('sha256').update(bytes).digest('hex');
    const expectedHash = checksums.get(assetName);
    if (actualHash !== expectedHash) {
      throw new Error(`Release asset SHA-256 mismatch for ${assetName}: expected ${expectedHash}, got ${actualHash}.`);
    }
  }
}

const entry = process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href;
if (import.meta.url === entry) {
  checkReleaseAssets().then(() => {
    console.log(`Verified all five release assets and SHA-256 digests for noesora ${packageManifest.version}.`);
  }).catch((error) => {
    console.error(`noesora: prepublish check failed: ${error.message}`);
    process.exitCode = 1;
  });
}

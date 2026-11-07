import { createHash, randomUUID } from 'node:crypto';
import { chmod, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const targets = [
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-x64',
];

export function targetFor(platform, arch) {
  const target = `${platform}-${arch}`;
  if (!targets.includes(target)) {
    throw new Error(`Unsupported target ${platform}/${arch}. Supported targets: ${targets.join(', ')}.`);
  }
  return target;
}

function releaseBaseUrl(version, override) {
  if (override === undefined) {
    return `https://github.com/Noesora/noesora-cli/releases/download/v${version}`;
  }

  const url = new URL(override);
  const loopback = url.hostname === '127.0.0.1' || url.hostname === 'localhost' || url.hostname === '[::1]';
  if (
    !loopback ||
    !['http:', 'https:'].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error('NOESORA_RELEASE_BASE_URL is reserved for loopback test fixtures; normal installs use GitHub HTTPS.');
  }
  return `${url.origin}${url.pathname}`.replace(/\/+$/, '');
}

function expectedChecksum(manifest, assetName) {
  const matches = [];
  for (const line of manifest.split(/\r?\n/)) {
    const match = /^([a-f0-9]{64}) {2}(.+)$/.exec(line);
    if (match?.[2] === assetName) matches.push(match[1]);
  }
  if (matches.length === 0) throw new Error(`Checksum manifest has no entry for ${assetName}.`);
  if (matches.length > 1) throw new Error(`Checksum manifest has duplicate entries for ${assetName}.`);
  return matches[0];
}

export async function installBinary({
  packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..'),
  platform = process.platform,
  arch = process.arch,
  baseUrl,
  fetchImpl = fetch,
  renameImpl = rename,
} = {}) {
  const manifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
  const target = targetFor(platform, arch);
  const assetName = `noesora-${target}${platform === 'win32' ? '.exe' : ''}`;
  const base = releaseBaseUrl(manifest.version, baseUrl ?? process.env.NOESORA_RELEASE_BASE_URL);
  const checksumUrl = `${base}/SHA256SUMS`;
  const checksumResponse = await fetchImpl(checksumUrl);
  if (!checksumResponse.ok) {
    throw new Error(`Checksum manifest download failed with HTTP ${checksumResponse.status}: ${checksumUrl}`);
  }

  const expectedHash = expectedChecksum(await checksumResponse.text(), assetName);
  const assetUrl = `${base}/${assetName}`;
  const assetResponse = await fetchImpl(assetUrl);
  if (!assetResponse.ok) {
    throw new Error(`Binary download failed with HTTP ${assetResponse.status}: ${assetUrl}`);
  }
  const bytes = Buffer.from(await assetResponse.arrayBuffer());
  if (bytes.length === 0) throw new Error(`Downloaded binary is empty: ${assetName}.`);

  const actualHash = createHash('sha256').update(bytes).digest('hex');
  if (actualHash !== expectedHash) {
    throw new Error(`SHA-256 mismatch for ${assetName}: expected ${expectedHash}, got ${actualHash}.`);
  }

  const binaryName = platform === 'win32' ? 'noesora.exe' : 'noesora';
  const directory = path.join(packageRoot, 'vendor', target);
  const binaryPath = path.join(directory, binaryName);
  const temporaryPath = path.join(directory, `.noesora-${randomUUID()}.tmp`);
  await mkdir(directory, { recursive: true });
  try {
    await writeFile(temporaryPath, bytes, { flag: 'wx', mode: 0o755 });
    if (platform !== 'win32') await chmod(temporaryPath, 0o755);
    // Same-directory rename replaces atomically where supported. Never unlink the old binary first.
    await renameImpl(temporaryPath, binaryPath);
  } catch (error) {
    try {
      await rm(temporaryPath, { force: true });
    } catch (cleanupError) {
      throw new AggregateError([error, cleanupError], 'Binary installation failed and its staged file could not be removed.');
    }
    throw error;
  }
}

const entry = process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href;
if (import.meta.url === entry) {
  installBinary().catch((error) => {
    console.error(`noesora: install failed: ${error.message}`);
    process.exitCode = 1;
  });
}

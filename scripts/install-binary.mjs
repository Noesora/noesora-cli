import { createHash } from 'node:crypto';
import { chmod, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const targets = new Set([
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-x64',
]);

export function targetFor(platform, arch) {
  const target = `${platform}-${arch}`;
  if (!targets.has(target)) {
    throw new Error(`Unsupported target ${platform}/${arch}. Supported targets: ${[...targets].join(', ')}.`);
  }
  return target;
}

async function installBinary() {
  const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const manifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
  const target = targetFor(process.platform, process.arch);
  const assetName = `noesora-${target}${process.platform === 'win32' ? '.exe' : ''}`;
  const base = (process.env.NOESORA_RELEASE_BASE_URL || `https://github.com/Noesora/noesora-cli/releases/download/v${manifest.version}`).replace(/\/+$/, '');
  const checksumUrl = `${base}/SHA256SUMS`;
  const checksumResponse = await fetch(checksumUrl);
  if (!checksumResponse.ok) {
    throw new Error(`Checksum manifest download failed with HTTP ${checksumResponse.status}: ${checksumUrl}`);
  }

  const checksums = await checksumResponse.text();
  let expectedHash;
  for (const line of checksums.split(/\r?\n/)) {
    const match = /^([a-f0-9]{64}) {2}(.+)$/.exec(line);
    if (match?.[2] === assetName) {
      expectedHash = match[1];
      break;
    }
  }
  if (!expectedHash) throw new Error(`Checksum manifest has no entry for ${assetName}.`);

  const assetUrl = `${base}/${assetName}`;
  const assetResponse = await fetch(assetUrl);
  if (!assetResponse.ok) {
    throw new Error(`Binary download failed with HTTP ${assetResponse.status}: ${assetUrl}`);
  }
  const bytes = Buffer.from(await assetResponse.arrayBuffer());
  if (bytes.length === 0) throw new Error(`Downloaded binary is empty: ${assetName}.`);

  const actualHash = createHash('sha256').update(bytes).digest('hex');
  if (actualHash !== expectedHash) {
    throw new Error(`SHA-256 mismatch for ${assetName}: expected ${expectedHash}, got ${actualHash}.`);
  }

  const binaryName = process.platform === 'win32' ? 'noesora.exe' : 'noesora';
  const directory = path.join(packageRoot, 'vendor', target);
  const binaryPath = path.join(directory, binaryName);
  const temporaryPath = `${binaryPath}.tmp-${process.pid}`;
  await mkdir(directory, { recursive: true });
  try {
    await writeFile(temporaryPath, bytes, { mode: 0o755 });
    if (process.platform !== 'win32') await chmod(temporaryPath, 0o755);
    await rm(binaryPath, { force: true });
    await rename(temporaryPath, binaryPath);
  } catch (error) {
    await rm(temporaryPath, { force: true });
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

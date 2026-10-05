import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64'];
const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const packageManifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));

export async function checkReleaseAssets({ baseUrl } = {}) {
  const base = (baseUrl || `https://github.com/Noesora/noesora-cli/releases/download/v${packageManifest.version}`).replace(/\/+$/, '');
  const checksumUrl = `${base}/SHA256SUMS`;
  const checksumResponse = await fetch(checksumUrl);
  if (!checksumResponse.ok) {
    throw new Error(`Release checksum manifest is unavailable (HTTP ${checksumResponse.status}): ${checksumUrl}`);
  }

  const checksumText = await checksumResponse.text();
  for (const target of targets) {
    const assetName = `noesora-${target}${target === 'win32-x64' ? '.exe' : ''}`;
    const hasChecksum = checksumText.split(/\r?\n/).some((line) => {
      const match = /^([a-f0-9]{64}) {2}(.+)$/.exec(line);
      return match?.[2] === assetName;
    });
    if (!hasChecksum) throw new Error(`Release checksum manifest has no entry for ${assetName}.`);

    const assetUrl = `${base}/${assetName}`;
    const response = await fetch(assetUrl, { method: 'HEAD' });
    if (!response.ok) throw new Error(`Release asset is unavailable (HTTP ${response.status}): ${assetUrl}`);
  }
}

const entry = process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href;
if (import.meta.url === entry) {
  checkReleaseAssets().then(() => {
    console.log(`Verified all five release assets for noesora ${packageManifest.version}.`);
  }).catch((error) => {
    console.error(`noesora: prepublish check failed: ${error.message}`);
    process.exitCode = 1;
  });
}

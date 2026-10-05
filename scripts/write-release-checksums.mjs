import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64'];
const output = path.resolve(process.argv[2] || 'dist/release');
const lines = [];

for (const target of targets) {
  const assetName = `noesora-${target}${target === 'win32-x64' ? '.exe' : ''}`;
  const bytes = await readFile(path.join(output, assetName));
  if (bytes.length === 0) throw new Error(`Release asset is empty: ${assetName}`);
  const digest = createHash('sha256').update(bytes).digest('hex');
  lines.push(`${digest}  ${assetName}`);
}

await writeFile(path.join(output, 'SHA256SUMS'), `${lines.join('\n')}\n`);
console.log(`Wrote SHA256SUMS for ${targets.length} assets in ${output}.`);

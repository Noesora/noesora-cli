import assert from 'node:assert/strict';
import { chmod, copyFile, mkdir, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const targets = new Set(['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64']);

function option(name) {
  const index = process.argv.indexOf(name);
  assert(index !== -1 && process.argv[index + 1], `Missing ${name}`);
  return process.argv[index + 1];
}

function cargoPackageVersion(source) {
  let inPackageSection = false;
  for (const line of source.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith('[')) {
      if (inPackageSection) break;
      inPackageSection = trimmed === '[package]';
      continue;
    }
    if (!inPackageSection) continue;
    const match = /^version\s*=\s*"([^"]+)"\s*$/.exec(trimmed);
    if (match) return match[1];
  }
  throw new Error('Cargo.toml has no package.version value.');
}

const target = option('--target');
assert(targets.has(target), `Unsupported target: ${target}`);
const binary = path.resolve(option('--binary'));
const output = path.resolve(option('--out'));
const packageRoot = process.cwd();
const manifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
const cargoManifest = await readFile(path.join(packageRoot, 'Cargo.toml'), 'utf8');
assert.equal(manifest.version, cargoPackageVersion(cargoManifest), 'package.json version must match Cargo.toml package version');

const versionResult = spawnSync(binary, ['--version'], { encoding: 'utf8' });
assert.equal(versionResult.status, 0, versionResult.stderr || versionResult.error?.message || 'Binary version check failed');
assert.equal(versionResult.stdout.trim(), `noesora ${manifest.version}`, 'Binary version must match package.json');

const assetName = `noesora-${target}${target === 'win32-x64' ? '.exe' : ''}`;
await mkdir(output, { recursive: true });
const assetPath = path.join(output, assetName);
await copyFile(binary, assetPath);
if (target !== 'win32-x64') await chmod(assetPath, 0o755);
console.log(assetPath);

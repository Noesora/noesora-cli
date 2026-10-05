import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const targets = {
  'darwin-arm64': { os: 'darwin', cpu: 'arm64', executable: 'noesora' },
  'darwin-x64': { os: 'darwin', cpu: 'x64', executable: 'noesora' },
  'linux-arm64': { os: 'linux', cpu: 'arm64', executable: 'noesora' },
  'linux-x64': { os: 'linux', cpu: 'x64', executable: 'noesora' },
  'win32-x64': { os: 'win32', cpu: 'x64', executable: 'noesora.exe' },
};

function option(name) {
  const index = process.argv.indexOf(name);
  assert(index !== -1 && process.argv[index + 1], 'Missing ' + name);
  return process.argv[index + 1];
}

const target = option('--target');
const profile = targets[target];
assert(profile, 'Unsupported target: ' + target);
const root = process.cwd();
const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
const packageName = 'noesora-bin-' + target;
assert.equal(manifest.optionalDependencies?.[packageName], manifest.version, packageName + ' version must match the meta-package');

const binary = resolve(option('--binary'));
const version = spawnSync(binary, ['--version'], { encoding: 'utf8' });
assert.equal(version.status, 0, version.stderr || version.error?.message || 'CLI version check failed');
assert.equal(version.stdout.trim(), 'noesora ' + manifest.version, 'Native binary version must match package.json');

const out = resolve(option('--out'));
const stage = join(out, 'staging', packageName);
rmSync(stage, { recursive: true, force: true });
mkdirSync(join(stage, 'bin'), { recursive: true });
const executablePath = join(stage, 'bin', profile.executable);
copyFileSync(binary, executablePath);
if (process.platform !== 'win32') chmodSync(executablePath, 0o755);
copyFileSync(join(root, 'LICENSE'), join(stage, 'LICENSE'));
writeFileSync(join(stage, 'README.md'), '# Noesora native binary\n\nPrebuilt executable for the noesora CLI on ' + profile.os + '/' + profile.cpu + '.\n');
writeFileSync(join(stage, 'package.json'), JSON.stringify({
  name: packageName,
  version: manifest.version,
  description: 'Noesora CLI binary for ' + profile.os + '/' + profile.cpu + '.',
  license: manifest.license,
  os: [profile.os],
  cpu: [profile.cpu],
  files: ['bin/' + profile.executable],
}, null, 2) + String.fromCharCode(10));
console.log(stage);

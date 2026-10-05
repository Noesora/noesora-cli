import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { join, resolve } from 'node:path';

const [prefixArg, version] = process.argv.slice(2);
assert(prefixArg && version, 'Usage: node scripts/assert-npm-install.mjs <install-prefix> <version>');
const launcher = join(resolve(prefixArg), 'node_modules', 'noesora', 'bin', 'noesora.js');

const versionResult = spawnSync(process.execPath, [launcher, '--version'], { encoding: 'utf8' });
assert.equal(versionResult.status, 0, versionResult.stderr || versionResult.error?.message);
assert.equal(versionResult.stdout.trim(), 'noesora ' + version);

const invalidResult = spawnSync(process.execPath, [launcher, '--noesora-test-invalid-option'], { encoding: 'utf8' });
assert.equal(invalidResult.status, 2, invalidResult.stderr || 'Invalid CLI arguments must keep the native exit code');
assert.match(invalidResult.stderr, /error/i);

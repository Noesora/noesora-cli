#!/usr/bin/env node
'use strict';

const { existsSync } = require('node:fs');
const { spawnSync } = require('node:child_process');
const { join } = require('node:path');

const target = process.platform + '-' + process.arch;
const binaryName = process.platform === 'win32' ? 'noesora.exe' : 'noesora';
const binary = join(__dirname, '..', 'vendor', target, binaryName);
const supported = 'darwin-arm64, darwin-x64, linux-arm64, linux-x64, win32-x64';

if (!existsSync(binary)) {
  console.error('Noesora binary is missing for ' + process.platform + '/' + process.arch + '. Reinstall with npm scripts enabled. Supported targets: ' + supported + '.');
  process.exitCode = 1;
} else {
  const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) {
    console.error('Failed to start Noesora: ' + result.error.message);
    process.exitCode = 1;
  } else {
    process.exitCode = result.status === null ? 1 : result.status;
  }
}

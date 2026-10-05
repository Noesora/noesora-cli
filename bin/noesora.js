#!/usr/bin/env node
'use strict';

const { spawnSync } = require('node:child_process');
const { createRequire } = require('node:module');

const packageRequire = createRequire(__filename);
const supported = 'darwin-arm64, darwin-x64, linux-arm64, linux-x64, win32-x64';
const packageName = 'noesora-bin-' + process.platform + '-' + process.arch;
const binaryName = process.platform === 'win32' ? 'noesora.exe' : 'noesora';

function main() {
  let binary;
  try {
    binary = packageRequire.resolve(packageName + '/bin/' + binaryName);
  } catch {
    console.error('Noesora has no native package for ' + process.platform + '/' + process.arch + '. Supported targets: ' + supported + '.');
    process.exitCode = 1;
    return;
  }

  const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) {
    console.error('Failed to start Noesora: ' + result.error.message);
    process.exitCode = 1;
  } else {
    process.exitCode = result.status === null ? 1 : result.status;
  }
}

main();

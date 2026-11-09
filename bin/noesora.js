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
  reportMissingBinary();
} else {
  run();
}

function reportMissingBinary() {
  console.error('Noesora binary is missing for ' + process.platform + '/' + process.arch + '. Reinstall with npm lifecycle scripts enabled. Supported targets: ' + supported + '.');
  process.exitCode = 1;
}

function launchBinary() {
  const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) {
    console.error('Failed to start Noesora: ' + result.error.message);
    process.exitCode = 1;
  } else {
    process.exitCode = result.status === null ? 1 : result.status;
  }
}

async function run() {
  let outcome = 'skipped';
  try {
    const { maybeUpdate } = require(join(__dirname, '..', 'scripts', 'update-check.cjs'));
    outcome = await maybeUpdate({
      args: process.argv.slice(2),
      packageRoot: join(__dirname, '..'),
    });
  } catch {
    console.error('noesora: update check failed; continuing with the installed CLI.');
  }

  if (outcome === 'cancelled') {
    process.exitCode = 130;
    return;
  }

  if (outcome === 'verification-failed') {
    process.exitCode = 1;
    return;
  }

  if (!existsSync(binary)) {
    reportMissingBinary();
    return;
  }

  launchBinary();
}

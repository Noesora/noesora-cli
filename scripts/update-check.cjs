'use strict';

const { createHash } = require('node:crypto');

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { createInterface } = require('node:readline');
let semver;

const latestUrl = 'https://registry.npmjs.org/noesora/latest';
const registryUrl = 'https://registry.npmjs.org/';
const lookupDeadlineMs = 1000;
const managerQueryTimeoutMs = 5000;
const maxVersionLength = 256;
const maxMetadataBytes = 65_536;
const maxExecutableBytes = 256 * 1024 * 1024;
const supportedTargets = new Set([
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-x64',
]);

function isEligible(args, stdin, stdout, stderr) {
  if (
    !Array.isArray(args)
    || stdin?.isTTY !== true
    || stdout?.isTTY !== true
    || stderr?.isTTY !== true
    || args.some((arg) => ['--json', '-h', '--help', '-V', '--version'].includes(arg))
  ) {
    return false;
  }

  return args.length === 0 || args[0] === 'tui';
}

function samePath(left, right) {
  const a = path.resolve(left);
  const b = path.resolve(right);
  return process.platform === 'win32'
    ? a.toLowerCase() === b.toLowerCase()
    : a === b;
}

function isWithin(parent, child) {
  const relative = path.relative(parent, child);
  return relative !== ''
    && relative !== '..'
    && !relative.startsWith(`..${path.sep}`)
    && !path.isAbsolute(relative);
}

function runtimeInstallation() {
  let nodePath;
  try {
    nodePath = fs.realpathSync(process.execPath);
    if (!fs.statSync(nodePath).isFile()) return undefined;
  } catch {
    return undefined;
  }

  const nodeDirectory = path.dirname(nodePath);
  const runtimeRoot = process.platform === 'win32' ? nodeDirectory : path.dirname(nodeDirectory);
  const expectedNode = process.platform === 'win32'
    ? path.join(runtimeRoot, 'node.exe')
    : path.join(runtimeRoot, 'bin', 'node');
  if (!samePath(nodePath, expectedNode)) return undefined;
  return { runtimeRoot };
}

function npmCliAtRuntimeRoot(runtimeRoot) {
  const candidates = process.platform === 'win32'
    ? [path.join(runtimeRoot, 'node_modules', 'npm', 'bin', 'npm-cli.js')]
    : [
      path.join(runtimeRoot, 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
      path.join(runtimeRoot, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
    ];
  const matches = new Set();

  for (const candidate of candidates) {
    try {
      const npmCli = fs.realpathSync(candidate);
      const npmRoot = fs.realpathSync(path.dirname(path.dirname(npmCli)));
      const manifestPath = fs.realpathSync(path.join(npmRoot, 'package.json'));
      if (
        path.basename(npmCli).toLowerCase() !== 'npm-cli.js'
        || !isWithin(runtimeRoot, npmCli)
        || !isWithin(runtimeRoot, npmRoot)
        || !isWithin(runtimeRoot, manifestPath)
        || !fs.statSync(npmCli).isFile()
        || !fs.statSync(manifestPath).isFile()
      ) {
        continue;
      }

      const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
      if (manifest.name === 'npm') matches.add(npmCli);
    } catch {}
  }

  return matches.size === 1 ? matches.values().next().value : undefined;
}

function resolveTrustedNpm() {
  const runtime = runtimeInstallation();
  if (!runtime) return undefined;
  const npmCli = npmCliAtRuntimeRoot(runtime.runtimeRoot);
  return npmCli ? { npmCli, runtimeRoot: runtime.runtimeRoot } : undefined;
}

function npmPathSetting(npmCli, runtimeRoot, setting) {
  let result;
  try {
    result = spawnSync(process.execPath, [
      npmCli,
      ...setting,
      '--offline',
      '--ignore-scripts=true',
      '--update-notifier=false',
    ], {
      cwd: runtimeRoot,
      env: { ...process.env, npm_execpath: npmCli },
      encoding: 'utf8',
      maxBuffer: 32 * 1024,
      stdio: ['ignore', 'pipe', 'ignore'],
      timeout: managerQueryTimeoutMs,
      windowsHide: true,
    });
  } catch {
    return undefined;
  }

  if (result.error || result.status !== 0 || typeof result.stdout !== 'string') return undefined;

  const output = result.stdout.trim();
  if (!output || /[\r\n]/.test(output)) return undefined;
  return output;
}

function validVersion(version) {
  if (
    typeof version !== 'string'
    || version.length === 0
    || version.length > maxVersionLength
  ) {
    return false;
  }

  try {
    semver ??= require('semver');
    return semver.valid(version) === version;
  } catch {
    return false;
  }
}

function packageManifest(packageRoot) {
  try {
    const manifestPath = path.join(packageRoot, 'package.json');
    if (!fs.lstatSync(manifestPath).isFile()) return undefined;
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    return manifest && typeof manifest === 'object' && !Array.isArray(manifest)
      ? manifest
      : undefined;
  } catch {
    return undefined;
  }
}

function isPlainDirectory(directory) {
  try {
    const entry = fs.lstatSync(directory);
    return entry.isDirectory() && !entry.isSymbolicLink();
  } catch {
    return false;
  }
}

function binaryPathFor(packageRoot) {
  const target = `${process.platform}-${process.arch}`;
  if (!supportedTargets.has(target)) return undefined;

  const binaryName = process.platform === 'win32' ? 'noesora.exe' : 'noesora';
  return path.join(packageRoot, 'vendor', target, binaryName);
}

function executableInfo(packageRoot) {
  const binaryPath = binaryPathFor(packageRoot);
  if (!binaryPath) return undefined;

  try {
    const stat = fs.lstatSync(binaryPath);
    if (
      !stat.isFile()
      || stat.size === 0
      || stat.size > maxExecutableBytes
      || (process.platform !== 'win32' && (stat.mode & 0o111) === 0)
    ) {
      return undefined;
    }
    return { path: binaryPath, stat };
  } catch {
    return undefined;
  }
}

function hasUsableBinary(packageRoot) {
  return executableInfo(packageRoot) !== undefined;
}

async function hashExecutable(executable) {
  if (!executable) return undefined;

  const { path: executablePath, stat: before } = executable;
  let file;
  let stream;
  const hash = createHash('sha256');
  let byteLength = 0;
  const matchesSnapshot = (stat) => stat.isFile()
    && stat.size === before.size
    && stat.dev === before.dev
    && stat.ino === before.ino
    && stat.mtimeMs === before.mtimeMs
    && stat.ctimeMs === before.ctimeMs;

  try {
    file = await fs.promises.open(executablePath, 'r');
    if (!matchesSnapshot(await file.stat())) return undefined;
    stream = file.createReadStream({ highWaterMark: 64 * 1024, autoClose: false });
    for await (const chunk of stream) {
      byteLength += chunk.byteLength;
      if (byteLength > maxExecutableBytes) {
        stream.destroy();
        return undefined;
      }
      hash.update(chunk);
    }

    const after = await file.stat();
    const pathAfter = fs.lstatSync(executablePath);
    if (
      before.size !== byteLength
      || !matchesSnapshot(after)
      || !matchesSnapshot(pathAfter)
    ) {
      return undefined;
    }
    return hash.digest('hex');
  } catch {
    return undefined;
  } finally {
    stream?.destroy();
    await file?.close().catch(() => {});
  }
}

async function isCurrentInstallUsable(owner, expectedHash) {
  const manifest = packageManifest(owner.packageRoot);
  const executable = executableInfo(owner.packageRoot);
  if (
    !isPlainDirectory(owner.packageRoot)
    || manifest?.name !== 'noesora'
    || manifest.version !== owner.version
    || !executable
  ) {
    return false;
  }

  const actualHash = await hashExecutable(executable);
  return typeof expectedHash === 'string' && actualHash === expectedHash;
}

function resolveGlobalOwner(packageRoot) {
  if (typeof packageRoot !== 'string' || !packageRoot) return undefined;

  const manager = resolveTrustedNpm();
  if (!manager) return undefined;

  const { npmCli, runtimeRoot } = manager;
  const globalRootOutput = npmPathSetting(npmCli, runtimeRoot, ['root', '--global']);
  const prefixOutput = npmPathSetting(npmCli, runtimeRoot, ['prefix', '--global']);
  if (!globalRootOutput || !prefixOutput) return undefined;

  try {
    const resolvedPackageRoot = fs.realpathSync(path.resolve(packageRoot));
    const packageRootEntry = fs.lstatSync(path.resolve(packageRoot));
    const globalRoot = fs.realpathSync(path.resolve(globalRootOutput));
    const prefix = fs.realpathSync(path.resolve(prefixOutput));
    const globalRootEntry = fs.statSync(globalRoot);
    const prefixEntry = fs.statSync(prefix);

    if (
      packageRootEntry.isSymbolicLink()
      || !packageRootEntry.isDirectory()
      || !globalRootEntry.isDirectory()
      || !prefixEntry.isDirectory()
      || !isWithin(prefix, globalRoot)
      || !samePath(resolvedPackageRoot, path.join(globalRoot, 'noesora'))
      || !hasUsableBinary(resolvedPackageRoot)
    ) {
      return undefined;
    }

    const manifest = packageManifest(resolvedPackageRoot);
    if (manifest?.name !== 'noesora' || !validVersion(manifest.version)) return undefined;

    return {
      npmCli,
      packageRoot: resolvedPackageRoot,
      prefix,
      runtimeRoot,
      version: manifest.version,
    };
  } catch {
    return undefined;
  }
}

function cancelMetadataBody(response, reader) {
  try {
    const cancellation = reader
      ? reader.cancel()
      : response?.body?.cancel?.();
    if (cancellation && typeof cancellation.catch === 'function') {
      cancellation.catch(() => {});
    }
  } catch {}
}

async function readMetadataBody(response, setReader, wasTimedOut) {
  if (!response || response.ok !== true) {
    cancelMetadataBody(response);
    return undefined;
  }

  const contentLength = response.headers?.get?.('content-length');
  if (contentLength !== null && contentLength !== undefined) {
    const declaredLength = contentLength.trim();
    if (!/^\d+$/.test(declaredLength) || Number(declaredLength) > maxMetadataBytes) {
      cancelMetadataBody(response);
      return undefined;
    }
  }

  if (!response.body || typeof response.body.getReader !== 'function') {
    cancelMetadataBody(response);
    return undefined;
  }

  let reader;
  try {
    reader = response.body.getReader();
    setReader(reader);
    if (wasTimedOut()) {
      cancelMetadataBody(response, reader);
      return undefined;
    }

    const chunks = [];
    let byteLength = 0;
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      if (!(result.value instanceof Uint8Array)) {
        cancelMetadataBody(response, reader);
        return undefined;
      }

      byteLength += result.value.byteLength;
      if (byteLength > maxMetadataBytes) {
        cancelMetadataBody(response, reader);
        return undefined;
      }
      chunks.push(Buffer.from(result.value));
    }

    return JSON.parse(Buffer.concat(chunks, byteLength).toString('utf8'));
  } finally {
    try {
      reader?.releaseLock();
    } catch {}
  }
}

async function latestVersion(fetchImpl) {
  if (typeof fetchImpl !== 'function' || typeof AbortController !== 'function') return undefined;

  const controller = new AbortController();
  let activeResponse;
  let activeReader;
  let timedOut = false;
  let timer;
  const timeout = new Promise((resolve) => {
    timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
      cancelMetadataBody(activeResponse, activeReader);
      resolve(undefined);
    }, lookupDeadlineMs);
  });

  const lookup = Promise.resolve().then(async () => {
    const response = await fetchImpl(latestUrl, {
      method: 'GET',
      credentials: 'omit',
      redirect: 'error',
      signal: controller.signal,
    });
    activeResponse = response;
    if (timedOut) {
      cancelMetadataBody(response);
      return undefined;
    }

    return readMetadataBody(
      response,
      (reader) => { activeReader = reader; },
      () => timedOut,
    );
  });

  let metadata;
  try {
    metadata = await Promise.race([lookup, timeout]);
  } catch {
    return undefined;
  } finally {
    clearTimeout(timer);
  }

  if (
    !metadata
    || typeof metadata !== 'object'
    || Array.isArray(metadata)
    || metadata.name !== 'noesora'
    || !validVersion(metadata.version)
  ) {
    return undefined;
  }
  return metadata.version;
}

function askForConsent(stdin, stderr, version) {
  return new Promise((resolve) => {
    let settled = false;
    let interfaceInstance;
    const finish = (answer) => {
      if (settled) return;
      settled = true;
      interfaceInstance?.close();
      resolve(answer);
    };

    try {
      interfaceInstance = createInterface({
        input: stdin,
        output: stderr,
        terminal: true,
        crlfDelay: Infinity,
      });
      interfaceInstance.once('SIGINT', () => finish('cancelled'));
      interfaceInstance.once('close', () => finish('eof'));
      interfaceInstance.question(
        `Noesora ${version} is available. Update now? [y/N] `,
        (answer) => finish(
          typeof answer === 'string' && /^y(?:es)?$/i.test(answer.trim()) ? 'yes' : 'no',
        ),
      );
    } catch {
      finish('eof');
    }
  });
}

function installArguments(prefix, version) {
  return [
    'install',
    '--global',
    '--prefix',
    prefix,
    `--registry=${registryUrl}`,
    '--ignore-scripts=false',
    '--no-audit',
    '--no-fund',
    '--update-notifier=false',
    `noesora@${version}`,
  ];
}

function runNpmManager(executable, args, options) {
  return spawnSync(executable, args, options);
}

function reportUpdateFailure(stderr, result, { verificationFailed = false, refuseLaunch = false } = {}) {
  if (verificationFailed) {
    const message = refuseLaunch
      ? 'noesora: update verification failed; refusing to launch an unverified CLI.\n'
      : 'noesora: update verification failed; continuing with the installed CLI.\n';
    stderr.write(message);
    return;
  }

  const status = Number.isInteger(result?.status) ? ` (npm exit ${result.status})` : '';
  const message = refuseLaunch
    ? `noesora: update failed${status}; verification failed; refusing to launch an unverified CLI.\n`
    : `noesora: update failed${status}; continuing with the installed CLI.\n`;
  stderr.write(message);
}

async function maybeUpdate({
  args = [],
  packageRoot,
  stdin = process.stdin,
  stdout = process.stdout,
  stderr = process.stderr,
  fetchImpl = globalThis.fetch,
  managerRunner = runNpmManager,
} = {}) {
  if (!isEligible(args, stdin, stdout, stderr)) return 'skipped';

  const owner = resolveGlobalOwner(packageRoot);
  if (!owner) return 'skipped';

  const version = await latestVersion(fetchImpl);
  if (!version || !semver.gt(version, owner.version)) return 'skipped';

  const answer = await askForConsent(stdin, stderr, version);
  if (answer === 'cancelled') return 'cancelled';
  if (answer !== 'yes') return 'declined';

  const current = packageManifest(owner.packageRoot);
  const executable = executableInfo(owner.packageRoot);
  if (
    !isPlainDirectory(owner.packageRoot)
    || current?.name !== 'noesora'
    || current.version !== owner.version
    || !executable
  ) {
    return 'skipped';
  }

  const originalHash = await hashExecutable(executable);
  if (!originalHash) {
    reportUpdateFailure(stderr, undefined, { verificationFailed: true });
    return 'failed';
  }

  const argsForManager = [process.execPath, [owner.npmCli, ...installArguments(owner.prefix, version)]];
  const managerEnvironment = { ...process.env, npm_execpath: owner.npmCli };
  // Prevent the installer's loopback-only fixture override from steering a user update.
  delete managerEnvironment.NOESORA_RELEASE_BASE_URL;
  const managerOptions = {
    cwd: owner.runtimeRoot,
    env: managerEnvironment,
    stdio: 'ignore',
    windowsHide: true,
  };

  let result;
  try {
    result = await managerRunner(...argsForManager, managerOptions);
  } catch {
    result = undefined;
  }

  if (!result || result.error || result.status !== 0) {
    const safeToContinue = await isCurrentInstallUsable(owner, originalHash);
    reportUpdateFailure(stderr, result, { refuseLaunch: !safeToContinue });
    return safeToContinue ? 'failed' : 'verification-failed';
  }

  const updated = packageManifest(owner.packageRoot);
  if (
    !isPlainDirectory(owner.packageRoot)
    || updated?.name !== 'noesora'
    || updated.version !== version
    || !hasUsableBinary(owner.packageRoot)
  ) {
    const safeToContinue = await isCurrentInstallUsable(owner, originalHash);
    reportUpdateFailure(stderr, result, {
      verificationFailed: true,
      refuseLaunch: !safeToContinue,
    });
    return safeToContinue ? 'failed' : 'verification-failed';
  }

  return 'updated';
}

module.exports = { maybeUpdate };

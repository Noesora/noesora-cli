import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { after, before, test } from 'node:test';
import { createServer } from 'node:http';
import { existsSync, realpathSync } from 'node:fs';
import { chmod, copyFile, cp, lstat, mkdtemp, mkdir, readFile, readdir, rm, stat, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { installBinary, targetFor } from '../scripts/install-binary.mjs';

const assetBytes = Buffer.from('deterministic native binary fixture\0');
const assetDigest = createHash('sha256').update(assetBytes).digest('hex');
const fixturePlatform = process.platform === 'win32' ? 'win32' : 'linux';
const fixtureArch = process.arch === 'arm64' ? 'arm64' : 'x64';
const fixtureTarget = `${fixturePlatform}-${fixtureArch}`;
const assetName = `noesora-${fixtureTarget}${fixturePlatform === 'win32' ? '.exe' : ''}`;
const binaryName = fixturePlatform === 'win32' ? 'noesora.exe' : 'noesora';
const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64'];
let releaseBase;
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const semverRoot = path.join(repositoryRoot, 'node_modules', 'semver');

function pathExecutable(name) {
  for (const directory of (process.env.PATH || '').split(path.delimiter)) {
    if (!directory) continue;
    const candidate = path.join(directory, name);
    try {
      return realpathSync(candidate);
    } catch {}
  }
  return undefined;
}

function resolveNpmCli() {
  for (const candidate of [process.env.npm_execpath, pathExecutable('npm'), pathExecutable('npm.cmd')]) {
    if (!candidate) continue;
    try {
      const resolved = realpathSync(candidate);
      if (path.basename(resolved) === 'npm-cli.js') return resolved;
    } catch {}
  }
  return undefined;
}


const npmCliPath = resolveNpmCli();
const pythonPath = process.platform === 'win32' ? undefined : pathExecutable('python3');
const ptyAvailable = (() => {
  if (!pythonPath || !['darwin', 'linux'].includes(process.platform)) return false;
  const result = spawnSync(pythonPath, [
    '-c',
    'import os, pty; master, slave = pty.openpty(); os.close(master); os.close(slave)',
  ], { encoding: 'utf8', timeout: 5000 });
  return !result.error && result.status === 0;
})();
const integrationSkipReason = ['darwin', 'linux'].includes(process.platform)
  ? false
  : 'updater package-manager integration requires macOS or Linux';
const ptySkipReason = integrationSkipReason;

let updateFixturesPromise;

function checksumManifest(mode) {
  return targets
    .filter((target) => !(mode === 'missing-checksum' && target === fixtureTarget))
    .map((target) => {
      const name = `noesora-${target}${target === 'win32-x64' ? '.exe' : ''}`;
      const digest = mode === 'wrong-hash' && target === fixtureTarget ? '0'.repeat(64) : assetDigest;
      return `${digest}  ${name}`;
    })
    .join('\n') + '\n';
}

const server = createServer((request, response) => {
  const url = new URL(request.url || '/', 'http://127.0.0.1');
  const [mode, name] = url.pathname.split('/').filter(Boolean);
  if (request.method !== 'GET') {
    response.writeHead(405).end();
    return;
  }
  if (name === 'SHA256SUMS') {
    if (mode === 'checksum-unavailable') {
      response.writeHead(503).end();
      return;
    }
    response.writeHead(200, { 'content-type': 'text/plain' }).end(checksumManifest(mode));
    return;
  }
  if (name === assetName) {
    if (mode === 'missing-asset') {
      response.writeHead(404).end();
      return;
    }
    if (mode === 'empty-asset') {
      response.writeHead(200, { 'content-type': 'application/octet-stream' }).end();
      return;
    }
    response.writeHead(200, { 'content-type': 'application/octet-stream' }).end(assetBytes);
    return;
  }
  response.writeHead(404).end();
});

before(async () => {
  if (['darwin', 'linux'].includes(process.platform)) {
    assert.ok(npmCliPath, 'POSIX updater integration requires a resolvable npm-cli.js');
    assert.ok(existsSync(path.join(semverRoot, 'package.json')), 'POSIX updater integration requires project-local semver');
    assert.ok(pythonPath, 'POSIX updater integration requires Python 3');
    assert.ok(ptyAvailable, 'POSIX updater integration requires Python POSIX PTY support');
  }
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  assert(address && typeof address === 'object');
  releaseBase = `http://127.0.0.1:${address.port}`;
});

after(async () => {
  if (server.listening) await new Promise((resolve) => server.close(resolve));
});

async function withPackageRoot(run) {
  const packageRoot = await mkdtemp(path.join(os.tmpdir(), 'noesora-installer-test-'));
  try {
    await writeFile(path.join(packageRoot, 'package.json'), JSON.stringify({ version: '0.1.0' }));
    return await run(packageRoot);
  } finally {
    await rm(packageRoot, { recursive: true, force: true });
  }
}

async function seedExistingBinary(packageRoot) {
  const directory = path.join(packageRoot, 'vendor', fixtureTarget);
  const binaryPath = path.join(directory, binaryName);
  await mkdir(directory, { recursive: true });
  await writeFile(binaryPath, 'previous-good-binary');
  if (fixturePlatform !== 'win32') await chmod(binaryPath, 0o755);
  return { directory, binaryPath };
}

async function installMode(packageRoot, mode, options = {}) {
  return installBinary({
    packageRoot,
    platform: fixturePlatform,
    arch: fixtureArch,
    baseUrl: `${releaseBase}/${mode}`,
    ...options,
  });
}
const UPDATE_VERSION_OLD = '1.0.0';
const UPDATE_VERSION_NEW = '2.0.0';
let nextRunId = 0;

function npmEnvironment(workspace, prefix) {
  const pathEntries = [
    path.join(prefix, 'bin'),
    path.dirname(process.execPath),
    process.env.PATH || '',
  ];
  return {
    PATH: pathEntries.join(path.delimiter),
    HOME: workspace,
    TMPDIR: workspace,
    TERM: 'xterm-256color',
    CI: '1',
    NO_COLOR: '1',
    npm_config_userconfig: path.join(workspace, 'user.npmrc'),
    npm_config_globalconfig: path.join(workspace, 'global.npmrc'),
    npm_config_cache: path.join(workspace, 'npm-cache'),
    npm_config_prefix: prefix,
  };
}

async function prepareNpmConfig(workspace) {
  await writeFile(path.join(workspace, 'user.npmrc'), '');
  await writeFile(path.join(workspace, 'global.npmrc'), '');
}

function runNpm(args, { cwd, env }) {
  const result = spawnSync(process.execPath, [npmCliPath, ...args], {
    cwd,
    env,
    encoding: 'utf8',
    maxBuffer: 8 * 1024 * 1024,
    timeout: 120_000,
  });
  assert.ifError(result.error);
  assert.equal(
    result.status,
    0,
    `npm ${args.join(' ')} failed (${result.status}):\n${result.stdout}\n${result.stderr}`,
  );
  return result;
}

function fixturePreloaderSource() {
  return String.raw`'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { ReadableStream } = require('node:stream/web');

function record(file, entry) {
  if (file) fs.appendFileSync(file, JSON.stringify(entry) + '\n');
}

function sameFile(candidate, expected) {
  if (!candidate || !expected) return false;
  try {
    return fs.realpathSync(path.resolve(candidate)) === fs.realpathSync(path.resolve(expected));
  } catch {
    return false;
  }
}

function observe(stream, name) {
  const originalWrite = stream.write;
  stream.write = function () {
    const chunk = arguments[0];
    const encoding = typeof arguments[1] === 'string' ? arguments[1] : 'utf8';
    const text = Buffer.isBuffer(chunk) ? chunk.toString(encoding) : String(chunk);
    record(process.env.NOESORA_TEST_EVENT_LOG, { type: 'stream-write', stream: name, text });
    const result = originalWrite.apply(this, arguments);
    if (
      name === 'stderr'
      && text.includes('Update now?')
      && process.env.NOESORA_TEST_ALTERNATE_PREFIX
    ) {
      process.env.npm_config_prefix = process.env.NOESORA_TEST_ALTERNATE_PREFIX;
      record(process.env.NOESORA_TEST_EVENT_LOG, {
        type: 'prefix-switched',
        prefix: process.env.NOESORA_TEST_ALTERNATE_PREFIX,
      });
    }
    return result;
  };
}

const isLauncher = sameFile(process.argv[1], process.env.NOESORA_TEST_LAUNCHER);
if (isLauncher) {
  record(process.env.NOESORA_TEST_EVENT_LOG, {
    type: 'tty',
    stdin: Boolean(process.stdin.isTTY),
    stdout: Boolean(process.stdout.isTTY),
    stderr: Boolean(process.stderr.isTTY),
  });
  observe(process.stdout, 'stdout');
  observe(process.stderr, 'stderr');
  if (process.env.NOESORA_TEST_EXPECTED_NODE) {
    record(process.env.NOESORA_TEST_EVENT_LOG, {
      type: 'runtime-node-match',
      matches: sameFile(process.execPath, process.env.NOESORA_TEST_EXPECTED_NODE),
    });
  }
  globalThis.fetch = async function (input, init = {}) {
    const startedAt = process.hrtime.bigint();
    const url = typeof input === 'string' ? input : String(input.url);
    const requestHeaders = init.headers || (input && typeof input === 'object' && input.headers);
    const headerNames = !requestHeaders
      ? []
      : typeof requestHeaders.keys === 'function'
        ? Array.from(requestHeaders.keys())
        : Array.isArray(requestHeaders)
          ? requestHeaders.map((header) => header[0])
          : Object.keys(requestHeaders);
    const sensitiveHeaderNames = headerNames
      .map((name) => String(name).toLowerCase())
      .filter((name) => /auth|cookie|token|api[-_]?key|secret|credential|session/.test(name))
      .sort();
    const requestBody = init.body === undefined && input && typeof input === 'object'
      ? input.body
      : init.body;
    const contextCanaries = process.env.NOESORA_TEST_CONTEXT_CANARIES
      ? JSON.parse(process.env.NOESORA_TEST_CONTEXT_CANARIES)
      : [];
    if (contextCanaries.length) {
      const headerValues = !requestHeaders
        ? []
        : typeof requestHeaders.values === 'function'
          ? Array.from(requestHeaders.values(), String)
          : Array.isArray(requestHeaders)
            ? requestHeaders.map((header) => String(header[1]))
            : Object.values(requestHeaders).map(String);
      const bodyText = typeof requestBody === 'string'
        ? requestBody
        : Buffer.isBuffer(requestBody)
          ? requestBody.toString('utf8')
          : requestBody instanceof URLSearchParams
            ? requestBody.toString()
            : '';
      const contextCanaryPresent = [url, ...headerValues, bodyText].some((value) =>
        contextCanaries.some((canary) => value.includes(canary)));
      record(process.env.NOESORA_TEST_REQUEST_LOG, {
        contextCanaryPresent,
        bodyPresent: requestBody !== undefined && requestBody !== null,
      });
    } else {
      record(process.env.NOESORA_TEST_REQUEST_LOG, {
        url,
        method: init.method || (input && input.method) || 'GET',
        credentials: init.credentials || (input && input.credentials) || null,
        redirect: init.redirect || (input && input.redirect) || null,
        hasSignal: Boolean(init.signal),
        sensitiveHeaderNames,
        bodyPresent: requestBody !== undefined && requestBody !== null,
      });
    }
    if (process.env.NOESORA_TEST_STALL_METADATA === '1') {
      return new Promise((_, reject) => {
        const abort = () => {
          clearTimeout(fallback);
          record(process.env.NOESORA_TEST_EVENT_LOG, {
            type: 'metadata-aborted',
            elapsedMs: Number(process.hrtime.bigint() - startedAt) / 1e6,
            signalAborted: Boolean(init.signal.aborted),
          });
          reject(new Error('controlled metadata request aborted'));
        };
        const fallback = setTimeout(() => {
          record(process.env.NOESORA_TEST_EVENT_LOG, {
            type: 'metadata-stall-fallback',
            elapsedMs: Number(process.hrtime.bigint() - startedAt) / 1e6,
            signalAborted: Boolean(init.signal && init.signal.aborted),
          });
          reject(new Error('controlled metadata fixture was not aborted'));
        }, 4000);
        if (!init.signal) return;
        if (init.signal.aborted) abort();
        else init.signal.addEventListener('abort', abort, { once: true });
      });
    }
    if (process.env.NOESORA_TEST_OFFLINE === '1') {
      throw new TypeError('controlled offline fixture');
    }
    const status = Number(process.env.NOESORA_TEST_METADATA_STATUS || '200');
    const body = process.env.NOESORA_TEST_METADATA || '{}';
    if (process.env.NOESORA_TEST_OVERSIZED_METADATA === '1') {
      const bytes = Buffer.concat([
        Buffer.from('{"name":"noesora","version":"2.0.0","padding":"'),
        Buffer.alloc(200_000, 0x61),
        Buffer.from('"}'),
      ]);
      let offset = 0;
      const stream = new ReadableStream({
        pull(controller) {
          const end = Math.min(offset + 8192, bytes.length);
          controller.enqueue(bytes.subarray(offset, end));
          offset = end;
          if (offset === bytes.length) controller.close();
        },
        cancel() {
          record(process.env.NOESORA_TEST_EVENT_LOG, {
            type: 'metadata-body-cancelled',
            kind: 'oversized',
            bytesEnqueued: offset,
            totalBytes: bytes.length,
          });
        },
      });
      record(process.env.NOESORA_TEST_EVENT_LOG, {
        type: 'metadata-body-response-ready',
        kind: 'oversized',
        byteLength: bytes.length,
      });
      return new Response(stream, {
        status,
        headers: { 'content-type': 'application/json' },
      });
    }
    if (process.env.NOESORA_TEST_STALL_BODY === '1') {
      if (init.signal) {
        init.signal.addEventListener('abort', () => {
          record(process.env.NOESORA_TEST_EVENT_LOG, {
            type: 'metadata-body-signal-aborted',
            elapsedMs: Number(process.hrtime.bigint() - startedAt) / 1e6,
          });
        }, { once: true });
      }
      const stream = new ReadableStream({
        cancel() {
          record(process.env.NOESORA_TEST_EVENT_LOG, {
            type: 'metadata-body-cancelled',
            kind: 'stalled',
          });
        },
      });
      record(process.env.NOESORA_TEST_EVENT_LOG, {
        type: 'metadata-body-response-ready',
        kind: 'stalled',
      });
      return new Response(stream, {
        status,
        headers: { 'content-type': 'application/json' },
      });
    }
    return new Response(body, {
      status,
      headers: { 'content-type': 'application/json' },
    });
  };
}

if (sameFile(process.argv[1], process.env.NOESORA_TEST_NPM_CLI)) {
  const args = process.argv.slice(2);
  const specIndex = args.findIndex((arg) => arg === 'noesora' || arg.startsWith('noesora@'));
  if (specIndex !== -1) {
    const packageSpec = args[specIndex];
    record(process.env.NOESORA_TEST_EVENT_LOG, {
      type: 'manager',
      packageSpec,
      args,
      npmConfigPrefix: process.env.npm_config_prefix || null,
    });
    if (process.env.NOESORA_TEST_CORRUPT_BINARY) {
      fs.copyFileSync(process.env.NOESORA_TEST_CORRUPT_SCRIPT, process.env.NOESORA_TEST_CORRUPT_BINARY);
      fs.chmodSync(process.env.NOESORA_TEST_CORRUPT_BINARY, 0o755);
      process.exit(42);
    }
    process.argv[specIndex + 2] = process.env.NOESORA_TEST_ARCHIVE;
  }
}

`;
}

function fixturePostinstallSource() {
  return String.raw`'use strict';
const fs = require('node:fs');
const path = require('node:path');

const packageRoot = path.resolve(__dirname, '..');
const manifest = JSON.parse(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
if (process.env.NOESORA_TEST_FAIL_POSTINSTALL === manifest.version) {
  process.stderr.write('controlled lifecycle failure\n');
  process.exit(42);
}

const target = process.platform + '-' + process.arch;
const binaryName = process.platform === 'win32' ? 'noesora.exe' : 'noesora';
const binaryPath = path.join(packageRoot, 'vendor', target, binaryName);
fs.mkdirSync(path.dirname(binaryPath), { recursive: true });
const executable = [
  '#!/usr/bin/env node',
  "'use strict';",
  "const fs = require('node:fs');",
  'const version = ' + JSON.stringify(manifest.version) + ';',
  'const args = process.argv.slice(2);',
  "if (process.env.NOESORA_TEST_EVENT_LOG) fs.appendFileSync(process.env.NOESORA_TEST_EVENT_LOG, JSON.stringify({ type: 'native-start', version, argCount: args.length }) + '\\n');",
  "if (args.includes('--json')) { process.stdout.write(JSON.stringify({ fixture: 'json', version, args }) + '\\n'); }",
  "else if (args[0] === 'mcp') { process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: 1, result: { protocolVersion: '2024-11-05', capabilities: {} } }) + '\\n'); }",
  "else { process.stdout.write('fixture-cli:' + version + (args.length ? ' ' + args.join(' ') : '') + '\\n'); }",
  '',
].join('\n');
fs.writeFileSync(binaryPath, executable);
if (process.platform !== 'win32') fs.chmodSync(binaryPath, 0o755);
`;
}

async function packFixturePackage(root, version, npmEnv) {
  const packageRoot = path.join(root, `package-${version}`);
  const binDirectory = path.join(packageRoot, 'bin');
  const scriptsDirectory = path.join(packageRoot, 'scripts');
  const semverManifest = JSON.parse(await readFile(path.join(semverRoot, 'package.json'), 'utf8'));
  await mkdir(binDirectory, { recursive: true });
  await mkdir(scriptsDirectory, { recursive: true });
  await cp(semverRoot, path.join(packageRoot, 'node_modules', 'semver'), { recursive: true });
  await writeFile(path.join(binDirectory, 'noesora.js'), await readFile(path.join(repositoryRoot, 'bin', 'noesora.js')));
  await writeFile(path.join(scriptsDirectory, 'create-fixture-binary.cjs'), fixturePostinstallSource());
  const updaterHelper = path.join(repositoryRoot, 'scripts', 'update-check.cjs');
  if (existsSync(updaterHelper)) {
    await writeFile(path.join(scriptsDirectory, 'update-check.cjs'), await readFile(updaterHelper));
  }
  await writeFile(path.join(packageRoot, 'package.json'), JSON.stringify({
    name: 'noesora',
    version,
    type: 'commonjs',
    engines: { node: '>=18' },
    bin: { noesora: 'bin/noesora.js' },
    files: ['bin', 'scripts'],
    dependencies: { semver: semverManifest.version },
    bundledDependencies: ['semver'],
    scripts: { postinstall: 'node scripts/create-fixture-binary.cjs' },
  }, null, 2) + '\n');
  const archiveDirectory = path.join(root, 'archives');
  await mkdir(archiveDirectory, { recursive: true });
  runNpm(['pack', '--ignore-scripts', '--pack-destination', archiveDirectory], {
    cwd: packageRoot,
    env: npmEnv,
  });
  const archive = path.join(archiveDirectory, `noesora-${version}.tgz`);
  assert.ok(existsSync(archive), `npm pack did not create ${path.basename(archive)}`);
  return archive;
}

async function createUpdateFixtures() {
  const root = await mkdtemp(path.join(os.tmpdir(), 'noesora-update-fixtures-'));
  try {
    await prepareNpmConfig(root);
    const packEnv = npmEnvironment(root, path.join(root, 'unused-prefix'));
    const preloaderPath = path.join(root, 'test-preload.cjs');
    await writeFile(preloaderPath, fixturePreloaderSource());
    const oldArchive = await packFixturePackage(root, UPDATE_VERSION_OLD, packEnv);
    const newArchive = await packFixturePackage(root, UPDATE_VERSION_NEW, packEnv);
    return { root, oldArchive, newArchive, preloaderPath };
  } catch (error) {
    await rm(root, { recursive: true, force: true });
    throw error;
  }
}

function ensureUpdateFixtures() {
  if (!updateFixturesPromise) updateFixturesPromise = createUpdateFixtures();
  return updateFixturesPromise;
}

async function withUpdatePrefix(run) {
  const fixtures = await ensureUpdateFixtures();
  const workspace = await mkdtemp(path.join(os.tmpdir(), 'noesora-update-prefix-'));
  try {
    await prepareNpmConfig(workspace);
    const prefix = path.join(workspace, 'prefix');
    await mkdir(prefix);
    const baseEnv = npmEnvironment(workspace, prefix);
    runNpm([
      'install',
      '--global',
      '--prefix',
      prefix,
      '--no-audit',
      '--no-fund',
      fixtures.oldArchive,
    ], { cwd: workspace, env: baseEnv });
    const packageRoot = path.join(prefix, 'lib', 'node_modules', 'noesora');
    const launcherPath = path.join(prefix, 'bin', 'noesora');
    const binaryPath = path.join(
      packageRoot,
      'vendor',
      `${process.platform}-${process.arch}`,
      process.platform === 'win32' ? 'noesora.exe' : 'noesora',
    );
    assert.ok(existsSync(launcherPath), 'npm did not create the installed noesora launcher');
    assert.equal((await readPackageState(packageRoot, binaryPath)).version, UPDATE_VERSION_OLD);
    return await run({
      fixtures,
      workspace,
      prefix,
      packageRoot,
      launcherPath,
      binaryPath,
      baseEnv,
    });
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
}

async function readPackageState(packageRoot, binaryPath) {
  const manifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
  const binary = await readFile(binaryPath);
  return {
    version: manifest.version,
    binaryHash: createHash('sha256').update(binary).digest('hex'),
  };
}

async function readJsonLines(file) {
  try {
    const contents = await readFile(file, 'utf8');
    return contents.split('\n').filter(Boolean).map((line) => JSON.parse(line));
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
}

function appendNodeOptions(env, preloaderPath) {
  const preload = `--require ${JSON.stringify(preloaderPath)}`;
  env.NODE_OPTIONS = [env.NODE_OPTIONS, preload].filter(Boolean).join(' ');
}

// Use the same controlling-PTY boundary as scripts/tui-smoke.py. macOS script(1)
// rejects Node's socket-backed stdin; send answers only after the actual prompt.
const updatePtyDriver = String.raw`
import errno, json, os, pty, select, signal, sys, time

command = json.loads(sys.argv[1])
answer = sys.argv[2].encode() or b"\x04"
pid, master = pty.fork()
if pid == 0:
    try:
        os.execvpe(command[0], command, os.environ)
    except BaseException:
        os._exit(127)

output = bytearray()
answered = False
eof = False
exit_status = None
deadline = time.monotonic() + 110

def interrupted(_signal, _frame):
    raise RuntimeError("owned updater PTY interrupted")

signal.signal(signal.SIGTERM, interrupted)
try:
    while exit_status is None or not eof:
        if time.monotonic() >= deadline:
            raise RuntimeError("owned updater PTY timed out")
        if not eof:
            ready, _, _ = select.select([master], [], [], 0.02)
            if ready:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                if not data:
                    eof = True
                else:
                    output.extend(data)
                    if len(output) > 8 * 1024 * 1024:
                        raise RuntimeError("owned updater PTY output limit exceeded")
                    if not answered and b"Update now? [y/N]" in output:
                        os.write(master, answer)
                        answered = True
        else:
            time.sleep(0.01)
        if exit_status is None:
            found, status = os.waitpid(pid, os.WNOHANG)
            if found == pid:
                exit_status = os.waitstatus_to_exitcode(status)
finally:
    if exit_status is None:
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
    os.close(master)
sys.stdout.buffer.write(output)
sys.exit(exit_status if exit_status >= 0 else 128 - exit_status)
`;

async function runFixture(ctx, {
  metadata = { name: 'noesora', version: UPDATE_VERSION_NEW },
  status = 200,
  offline = false,
  stallMetadata = false,
  oversizedMetadata = false,
  stallBody = false,
  failVersion,
  alternatePrefix,
  corruptNativeWith,
  input = '',
  args = [],
  pty = true,
  cwd = ctx.workspace,
  contextCanaries,
} = {}) {
  const id = ++nextRunId;
  const eventLog = path.join(ctx.workspace, `events-${id}.jsonl`);
  const requestLog = path.join(ctx.workspace, `requests-${id}.jsonl`);
  const env = { ...ctx.baseEnv };
  env.NOESORA_TEST_LAUNCHER = ctx.launcherPath;
  env.NOESORA_TEST_NPM_CLI = npmCliPath;
  env.NOESORA_TEST_ARCHIVE = ctx.fixtures.newArchive;
  env.NOESORA_TEST_EVENT_LOG = eventLog;
  env.NOESORA_TEST_REQUEST_LOG = requestLog;
  env.NOESORA_TEST_METADATA = JSON.stringify(metadata);
  env.NOESORA_TEST_METADATA_STATUS = String(status);
  env.NOESORA_TEST_OFFLINE = offline ? '1' : '0';
  env.NOESORA_TEST_STALL_METADATA = stallMetadata ? '1' : '0';
  env.NOESORA_TEST_OVERSIZED_METADATA = oversizedMetadata ? '1' : '0';
  env.NOESORA_TEST_STALL_BODY = stallBody ? '1' : '0';
  if (alternatePrefix) env.NOESORA_TEST_ALTERNATE_PREFIX = alternatePrefix;
  if (contextCanaries) env.NOESORA_TEST_CONTEXT_CANARIES = JSON.stringify(contextCanaries);
  env.NOESORA_TEST_FAIL_POSTINSTALL = failVersion || '';
  if (corruptNativeWith) {
    env.NOESORA_TEST_CORRUPT_BINARY = ctx.binaryPath;
    env.NOESORA_TEST_CORRUPT_SCRIPT = corruptNativeWith;
  }
  appendNodeOptions(env, ctx.fixtures.preloaderPath);

  const result = pty
    ? spawnSync(pythonPath, ['-c', updatePtyDriver, JSON.stringify([ctx.launcherPath, ...args]), input], {
      cwd,
      env,
      encoding: 'utf8',
      maxBuffer: 8 * 1024 * 1024,
      timeout: 120_000,
    })
    : spawnSync(ctx.launcherPath, args, {
      cwd,
      env,
      encoding: 'utf8',
      input,
      maxBuffer: 8 * 1024 * 1024,
      timeout: 120_000,
    });
  assert.ifError(result.error);
  return {
    result,
    events: await readJsonLines(eventLog),
    requests: await readJsonLines(requestLog),
  };
}

function streamOutput(events, stream) {
  return events
    .filter((event) => event.type === 'stream-write' && event.stream === stream)
    .map((event) => event.text)
    .join('');
}

function assertOneNativeRun(events, version) {
  const runs = events.filter((event) => event.type === 'native-start');
  assert.equal(runs.length, 1, 'the launcher must start the native CLI exactly once');
  assert.equal(runs[0].version, version);
}

function assertLatestRequest(requests) {
  assert.equal(requests.length, 1, 'eligible startup should make one metadata request');
  assert.deepEqual(requests[0], {
    url: 'https://registry.npmjs.org/noesora/latest',
    method: 'GET',
    credentials: 'omit',
    redirect: 'error',
    hasSignal: true,
    sensitiveHeaderNames: [],
    bodyPresent: false,
  });
}

function assertNoManagerCall(events) {
  assert.equal(events.filter((event) => event.type === 'manager').length, 0);
}

function assertManagerCall(events, prefix) {
  const calls = events.filter((event) => event.type === 'manager');
  assert.equal(calls.length, 1, 'an affirmative answer must invoke the package manager once');
  assert.equal(calls[0].packageSpec, `noesora@${UPDATE_VERSION_NEW}`);
  assert.ok(calls[0].args.includes('--global') || calls[0].args.includes('-g'));
  const prefixIndex = calls[0].args.indexOf('--prefix');
  const prefixArgument = prefixIndex !== -1
    ? calls[0].args[prefixIndex + 1]
    : calls[0].args.find((arg) => arg.startsWith('--prefix='))?.slice('--prefix='.length);
  assert.ok(prefixArgument, 'the update must pass an explicit confirmed prefix');
  assert.equal(
    realpathSync(prefixArgument),
    realpathSync(prefix),
    'the update must remain bound to the confirmed temporary prefix',
  );
  assert.ok(
    calls[0].args.includes('--registry=https://registry.npmjs.org/')
      || calls[0].args.includes('--registry=https://registry.npmjs.org')
      || calls[0].args.some((arg, index) =>
        arg === '--registry'
        && ['https://registry.npmjs.org/', 'https://registry.npmjs.org'].includes(calls[0].args[index + 1])),
    'the package manager must use the fixed public registry',
  );
}

async function verifyNoUpdateCase(ctx, options, { requestCount = 1, args = [] } = {}) {
  const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
  const { result, events, requests } = await runFixture(ctx, { ...options, args });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.equal(requests.length, requestCount);
  if (requestCount === 1) assertLatestRequest(requests);
  assertNoManagerCall(events);
  assert.equal(streamOutput(events, 'stderr'), '');
  assert.equal(streamOutput(events, 'stdout').includes('Update now?'), false);
  assert.equal(result.stdout.includes('Update now?'), false);
  const after = await readPackageState(ctx.packageRoot, ctx.binaryPath);
  assert.deepEqual(after, before);
  assertOneNativeRun(events, UPDATE_VERSION_OLD);
}


test('supported targets are explicit and unsupported hosts fail', () => {
  assert.equal(targetFor('darwin', 'arm64'), 'darwin-arm64');
  assert.equal(targetFor('linux', 'x64'), 'linux-x64');
  assert.equal(targetFor('win32', 'x64'), 'win32-x64');
  assert.throws(() => targetFor('freebsd', 'x64'), /Unsupported target/);
});

test('verified bytes atomically replace an existing binary', async () => {
  await withPackageRoot(async (packageRoot) => {
    const { binaryPath } = await seedExistingBinary(packageRoot);
    await installMode(packageRoot, 'good');
    assert.deepEqual(await readFile(binaryPath), assetBytes);
    if (fixturePlatform !== 'win32' && process.platform !== 'win32') {
      assert.notEqual((await stat(binaryPath)).mode & 0o111, 0);
    }
  });
});

for (const [mode, message] of [
  ['checksum-unavailable', /Checksum manifest download failed with HTTP 503/],
  ['missing-checksum', /Checksum manifest has no entry/],
  ['wrong-hash', /SHA-256 mismatch/],
  ['empty-asset', /Downloaded binary is empty/],
  ['missing-asset', /Binary download failed with HTTP 404/],
]) {
  test(`${mode} leaves the previous binary intact`, async () => {
    await withPackageRoot(async (packageRoot) => {
      const { directory, binaryPath } = await seedExistingBinary(packageRoot);
      await assert.rejects(installMode(packageRoot, mode), message);
      assert.equal(await readFile(binaryPath, 'utf8'), 'previous-good-binary');
      assert.deepEqual(await readdir(directory), [binaryName]);
    });
  });
}

test('a failed replacement leaves the previous binary intact and removes staging bytes', async () => {
  await withPackageRoot(async (packageRoot) => {
    const { directory, binaryPath } = await seedExistingBinary(packageRoot);
    const replacementError = new Error('simulated replacement failure');
    await assert.rejects(
      installMode(packageRoot, 'good', { renameImpl: async () => { throw replacementError; } }),
      (error) => error === replacementError,
    );
    assert.equal(await readFile(binaryPath, 'utf8'), 'previous-good-binary');
    assert.deepEqual(await readdir(directory), [binaryName]);
  });
});

test('unsupported target fails before it can replace a binary', async () => {
  await withPackageRoot(async (packageRoot) => {
    const { binaryPath } = await seedExistingBinary(packageRoot);
    await assert.rejects(
      installBinary({
        packageRoot,
        platform: 'freebsd',
        arch: 'x64',
        baseUrl: `${releaseBase}/good`,
      }),
      /Unsupported target/,
    );
    assert.equal(await readFile(binaryPath, 'utf8'), 'previous-good-binary');
  });
});
test(
  'packaged npm-global launcher prompts for a newer release before its one old CLI run',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(ctx, { input: 'n\n' });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assert.match(
        streamOutput(events, 'stderr'),
        /Noesora 2\.0\.0 is available\. Update now\? \[y\/N\]/,
        'eligible npm-global TTY startup must ask before running the CLI',
      );
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assert.equal((result.stdout.match(/fixture-cli:1\.0\.0/g) || []).length, 1);
      assertLatestRequest(requests);
      assert.deepEqual(events.find((event) => event.type === 'tty'), {
        type: 'tty',
        stdin: true,
        stdout: true,
        stderr: true,
      });
      assert.equal(streamOutput(events, 'stdout').includes('Update now?'), false);
      assertNoManagerCall(events);
      const promptIndex = events.findIndex((event) =>
        event.type === 'stream-write'
        && event.stream === 'stderr'
        && event.text.includes('Update now?'));
      const nativeIndex = events.findIndex((event) => event.type === 'native-start');
      assert.ok(promptIndex !== -1 && promptIndex < nativeIndex, 'the prompt must precede the native CLI');
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);

test(
  'empty input and EOF decline without changing the installed package',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      for (const input of ['\n', '\u0004']) {
        const { result, events, requests } = await runFixture(ctx, { input });
        assert.equal(result.status, 0, result.stderr || result.stdout);
        assertLatestRequest(requests);
        assert.match(streamOutput(events, 'stderr'), /Update now\? \[y\/N\]/);
        assertNoManagerCall(events);
        assertOneNativeRun(events, UPDATE_VERSION_OLD);
        assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
      }
    });
  },
);

test(
  'yes answers install the exact release through npm and run the new CLI once',
  { skip: ptySkipReason },
  async () => {
    for (const input of ['y\n', ' YeS \n']) {
      await withUpdatePrefix(async (ctx) => {
        const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
        const { result, events, requests } = await runFixture(ctx, { input });
        assert.equal(result.status, 0, result.stderr || result.stdout);
        assertLatestRequest(requests);
        assertManagerCall(events, ctx.prefix);
        const after = await readPackageState(ctx.packageRoot, ctx.binaryPath);
        assert.equal(after.version, UPDATE_VERSION_NEW);
        assert.notEqual(after.binaryHash, before.binaryHash);
        assert.match(result.stdout, /fixture-cli:2\.0\.0/);
        assert.equal((result.stdout.match(/fixture-cli:2\.0\.0/g) || []).length, 1);
        assertOneNativeRun(events, UPDATE_VERSION_NEW);
        const promptIndex = events.findIndex((event) =>
          event.type === 'stream-write'
          && event.stream === 'stderr'
          && event.text.includes('Update now?'));
        const managerIndex = events.findIndex((event) => event.type === 'manager');
        const nativeIndex = events.findIndex((event) => event.type === 'native-start');
        assert.ok(promptIndex !== -1 && promptIndex < managerIndex && managerIndex < nativeIndex);
      });
    }
  },
);

test(
  'update stays bound to its confirmed prefix when npm_config_prefix changes at the prompt',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const originalBefore = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const alternatePrefix = path.join(ctx.workspace, 'alternate-prefix');
      await mkdir(alternatePrefix);
      const alternateEnv = npmEnvironment(ctx.workspace, alternatePrefix);
      runNpm([
        'install',
        '--global',
        '--prefix',
        alternatePrefix,
        '--no-audit',
        '--no-fund',
        ctx.fixtures.oldArchive,
      ], { cwd: ctx.workspace, env: alternateEnv });
      const alternatePackageRoot = path.join(alternatePrefix, 'lib', 'node_modules', 'noesora');
      const alternateBinaryPath = path.join(
        alternatePackageRoot,
        'vendor',
        `${process.platform}-${process.arch}`,
        process.platform === 'win32' ? 'noesora.exe' : 'noesora',
      );
      const alternateBefore = await readPackageState(alternatePackageRoot, alternateBinaryPath);
      const { result, events, requests } = await runFixture(ctx, {
        input: 'y\n',
        alternatePrefix,
      });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assertLatestRequest(requests);
      assertManagerCall(events, ctx.prefix);
      const promptIndex = events.findIndex((event) =>
        event.type === 'stream-write'
        && event.stream === 'stderr'
        && event.text.includes('Update now?'));
      const switchIndex = events.findIndex((event) => event.type === 'prefix-switched');
      const managerIndex = events.findIndex((event) => event.type === 'manager');
      const nativeIndex = events.findIndex((event) => event.type === 'native-start');
      const prefixSwitch = events[switchIndex];
      const managerCall = events[managerIndex];
      assert.ok(promptIndex !== -1 && promptIndex < switchIndex && switchIndex < managerIndex && managerIndex < nativeIndex);
      assert.equal(realpathSync(prefixSwitch.prefix), realpathSync(alternatePrefix));
      assert.equal(realpathSync(managerCall.npmConfigPrefix), realpathSync(alternatePrefix));
      const originalAfter = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      assert.equal(originalAfter.version, UPDATE_VERSION_NEW);
      assert.notEqual(originalAfter.binaryHash, originalBefore.binaryHash);
      assert.deepEqual(await readPackageState(alternatePackageRoot, alternateBinaryPath), alternateBefore);
      assert.match(result.stdout, /fixture-cli:2\.0\.0/);
      assertOneNativeRun(events, UPDATE_VERSION_NEW);
    });
  },
);

test(
  'current, older, invalid, missing, and offline metadata fail open without installing',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const cases = [
        { options: { metadata: { name: 'noesora', version: UPDATE_VERSION_OLD } } },
        { options: { metadata: { name: 'noesora', version: '0.9.0' } } },
        { options: { metadata: { name: 'not-noesora', version: UPDATE_VERSION_NEW } } },
        { options: { metadata: { name: 'noesora', version: '2.0' } } },
        { options: { metadata: { name: 'noesora', version: 'v2.0.0' } } },
        { options: { status: 404 } },
        { options: { offline: true } },
      ];
      for (const { options } of cases) {
        await verifyNoUpdateCase(ctx, options);
      }
      await verifyNoUpdateCase(ctx, {
        metadata: { name: 'noesora', version: UPDATE_VERSION_OLD },
      }, { args: ['tui'] });
      for (const args of [['--json'], ['mcp'], ['--help'], ['--version'], ['status']]) {
        await verifyNoUpdateCase(ctx, {}, { requestCount: 0, args });
      }
    });
  },
);

test(
  'piped JSON, MCP, help, version, and ordinary commands preserve stdout without update traffic',
  { skip: integrationSkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      for (const args of [[], ['--json'], ['mcp'], ['--help'], ['--version'], ['status'], ['tui']]) {
        const { result, events, requests } = await runFixture(ctx, { args, pty: false });
        assert.equal(result.status, 0, result.stderr || result.stdout);
        assert.deepEqual(requests, []);
        assertNoManagerCall(events);
        assert.deepEqual(events.find((event) => event.type === 'tty'), {
          type: 'tty',
          stdin: false,
          stdout: false,
          stderr: false,
        });
        assert.equal(streamOutput(events, 'stderr'), '');
        assert.equal(result.stderr, '');
        assertOneNativeRun(events, UPDATE_VERSION_OLD);
        if (args[0] === '--json') {
          const payload = { fixture: 'json', version: UPDATE_VERSION_OLD, args };
          assert.deepEqual(JSON.parse(result.stdout), payload);
          assert.equal(result.stdout, JSON.stringify(payload) + '\n');
        } else if (args[0] === 'mcp') {
          const payload = {
            jsonrpc: '2.0',
            id: 1,
            result: { protocolVersion: '2024-11-05', capabilities: {} },
          };
          assert.deepEqual(JSON.parse(result.stdout), payload);
          assert.equal(result.stdout, JSON.stringify(payload) + '\n');
        } else {
          assert.equal(result.stdout, `fixture-cli:1.0.0${args.length ? ` ${args.join(' ')}` : ''}\n`);
        }
      }
    });
  },
);

test(
  'failed npm lifecycle install preserves the old CLI and launches it once',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(ctx, {
        input: 'y\n',
        failVersion: UPDATE_VERSION_NEW,
      });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assertLatestRequest(requests);
      assertManagerCall(events, ctx.prefix);
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assert.match(
        result.stdout + result.stderr + streamOutput(events, 'stderr'),
        /controlled lifecycle failure|(?:update|npm).{0,32}(?:failed|failure|error)|(?:failed|failure|error).{0,32}(?:update|npm)/i,
        'the failed package-manager update must be visible to the user',
      );
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
    });
  },
);

test(
  'copied local package skips updater lookup and preserves the separate global installation',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const globalBefore = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const localPackageRoot = path.join(ctx.workspace, 'copied-noesora-package');
      await cp(ctx.packageRoot, localPackageRoot, { recursive: true });
      const localLauncher = path.join(localPackageRoot, 'bin', 'noesora.js');
      await chmod(localLauncher, 0o755);
      assert.ok((await lstat(localPackageRoot)).isDirectory());
      assert.equal((await lstat(localPackageRoot)).isSymbolicLink(), false);
      const localBinary = path.join(
        localPackageRoot,
        'vendor',
        `${process.platform}-${process.arch}`,
        process.platform === 'win32' ? 'noesora.exe' : 'noesora',
      );
      const localBefore = await readPackageState(localPackageRoot, localBinary);
      const { result, events, requests } = await runFixture(
        { ...ctx, launcherPath: localLauncher },
        { input: 'y\n' },
      );
      assert.equal(result.status, 0, result.stdout);
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assert.equal((result.stdout.match(/fixture-cli:1\.0\.0/g) || []).length, 1);
      assert.deepEqual(requests, []);
      assertNoManagerCall(events);
      assert.equal(streamOutput(events, 'stderr'), '');
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), globalBefore);
      assert.deepEqual(await readPackageState(localPackageRoot, localBinary), localBefore);
    });
  },
);

test(
  'npm-linked package skips updater lookup and preserves the actual prefix symlink and hashes',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const localPackageRoot = path.join(ctx.workspace, 'linked-noesora-package');
      await cp(ctx.packageRoot, localPackageRoot, { recursive: true });
      await chmod(path.join(localPackageRoot, 'bin', 'noesora.js'), 0o755);
      const localBinary = path.join(
        localPackageRoot,
        'vendor',
        `${process.platform}-${process.arch}`,
        process.platform === 'win32' ? 'noesora.exe' : 'noesora',
      );
      const localBefore = await readPackageState(localPackageRoot, localBinary);
      runNpm(['uninstall', '--global', '--prefix', ctx.prefix, 'noesora'], {
        cwd: ctx.workspace,
        env: ctx.baseEnv,
      });
      runNpm(['link'], {
        cwd: localPackageRoot,
        env: ctx.baseEnv,
      });
      const linkedPackageRoot = path.join(ctx.prefix, 'lib', 'node_modules', 'noesora');
      const linkedLauncher = path.join(ctx.prefix, 'bin', 'noesora');
      const linkedBinary = path.join(
        linkedPackageRoot,
        'vendor',
        `${process.platform}-${process.arch}`,
        process.platform === 'win32' ? 'noesora.exe' : 'noesora',
      );
      assert.ok((await lstat(linkedPackageRoot)).isSymbolicLink(), 'npm link must create a package symlink');
      assert.equal(realpathSync(linkedPackageRoot), realpathSync(localPackageRoot));
      const linkedBefore = await readPackageState(linkedPackageRoot, linkedBinary);
      const linkedLauncherTarget = realpathSync(linkedLauncher);
      const { result, events, requests } = await runFixture({
        ...ctx,
        launcherPath: linkedLauncher,
        packageRoot: linkedPackageRoot,
        binaryPath: linkedBinary,
      }, { input: 'y\n' });
      assert.equal(result.status, 0, result.stdout);
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assert.equal((result.stdout.match(/fixture-cli:1\.0\.0/g) || []).length, 1);
      assert.deepEqual(requests, []);
      assertNoManagerCall(events);
      assert.equal(streamOutput(events, 'stderr'), '');
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.ok((await lstat(linkedPackageRoot)).isSymbolicLink());
      assert.equal(realpathSync(linkedPackageRoot), realpathSync(localPackageRoot));
      assert.equal(realpathSync(linkedLauncher), linkedLauncherTarget);
      assert.deepEqual(await readPackageState(linkedPackageRoot, linkedBinary), linkedBefore);
      assert.deepEqual(await readPackageState(localPackageRoot, localBinary), localBefore);
    });
  },
);

test(
  'Ctrl-C at the update prompt cancels without installing or starting the native CLI',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(ctx, { input: '\u0003' });
      assertLatestRequest(requests);
      assert.equal(result.status, 130);
      assert.deepEqual(events.find((event) => event.type === 'tty'), {
        type: 'tty',
        stdin: true,
        stdout: true,
        stderr: true,
      });
      assert.match(streamOutput(events, 'stderr'), /Update now\? \[y\/N\]/);
      assertNoManagerCall(events);
      assert.equal(events.filter((event) => event.type === 'native-start').length, 0);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);

test(
  'stalled metadata lookup is aborted by the one-second deadline before the old CLI runs',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(ctx, { stallMetadata: true });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assertLatestRequest(requests);
      const aborted = events.find((event) => event.type === 'metadata-aborted');
      assert.ok(aborted, 'the stalled metadata request must be aborted, not merely rejected');
      assert.equal(aborted.signalAborted, true);
      assert.ok(
        aborted.elapsedMs >= 750 && aborted.elapsedMs < 3500,
        `expected an approximately one-second deadline, observed ${aborted.elapsedMs}ms`,
      );
      assertNoManagerCall(events);
      assert.equal(streamOutput(events, 'stderr'), '');
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);

test(
  'fake npm_execpath and PATH npm-cli.js hints are ignored for N and Y on an owned global install',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const alternatePrefix = path.join(ctx.workspace, 'fake-manager-alternate-prefix');
      await mkdir(alternatePrefix);
      const alternateEnvironment = npmEnvironment(ctx.workspace, alternatePrefix);
      runNpm([
        'install',
        '--global',
        '--prefix',
        alternatePrefix,
        '--no-audit',
        '--no-fund',
        ctx.fixtures.oldArchive,
      ], { cwd: ctx.workspace, env: alternateEnvironment });
      const alternatePackageRoot = path.join(alternatePrefix, 'lib', 'node_modules', 'noesora');
      const alternateBinaryPath = path.join(
        alternatePackageRoot,
        'vendor',
        `${process.platform}-${process.arch}`,
        process.platform === 'win32' ? 'noesora.exe' : 'noesora',
      );
      const alternateBefore = await readPackageState(alternatePackageRoot, alternateBinaryPath);

      const fakeNpmRoot = path.join(ctx.workspace, 'fake-npm', 'node_modules', 'npm');
      const fakeNpmBin = path.join(fakeNpmRoot, 'bin');
      const fakeNpmCli = path.join(fakeNpmBin, 'npm-cli.js');
      const markerPath = path.join(ctx.workspace, 'fake-npm-executed.jsonl');
      await mkdir(fakeNpmBin, { recursive: true });
      await writeFile(path.join(fakeNpmRoot, 'package.json'), JSON.stringify({
        name: 'npm',
        version: '0.0.0-fixture',
      }));
      await writeFile(fakeNpmCli, [
        "'use strict';",
        "const fs = require('node:fs');",
        'const args = process.argv.slice(2);',
        "fs.appendFileSync(process.env.NOESORA_TEST_FAKE_MANAGER_MARKER, JSON.stringify(args) + '\\n');",
        "if (args[0] === 'root') process.stdout.write(process.env.NOESORA_TEST_REAL_GLOBAL_ROOT + '\\n');",
        "else if (args[0] === 'prefix') process.stdout.write(process.env.NOESORA_TEST_REAL_PREFIX + '\\n');",
        '',
      ].join('\n'));
      await symlink(fakeNpmCli, path.join(fakeNpmBin, 'npm'));
      const fakeManagerEnv = {
        ...ctx.baseEnv,
        npm_execpath: fakeNpmCli,
        PATH: [fakeNpmBin, ctx.baseEnv.PATH].join(path.delimiter),
        NOESORA_TEST_FAKE_MANAGER_MARKER: markerPath,
        NOESORA_TEST_REAL_GLOBAL_ROOT: path.join(ctx.prefix, 'lib', 'node_modules'),
        NOESORA_TEST_REAL_PREFIX: ctx.prefix,
      };
      const fakeManagerContext = { ...ctx, baseEnv: fakeManagerEnv };
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const declined = await runFixture(fakeManagerContext, { input: 'n\n' });
      assert.equal(declined.result.status, 0, declined.result.stderr || declined.result.stdout);
      assertLatestRequest(declined.requests);
      assert.match(streamOutput(declined.events, 'stderr'), /Update now\? \[y\/N\]/);
      assert.equal(existsSync(markerPath), false, 'the hinted npm-cli.js must not execute');
      assertNoManagerCall(declined.events);
      assertOneNativeRun(declined.events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);

      const accepted = await runFixture(fakeManagerContext, { input: 'y\n' });
      assert.equal(accepted.result.status, 0, accepted.result.stderr || accepted.result.stdout);
      assertLatestRequest(accepted.requests);
      assertManagerCall(accepted.events, ctx.prefix);
      assert.equal(existsSync(markerPath), false, 'neither the query hint nor its PATH alias may execute');
      const updated = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      assert.equal(updated.version, UPDATE_VERSION_NEW);
      assert.notEqual(updated.binaryHash, before.binaryHash);
      assertOneNativeRun(accepted.events, UPDATE_VERSION_NEW);
      assert.deepEqual(await readPackageState(alternatePackageRoot, alternateBinaryPath), alternateBefore);
    });
  },
);

test(
  'oversized actual metadata response is cancelled and ignored before prompting',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(ctx, {
        input: 'n\n',
        oversizedMetadata: true,
      });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assertLatestRequest(requests);
      const response = events.find((event) => event.type === 'metadata-body-response-ready');
      assert.ok(response && response.byteLength > 65_536, 'the fixture must provide an oversized real response stream');
      const cancellation = events.find((event) =>
        event.type === 'metadata-body-cancelled' && event.kind === 'oversized');
      assert.ok(cancellation, 'the oversized response body must be cancelled');
      assert.ok(
        cancellation.bytesEnqueued < cancellation.totalBytes,
        'the byte cap must stop and cancel the stream before the full body is enqueued',
      );
      assertNoManagerCall(events);
      assert.equal(streamOutput(events, 'stderr'), '');
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);

test(
  'metadata body that stalls after headers is cancelled by the one-second deadline',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(ctx, { stallBody: true });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assertLatestRequest(requests);
      const response = events.find((event) =>
        event.type === 'metadata-body-response-ready' && event.kind === 'stalled');
      const aborted = events.find((event) => event.type === 'metadata-body-signal-aborted');
      const cancelled = events.find((event) =>
        event.type === 'metadata-body-cancelled' && event.kind === 'stalled');
      assert.ok(response, 'the fixture must deliver response headers before stalling its body');
      assert.ok(aborted, 'the deadline must abort after response headers arrive');
      assert.ok(cancelled, 'the stalled response body must be cancelled');
      assert.ok(
        aborted.elapsedMs >= 750 && aborted.elapsedMs < 3500,
        `expected an approximately one-second body deadline, observed ${aborted.elapsedMs}ms`,
      );
      assertNoManagerCall(events);
      assert.equal(streamOutput(events, 'stderr'), '');
      assert.match(result.stdout, /fixture-cli:1\.0\.0/);
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);

test(
  'failed manager with a corrupted but executable old binary refuses native launch',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const corruptScript = path.join(ctx.workspace, 'unverified-native.js');
      await writeFile(corruptScript, [
        '#!/usr/bin/env node',
        "'use strict';",
        "const fs = require('node:fs');",
        "fs.appendFileSync(process.env.NOESORA_TEST_EVENT_LOG, JSON.stringify({ type: 'unverified-native-ran' }) + '\\n');",
        "process.stdout.write('unverified native CLI executed\\n');",
        '',
      ].join('\n'));
      const { result, events, requests } = await runFixture(ctx, {
        input: 'y\n',
        corruptNativeWith: corruptScript,
      });
      assertLatestRequest(requests);
      assertManagerCall(events, ctx.prefix);
      assert.equal(result.status, 1, result.stdout || result.stderr);
      assert.match(streamOutput(events, 'stderr'), /verification failed; refusing to launch/i);
      const after = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      assert.equal(after.version, before.version, 'the injected manager failure preserves the old manifest');
      assert.notEqual(after.binaryHash, before.binaryHash, 'the injected manager fault really corrupts the old executable');
      assert.notEqual((await stat(ctx.binaryPath)).mode & 0o111, 0, 'the corrupted fixture remains executable');
      assert.equal(events.filter((event) => event.type === 'unverified-native-ran').length, 0);
      assert.equal(events.filter((event) => event.type === 'native-start').length, 0);
    });
  },
);

test(
  'eligible TTY tui project, cwd, and store canaries are absent from metadata request content',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const projectCanary = 'synthetic-project-canary-75a2';
      const cwdCanary = 'synthetic-cwd-canary-75a2';
      const storeCanary = 'synthetic-store-canary-75a2';
      const syntheticCwd = path.join(ctx.workspace, `cwd-${cwdCanary}`);
      const syntheticHome = path.join(ctx.workspace, `home-${storeCanary}`);
      await mkdir(syntheticCwd);
      await mkdir(path.join(syntheticHome, 'store'), { recursive: true });
      const privacyContext = {
        ...ctx,
        baseEnv: { ...ctx.baseEnv, NOESORA_HOME: syntheticHome },
      };
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(privacyContext, {
        args: ['tui', '--project', projectCanary],
        input: 'n\n',
        cwd: syntheticCwd,
        contextCanaries: [projectCanary, cwdCanary, storeCanary],
      });
      assert.equal(result.status, 0, 'the synthetic TUI launch should complete');
      assert.equal(requests.length, 1);
      assert.deepEqual(requests[0], {
        contextCanaryPresent: false,
        bodyPresent: false,
      });
      assert.match(streamOutput(events, 'stderr'), /Update now\? \[y\/N\]/);
      assertNoManagerCall(events);
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);

test(
  'closest copied Node runtime without npm never falls back to an outer Node backlink manager',
  { skip: ptySkipReason },
  async () => {
    await withUpdatePrefix(async (ctx) => {
      const outerRuntime = path.join(ctx.workspace, 'outer-runtime');
      const closestRuntime = path.join(outerRuntime, 'nested-runtime');
      const copiedNode = path.join(closestRuntime, 'bin', 'node');
      const outerBin = path.join(outerRuntime, 'bin');
      await mkdir(path.dirname(copiedNode), { recursive: true });
      await mkdir(outerBin, { recursive: true });
      await copyFile(process.execPath, copiedNode);
      await chmod(copiedNode, 0o755);
      await symlink(copiedNode, path.join(outerBin, 'node'));
      assert.equal(realpathSync(path.join(outerBin, 'node')), realpathSync(copiedNode));

      const closestNpmCandidates = [
        path.join(closestRuntime, 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
        path.join(closestRuntime, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
      ];
      assert.equal(closestNpmCandidates.some((candidate) => existsSync(candidate)), false);
      const outerNpmRoot = path.join(outerRuntime, 'lib', 'node_modules', 'npm');
      const outerNpmBin = path.join(outerNpmRoot, 'bin');
      const outerNpmCli = path.join(outerNpmBin, 'npm-cli.js');
      const markerPath = path.join(ctx.workspace, 'outer-runtime-npm-executed.jsonl');
      await mkdir(outerNpmBin, { recursive: true });
      await writeFile(path.join(outerNpmRoot, 'package.json'), JSON.stringify({
        name: 'npm',
        version: '0.0.0-fixture',
      }));
      await writeFile(outerNpmCli, [
        "'use strict';",
        "const fs = require('node:fs');",
        "fs.appendFileSync(process.env.NOESORA_TEST_FAKE_MANAGER_MARKER, 'executed\\n');",
        '',
      ].join('\n'));

      const launcherScript = path.join(ctx.packageRoot, 'bin', 'noesora.js');
      const launcherSource = await readFile(launcherScript, 'utf8');
      const copiedNodeLauncher = launcherSource.replace(/^#![^\r\n]*/, `#!${copiedNode}`);
      assert.notEqual(copiedNodeLauncher, launcherSource);
      await writeFile(launcherScript, copiedNodeLauncher);
      await chmod(launcherScript, 0o755);

      const fakeOuterRuntime = {
        ...ctx.baseEnv,
        PATH: [outerBin, ctx.baseEnv.PATH].join(path.delimiter),
        NOESORA_TEST_FAKE_MANAGER_MARKER: markerPath,
        NOESORA_TEST_EXPECTED_NODE: copiedNode,
      };
      const before = await readPackageState(ctx.packageRoot, ctx.binaryPath);
      const { result, events, requests } = await runFixture(
        { ...ctx, baseEnv: fakeOuterRuntime },
        { input: 'n\n' },
      );
      assert.equal(result.status, 0, result.stderr || result.stdout);
      assert.deepEqual(events.find((event) => event.type === 'runtime-node-match'), {
        type: 'runtime-node-match',
        matches: true,
      });
      assert.equal(existsSync(markerPath), false, 'the outer ancestor npm must not be executed');
      assert.deepEqual(requests, []);
      assert.equal(streamOutput(events, 'stderr'), '');
      assertNoManagerCall(events);
      assertOneNativeRun(events, UPDATE_VERSION_OLD);
      assert.deepEqual(await readPackageState(ctx.packageRoot, ctx.binaryPath), before);
    });
  },
);


after(async () => {
  if (!updateFixturesPromise) return;
  const fixtures = await updateFixturesPromise.catch(() => undefined);
  if (fixtures) await rm(fixtures.root, { recursive: true, force: true });
});

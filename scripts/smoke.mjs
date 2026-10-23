import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, appendFileSync, rmSync, copyFileSync, lstatSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';
import { spawnSync } from 'node:child_process';

const MODEL_ID = 'intfloat/multilingual-e5-small';
const MODEL_REVISION = '614241f622f53c4eeff9890bdc4f31cfecc418b3';
const MODEL_ASSETS = ['config.json', 'model.safetensors', 'tokenizer.json'];
const usage = 'Usage: node scripts/smoke.mjs [<binary>] [--model | --model-dir <installed-directory>] [--keep]';
const args = process.argv.slice(2);
let modelDirArg;
for (let i = 0; i < args.length; i++) {
  if (args[i] !== '--model-dir') continue;
  if (modelDirArg !== undefined) throw new Error('Only one --model-dir may be provided. ' + usage);
  const value = args[i + 1];
  if (!value || value.startsWith('--')) throw new Error('--model-dir requires an installed model directory. ' + usage);
  modelDirArg = value;
  args.splice(i, 2);
  i--;
}
const withModel = args.includes('--model');
const keep = args.includes('--keep');
const unknownFlags = args.filter(arg => arg.startsWith('--') && arg !== '--model' && arg !== '--keep');
if (unknownFlags.length) throw new Error('Unknown smoke option: ' + unknownFlags[0] + '. ' + usage);
const positional = args.filter(arg => !arg.startsWith('--'));
if (positional.length > 1) throw new Error(usage);
if (withModel && modelDirArg !== undefined) throw new Error('Choose --model or --model-dir, not both. ' + usage);
const binary = resolve(positional[0] || 'target/debug/noesora');

function invalidModelCache(source, reason) {
  throw new Error('Invalid --model-dir "' + source + '": ' + reason);
}
function requireCachePath(source, path, kind) {
  let metadata;
  try { metadata = lstatSync(path); } catch (error) {
    invalidModelCache(source, 'cannot read ' + path + ': ' + error.message);
  }
  const valid = kind === 'directory' ? metadata.isDirectory() : metadata.isFile();
  if (!valid) invalidModelCache(source, path + ' must be a regular ' + kind + ' (symlinks are not accepted)');
  return path;
}
function validateModelCache(sourceArg) {
  const source = resolve(sourceArg);
  requireCachePath(source, source, 'directory');
  const manifestPath = requireCachePath(source, join(source, 'manifest.json'), 'file');
  let manifest;
  try { manifest = JSON.parse(readFileSync(manifestPath, 'utf8')); } catch (error) {
    invalidModelCache(source, 'cannot parse manifest.json: ' + error.message);
  }
  if (!manifest || manifest.version !== 1 || manifest.model_id !== MODEL_ID ||
      manifest.revision !== MODEL_REVISION || manifest.dimension !== 384) {
    invalidModelCache(source, 'manifest.json does not identify the pinned ' + MODEL_ID + '@' + MODEL_REVISION + ' model');
  }
  const fingerprint = typeof manifest.fingerprint === 'string' && manifest.fingerprint.match(/^sha256:([0-9a-f]{64})$/);
  if (!fingerprint || typeof manifest.asset_dir !== 'string' ||
      !manifest.asset_dir.startsWith('assets-' + fingerprint[1] + '-') ||
      !/^[A-Za-z0-9-]+$/.test(manifest.asset_dir)) {
    invalidModelCache(source, 'manifest.json has an invalid fingerprint or asset directory');
  }
  if (!Array.isArray(manifest.assets) || manifest.assets.length !== MODEL_ASSETS.length ||
      manifest.assets.some((asset, index) => !asset || asset.name !== MODEL_ASSETS[index] ||
        typeof asset.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(asset.sha256))) {
    invalidModelCache(source, 'manifest.json does not contain the pinned config, weights, and tokenizer assets');
  }
  const assetDir = requireCachePath(source, join(source, manifest.asset_dir), 'directory');
  for (const name of MODEL_ASSETS) requireCachePath(source, join(assetDir, name), 'file');
  return { source, manifest, assetDir };
}
function copyModelCache(cache, destination) {
  try {
    mkdirSync(destination, { recursive: true });
    copyFileSync(join(cache.source, 'manifest.json'), join(destination, 'manifest.json'));
    const copiedAssets = join(destination, cache.manifest.asset_dir);
    mkdirSync(copiedAssets);
    for (const name of MODEL_ASSETS) copyFileSync(join(cache.assetDir, name), join(copiedAssets, name));
  } catch (error) {
    throw new Error('Could not copy --model-dir "' + cache.source + '" into the isolated smoke home: ' + error.message);
  }
}
const cachedModel = modelDirArg === undefined ? undefined : validateModelCache(modelDirArg);
const root = realpathSync(mkdtempSync(join(tmpdir(), 'noesora-r1-')));
const home = join(root, 'home');
const cwdA = join(root, 'project-a');
const cwdB = join(root, 'project-b');
mkdirSync(cwdA); mkdirSync(cwdB);
const env = { ...process.env, NOESORA_HOME: home, HOME: home, USERPROFILE: home, XDG_CONFIG_HOME: join(home, '.config'), XDG_DATA_HOME: join(home, '.local', 'share'), XDG_STATE_HOME: join(home, '.local', 'state'), NO_COLOR: '1' };
const stamp = '2026-10-21T00:00:00Z';
let storeRoot;
function run(args, { cwd = cwdA, extraEnv = {}, ok = true } = {}) {
  const denyNetwork = process.platform === 'darwin' && extraEnv.HF_HUB_OFFLINE === '1';
  const executable = denyNetwork ? '/usr/bin/sandbox-exec' : binary;
  const argv = denyNetwork ? ['-p', '(version 1) (allow default) (deny network*)', binary, '--json', ...args] : ['--json', ...args];
  const result = spawnSync(executable, argv, { cwd, env: { ...env, ...extraEnv }, encoding: 'utf8', timeout: 600_000, maxBuffer: 8 * 1024 * 1024 });
  assert.ifError(result.error);
  let value;
  try { value = JSON.parse(result.stdout); } catch { throw new Error('Invalid JSON stdout for ' + args.join(' ') + ': ' + result.stdout + '\n' + result.stderr); }
  if (ok) { assert.equal(result.status, 0, JSON.stringify(value) + '\n' + result.stderr); assert.equal(value.ok, true); }
  else { assert.notEqual(result.status, 0); assert.equal(value.ok, false); assert.equal(typeof value.error, 'string'); }
  return value;
}
function fixture(name, events) { const path = join(root, name + '.jsonl'); writeFileSync(path, events.map(x => JSON.stringify(x)).join('\n') + '\n'); return path; }
function withinStore(path) {
  const resolved = resolve(storeRoot, path);
  const relativePath = relative(storeRoot, resolved);
  assert(relativePath && relativePath !== '..' && !relativePath.startsWith('..' + sep) && !isAbsolute(relativePath), 'Path escapes global store root: ' + path);
  return resolved;
}
function verify(hit) {
  assert.equal(typeof hit.path, 'string');
  assert(!isAbsolute(hit.path), 'Search hit path must be store-relative');
  const bytes = readFileSync(withinStore(hit.path));
  assert.match(hit.hash, /^sha256:[0-9a-f]{64}$/);
  assert.equal('sha256:' + createHash('sha256').update(bytes).digest('hex'), hit.hash);
  const slice = bytes.subarray(hit.span[0], hit.span[1]);
  if (hit.field) {
    let field = JSON.parse(slice.toString('utf8'));
    for (const key of hit.field.slice(1).split('/').map(x => x.replaceAll('~1', '/').replaceAll('~0', '~'))) field = field[key];
    assert.equal(typeof field, 'string');
    assert.deepEqual(Buffer.from(field).subarray(hit.text_span[0], hit.text_span[1]).toString('utf8'), hit.text);
  } else assert.equal(slice.toString('utf8'), hit.text);
}
try {
  const initialized = run(['init']);
  assert.equal(typeof initialized.root, 'string');
  assert(isAbsolute(initialized.root), 'Init must return the absolute global store root');
  storeRoot = initialized.root;
  const omp = fixture('omp', [
    { type: 'session', version: 3, id: 'smoke-omp', timestamp: stamp, cwd: '/projects/harbor' },
    { type: 'message', id: 'u1', timestamp: stamp, message: { role: 'user', content: [{ type: 'text', text: 'Where do application access secrets belong?' }] } },
    { type: 'message', id: 'a1', timestamp: stamp, message: { role: 'assistant', content: [{ type: 'text', text: 'Save authentication tokens in the operating system credential store, not in repository files.\nThe key label is café-密钥.' }] } },
    { type: 'message', id: 't1', timestamp: stamp, message: { role: 'toolResult', toolCallId: 'call-1', toolName: 'bash', content: [{ type: 'text', text: 'vectorcache-server7 connection refused' }], isError: true } },
    { type: 'message', id: 'long-1', timestamp: stamp, message: { role: 'assistant', content: [{ type: 'text', text: 'The context discusses ordinary garden planting. '.repeat(220) + 'The mineral used for disaster recovery is amber quartz.' }] } },
  ]);
  const claude = fixture('claude', [
    { type: 'user', sessionId: 'smoke-claude', uuid: 'cu1', cwd: '/projects/orbit', timestamp: stamp, message: { role: 'user', content: 'How do we publish an archive snapshot?' } },
    { type: 'assistant', sessionId: 'smoke-claude', uuid: 'ca1', cwd: '/projects/orbit', timestamp: stamp, message: { role: 'assistant', content: [{ type: 'text', text: 'Publish the immutable snapshot atomically after flushing its bytes.' }] } },
  ]);
  const codex = fixture('codex', [
    { timestamp: stamp, type: 'session_meta', payload: { id: 'smoke-codex', timestamp: stamp, cwd: '/projects/lumen' } },
    { timestamp: stamp, type: 'response_item', payload: { type: 'message', id: 'cx1', role: 'assistant', content: [{ type: 'output_text', text: 'Measure the local working set before replacing the allocator.' }] } },
  ]);
  let imported = run(['session', 'import', '--client', 'omp', omp]);
  run(['session', 'import', '--client', 'claude-code', claude]);
  run(['session', 'import', '--client', 'codex', codex]);
  const before = run(['status']);
  assert.equal(before.sessions, 3);
  assert.equal(before.records, 0, 'Raw sessions became approved records');
  const repeat = run(['session', 'import', '--client', 'omp', omp]);
  assert.equal(repeat.duplicate, true);
  assert.equal(run(['status']).messages, before.messages);
  assert.equal(run(['session', 'list'], { cwd: cwdB }).sessions.length, 3);
  const evidence = run(['search', 'credential store'], { cwd: cwdB });
  assert.equal(evidence.hits[0].project, '/projects/harbor');
  assert.equal(evidence.hits[0].type, 'session');
  evidence.hits.forEach(verify);
  const unicode = run(['search', 'café']);
  assert(unicode.hits.some(x => x.text.includes('café-密钥')));
  unicode.hits.forEach(verify);
  assert.equal(run(['search', 'vectorcache-server7']).hits.length, 0);
  const tools = run(['search', 'vectorcache-server7', '--scope', 'tools']);
  assert(tools.hits.some(x => x.text.includes('connection refused')));
  tools.hits.forEach(verify);
  assert.equal(run(['search', 'credential', '--project', '/projects/orbit']).hits.length, 0);
  run(['query', 'DELETE FROM sessions'], { ok: false });
  assert.equal(run(['status']).sessions, 3);
  run(['search', 'where should login credentials be kept safely', '--mode', 'semantic'], { ok: false });
  run(['note', '--title', 'Explicit decision', '--project', '/projects/harbor', 'Noesora uses one shared local database.']);
  assert.equal(run(['status']).records, 1);
  const broken = join(root, 'broken.jsonl'); writeFileSync(broken, '{not json}\n');
  run(['session', 'import', '--client', 'omp', broken], { ok: false });
  assert.equal(run(['status']).sessions, 3);
  const rebuilt = run(['rebuild']);
  assert.equal(run(['status']).sessions, 3);
  assert.equal(run(['status']).records, 1);
  run(['search', 'credential store']).hits.forEach(verify);
  if (withModel || cachedModel) {
    const modelPath = join(home, 'models', 'multilingual-e5-small');
    if (cachedModel) copyModelCache(cachedModel, modelPath);
    else run(['model', 'install']);
    const offline = { HF_HUB_OFFLINE: '1', HTTP_PROXY: 'http://127.0.0.1:9', HTTPS_PROXY: 'http://127.0.0.1:9', ALL_PROXY: 'http://127.0.0.1:9' };
    const modelStatus = run(['model', 'status'], { extraEnv: offline });
    assert.equal(modelStatus.installed, true, cachedModel
      ? '--model-dir cache was not accepted as the pinned installed model'
      : 'model install did not leave the pinned model installed');
    assert.equal(modelStatus.model_id, MODEL_ID);
    assert.equal(modelStatus.revision, MODEL_REVISION);
    assert.equal(modelStatus.dimension, 384);
    assert.match(modelStatus.fingerprint, /^sha256:[0-9a-f]{64}$/);
    assert.equal(typeof modelStatus.path, 'string');
    assert.equal(resolve(cwdA, modelStatus.path), resolve(modelPath), 'Model status must point to the isolated smoke cache');
    if (cachedModel) assert.equal(modelStatus.fingerprint, cachedModel.manifest.fingerprint, 'Copied model fingerprint changed');
    run(['index'], { extraEnv: offline });
    const semantic = run(['search', 'where should login credentials be kept safely', '--mode', 'semantic', '--limit', '3'], { cwd: cwdB, extraEnv: offline });
    assert.equal(semantic.mode, 'semantic');
    assert.equal(semantic.index_complete, true);
    assert(semantic.hits.some(x => x.text.includes('operating system credential store')));
    semantic.hits.forEach(verify);
    const late = run(['search', 'Which mineral unlocks disaster recovery?', '--mode', 'semantic', '--limit', '3'], { extraEnv: offline });
    const lateHit = late.hits.find(x => x.text.includes('amber quartz'));
    assert(lateHit, 'Semantic search lost the end of a long message');
    assert(lateHit.field && lateHit.text_span[0] > 0, 'Late evidence must cite the later source window, not an unrelated first-window vector');
    late.hits.forEach(verify);
    const readyBeforeUpdate = run(['status']);
    const priorSnapshot = withinStore(imported.snapshot);
    const priorBytes = readFileSync(priorSnapshot);
    appendFileSync(omp, JSON.stringify({ type: 'message', id: 'continued-a2', timestamp: stamp, message: { role: 'assistant', content: [{ type: 'text', text: 'Cache unchanged passages by source text and pinned model identity when a session grows.' }] } }) + '\n');
    imported = run(['session', 'import', '--client', 'omp', omp]);
    assert.equal(imported.duplicate, false);
    assert(readFileSync(priorSnapshot).equals(priorBytes), 'Session growth changed the previous immutable snapshot');
    const afterUpdate = run(['status']);
    assert.equal(afterUpdate.embedded_chunks, readyBeforeUpdate.embedded_chunks, 'Session growth discarded embeddings for unchanged passages');
    const partial = run(['search', 'where should login credentials be kept safely', '--mode', 'semantic'], { extraEnv: offline });
    assert.equal(partial.index_complete, false, 'New unembedded history was reported complete');
    assert(partial.hits.some(x => x.text.includes('operating system credential store')));
    partial.hits.forEach(verify);
    run(['index'], { extraEnv: offline });
    assert.equal(run(['search', 'credential store', '--mode', 'semantic'], { extraEnv: offline }).index_complete, true);
    const boundary = process.platform === 'darwin' ? 'OS-enforced network denial' : 'HF offline mode and blocked proxy settings';
    console.log('Local-model paraphrase and late-message retrieval passed with ' + boundary + '.');
  }
  const snapshot = withinStore(imported.snapshot);
  appendFileSync(snapshot, '\nchanged snapshot');
  run(['search', 'credential store'], { ok: false });
  // Restore only this synthetic fixture after the deliberate integrity fault.
  writeFileSync(snapshot, readFileSync(omp));
  run(['search', 'credential store']).hits.forEach(verify);
  console.log('Global archive, all three adapters, cross-project citations, deduplication, Tools scope, readonly SQL, malformed-source and source-integrity checks passed.');
} finally {
  if (keep) console.log('Isolated smoke store retained at ' + home);
  else rmSync(root, { recursive: true, force: true });
}

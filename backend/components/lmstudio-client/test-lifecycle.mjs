// Execute the real loader against an in-memory SDK. Never contact LM Studio or a GPU.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { inspect } from 'node:util';
import { createContext, SourceTextModule, SyntheticModule } from 'node:vm';

const source = fs.readFileSync(new URL('./load-model.mjs', import.meta.url), 'utf8');
const config = JSON.parse(fs.readFileSync(new URL('../../config/local-model.json', import.meta.url), 'utf8'));
const modelKey = `${config.repository}/${config.filename}`;

async function scenario({ unload = false, existing = false, otherModel = false, paused = false, stopDuringLoad = false, stopAfterLoad = false } = {}) {
  const events = [], errors = [];
  let loaded = existing, options, timer;
  let finish;
  const done = new Promise(resolve => { finish = resolve; });
  const handle = {
    identifier: config.identifier,
    getModelInfo: async () => ({ path: otherModel ? 'other/model.gguf' : modelKey, ttlMs: null }),
    getLoadConfig: async () => options?.config ?? {},
    unload: async () => { events.push('unload'); loaded = false; },
  };
  const context = createContext({
    AbortController,
    console: { log() {}, error(message) { errors.push(message); } },
    process: { platform: 'win32', argv: ['node', 'loader', unload ? '--unload' : '16384'], env: {}, exit(code) { finish(code); } },
    setInterval(fn) { timer = fn; return 1; }, clearInterval() {},
  });
  const modules = {
    '@lmstudio/sdk': { LMStudioClient: class {
      llm = {
        listLoaded: async () => loaded ? [handle] : [],
        estimateResourcesUsage: async () => ({ passesGuardrails: true, memory: { totalVramBytes: 0 } }),
        load: async (_key, opts) => {
          events.push('load'); options = opts;
          if (stopDuringLoad) { paused = true; timer(); }
          if (opts.signal.aborted) throw new Error('synthetic load cancelled');
          loaded = true;
          if (stopAfterLoad) paused = true;
          return handle;
        },
      };
    } },
    'node:fs': { default: {
      readFileSync(filename) { return JSON.stringify(filename.endsWith('local-model.json') ? config : { enabled: !paused }); },
      mkdirSync() {}, writeFileSync() {}, appendFileSync() {},
    } },
    'node:path': { default: path },
    'node:url': { fileURLToPath },
    'node:child_process': {
      spawnSync() { events.push('gpu-query'); return { status: 1 }; },
      execFileSync() { throw new Error('No real GPU commands allowed'); },
    },
    'node:os': { default: { freemem: () => 128 * 1024 ** 3 } },
    'node:util': { inspect },
  };
  const module = new SourceTextModule(source, { context, initializeImportMeta(meta) { meta.url = new URL('./load-model.mjs', import.meta.url).href; } });
  await module.link(specifier => {
    const exports = modules[specifier];
    assert.ok(exports, `Unexpected import: ${specifier}`);
    return new SyntheticModule(Object.keys(exports), function () {
      for (const [key, value] of Object.entries(exports)) this.setExport(key, value);
    }, { context });
  });
  await module.evaluate();
  const code = await done;
  return { events, errors, code, options, loaded };
}

let result = await scenario({ unload: true, existing: true });
assert.equal(result.code, 0);
assert.deepEqual(result.events, ['unload']);
assert.equal(result.loaded, false);
result = await scenario({ unload: true });
assert.equal(result.code, 0);
assert.deepEqual(result.events, []);
result = await scenario({ unload: true, existing: true, otherModel: true });
assert.equal(result.code, 1);
assert.deepEqual(result.events, []);
assert.equal(result.loaded, true);
result = await scenario();
assert.equal(result.code, 0);
assert.equal(result.options.ttl, 60);
result = await scenario({ existing: true });
assert.equal(result.code, 0);
assert.deepEqual(result.events, ['gpu-query', 'unload', 'load']);
assert.equal(result.options.ttl, 60);
result = await scenario({ paused: true });
assert.equal(result.code, 1);
assert.ok(!result.events.includes('load'));
result = await scenario({ stopDuringLoad: true });
assert.equal(result.code, 1);
assert.equal(result.options.signal.aborted, true);
assert.equal(result.loaded, false);
result = await scenario({ stopAfterLoad: true });
assert.equal(result.code, 1);
assert.equal(result.loaded, false);
assert.ok(result.events.includes('unload'));
console.log('8 loader lifecycle scenarios passed (SDK, files and GPU fully mocked).');

import { LMStudioClient } from '@lmstudio/sdk';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawnSync } from 'node:child_process';
import os from 'node:os';
import { inspect } from 'node:util';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'config/local-model.json'), 'utf8'));
const unloadOnly = process.argv.includes('--unload');
const contextLength = Number((unloadOnly ? null : process.argv[2]) || config.contextLength);
const idleTtlSeconds = 60;
const appHome = process.env.SUPA_DEV_HOME || path.resolve(root, '../..');
function editorEnabled() {
  try { return JSON.parse(fs.readFileSync(path.join(appHome, 'local-editor.json'), 'utf8')).enabled !== false; }
  catch (error) { if (error.code === 'ENOENT') return true; throw error; }
}
const automaticGpu = config.gpuOffload === 'auto';
const nvidia = process.platform === 'win32' ? 'nvidia-smi.exe' : 'nvidia-smi';
const gpuAvailable = !unloadOnly && spawnSync(nvidia, ['--query-gpu=memory.free', '--format=csv,noheader,nounits'], { windowsHide: true, timeout: 10000 }).status === 0;
let gpuRatio = automaticGpu ? (gpuAvailable ? 1 : 0) : config.gpuOffload === 'max' ? 1 : Number(config.gpuOffload);
if (!Number.isFinite(gpuRatio) || gpuRatio < 0 || gpuRatio > 1) throw new Error('gpuOffload must be auto, max or a ratio from 0 to 1.');
const estimateOnly = process.argv.includes('--estimate-only');
if (![16384, 32768].includes(contextLength)) throw new Error('Allowed context lengths: 16384, 32768.');
const output = path.join(root, 'output');
fs.mkdirSync(output, { recursive: true });
const client = new LMStudioClient({ baseUrl: 'ws://127.0.0.1:1234', verboseErrorMessages: true });
const modelKey = `${config.repository}/${config.filename}`;
const desired = {
  autoFit: false,
  gpu: { ratio: gpuRatio, mainGpu: 0, splitStrategy: 'favorMainGpu' },
  // LM Studio's internal heuristic silently reduced max to 60 layers.
  // Our full-load estimate, reserve and monitor below enforce the memory limit.
  gpuStrictVramCap: false,
  speculativeDraftMtp: false,
  speculativeDraftSimple: false,
  contextLength,
  flashAttention: true,
  llamaKCacheQuantizationType: config.kvCache,
  llamaVCacheQuantizationType: config.kvCache,
  offloadKVCacheToGpu: gpuAvailable,
  maxParallelPredictions: 1,
  evalBatchSize: config.evalBatchSize,
  tryMmap: true,
};
function gpu() {
  if (!gpuAvailable) return { time: new Date().toISOString(), freeMiB: Number.MAX_SAFE_INTEGER, usedMiB: 0, totalMiB: 0, cpu: true };
  const raw = execFileSync(nvidia, ['--id=0', '--query-gpu=memory.total,memory.used,memory.free', '--format=csv,noheader,nounits'], { encoding: 'utf8', windowsHide: true }).trim();
  const [totalMiB, usedMiB, freeMiB] = raw.split(',').map(Number);
  if (![totalMiB, usedMiB, freeMiB].every(Number.isFinite)) throw new Error('Could not read NVIDIA VRAM. Load cancelled.');
  const sample = { time: new Date().toISOString(), totalMiB, usedMiB, freeMiB };
  fs.appendFileSync(path.join(output, 'vram.jsonl'), JSON.stringify(sample) + '\n');
  return sample;
}
function compatible(actual) {
  return actual.contextLength === contextLength && actual.flashAttention === true &&
    actual.gpuStrictVramCap === false && actual.speculativeDraftMtp === false &&
    actual.speculativeDraftSimple === false && actual.evalBatchSize === config.evalBatchSize &&
    actual.llamaKCacheQuantizationType === config.kvCache && actual.llamaVCacheQuantizationType === config.kvCache &&
    actual.maxParallelPredictions === 1 && (automaticGpu || actual.gpu?.ratio === gpuRatio || (gpuRatio === 1 && actual.gpu?.ratio === 'max'));
}
function writeReport(report) {
  fs.writeFileSync(path.join(output, 'model-load.json'), JSON.stringify(report, null, 2));
}
async function main() {
  const existing = (await client.llm.listLoaded()).find(m => m.identifier === config.identifier);
  if (unloadOnly) {
    if (existing) {
      const info = await existing.getModelInfo();
      if (info.path !== modelKey && info.modelKey !== modelKey) throw new Error('The editor identifier belongs to a different model; nothing unloaded.');
      await existing.unload();
    }
    if ((await client.llm.listLoaded()).some(m => m.identifier === config.identifier)) throw new Error('The editor is still loaded.');
    console.log('Local prompt editor is unloaded.');
    return;
  }
  if (!editorEnabled()) throw new Error('Local prompt editor was stopped in Supa Dev.');
  if (existing) {
    const info = await existing.getModelInfo();
    if (info.path !== modelKey && info.modelKey !== modelKey) throw new Error('The editor identifier belongs to a different model; nothing unloaded.');
    const actual = await existing.getLoadConfig();
    if ((info.path === modelKey || info.modelKey === modelKey) && compatible(actual) && info.ttlMs === idleTtlSeconds * 1000) {
      const memory = gpu();
      const existingRatio = actual.gpu?.ratio === 'max' ? 1 : Number(actual.gpu?.ratio);
      const canUpgrade = gpuAvailable && automaticGpu && existingRatio < 1 && memory.freeMiB >
        (config.sizeBytes * (1 - existingRatio) / 1048576) + config.vramReserveMiB + config.computeBudgetMiB;
      if (memory.freeMiB >= config.vramReserveMiB && !canUpgrade) {
        writeReport({ status: 'already-loaded', modelKey, actual, memory });
        console.log(`local-coder ready; context=${contextLength}; free VRAM=${memory.freeMiB} MiB.`);
        return;
      }
      if (!automaticGpu) throw new Error(`Only ${memory.freeMiB} MiB VRAM remains. Free GPU memory before inference.`);
    }
    if (estimateOnly) throw new Error('Unload local-coder before estimating a different configuration.');
    await existing.unload();
  }
  const memory = gpu();
  if (!gpuAvailable && os.freemem() < config.sizeBytes + contextLength * config.kvBytesPerToken + 2 * 1024 ** 3) {
    throw new Error('Insufficient free RAM for the local prompt editor. Free memory before retrying.');
  }
  let estimate, estimateMiB, requiredMiB;
  for (const candidate of automaticGpu && gpuAvailable ? [1, 0.9, 0.75, 0.5, 0.35, 0.25, 0] : [gpuRatio]) {
    gpuRatio = candidate;
    desired.gpu.ratio = candidate;
    estimate = await client.llm.estimateResourcesUsage(modelKey, desired, { deviceIdentifier: null });
    const lowerBoundMiB = Math.ceil((config.sizeBytes * gpuRatio + contextLength * config.kvBytesPerToken) / 1048576) + config.computeBudgetMiB;
    estimateMiB = Math.ceil(estimate.memory.totalVramBytes / 1048576);
    if (!Number.isFinite(estimateMiB)) throw new Error('LM Studio returned no valid VRAM estimate; refusing to guess.');
    requiredMiB = Math.max(estimateMiB, lowerBoundMiB) + config.vramReserveMiB;
    if (memory.freeMiB >= requiredMiB && estimate.passesGuardrails) break;
  }
  const report = { modelKey, contextLength, desired, estimate, requiredMiB, memory, status: 'estimated' };
  writeReport(report);
  console.log(`VRAM: free=${memory.freeMiB} MiB, required with reserve=${requiredMiB} MiB, LM Studio estimate=${estimateMiB} MiB.`);
  if (estimateOnly) return;
  if (memory.freeMiB < requiredMiB || !estimate.passesGuardrails) {
    throw new Error(`Safe load blocked. Free at least ${Math.max(0, requiredMiB-memory.freeMiB)} MiB more VRAM. No model loaded, no cloud fallback.`);
  }
  const abort = new AbortController();
  const timer = setInterval(() => {
    try {
      if (!editorEnabled()) abort.abort('Local prompt editor was stopped in Supa Dev.');
      if (gpu().freeMiB < config.vramReserveMiB) abort.abort('VRAM reserve exhausted during load.');
    } catch (error) { abort.abort(error); }
  }, 1000);
  let model;
  try {
    model = await client.llm.load(modelKey, { identifier: config.identifier, deviceIdentifier: null, config: desired, ttl: idleTtlSeconds, signal: abort.signal, verbose: false });
  } catch (error) {
    writeReport({ ...report, status: 'load-failed', error: inspect(error, { depth: 8 }), after: gpu() });
    throw error;
  } finally { clearInterval(timer); }
  const actual = await model.getLoadConfig();
  const after = gpu();
  if (!editorEnabled() || !compatible(actual) || after.freeMiB < config.vramReserveMiB) {
    await model.unload();
    writeReport({ ...report, status: 'rejected-after-load', actual, after });
    throw new Error('Actual load configuration or free VRAM did not pass verification. Model unloaded.');
  }
  writeReport({ ...report, status: 'loaded', actual, after });
  console.log(`local-coder loaded: context=${contextLength}, GPU=${gpuRatio}, FA=true, KV=q8_0, parallel=1; free VRAM=${after.freeMiB} MiB.`);
}
main().then(() => process.exit(0)).catch(error => {
  fs.writeFileSync(path.join(output, 'model-load-error.log'), inspect(error, { depth: 8 }));
  console.error(error.message);
  process.exit(1);
});

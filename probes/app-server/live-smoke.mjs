// B1 live smoke (approved 2026-10-05): real ChatGPT login and real inference via the
// shared default CODEX_HOME (decision A3). Read-only use of existing auth/config;
// one ephemeral general-chat turn. No product code, no external packages.
// Records shapes and booleans only: no email, tokens, headers or free-form model text.
import { spawn, execFileSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const exe = process.argv[2];
if (!exe) throw new Error('Supply an absolute path to the 0.160.0 codex.exe');
const version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim();
if (version !== 'codex-cli 0.160.0') throw new Error(`Wrong version: ${version}`);

const run = join(root, 'runs', `${new Date().toISOString().replace(/[:.]/g, '-')}-live`);
const work = join(run, 'work');
mkdirSync(work, { recursive: true });
const codexHome = process.env.CODEX_HOME || join(process.env.USERPROFILE, '.codex');
const watched = ['config.toml', 'auth.json'].map(f => join(codexHome, f));
const fingerprint = () => Object.fromEntries(watched.map(p => [p.split(/[\\/]/).pop(),
  existsSync(p) ? { sha256: createHash('sha256').update(readFileSync(p)).digest('hex'),
    mtimeMs: statSync(p).mtimeMs } : null]));
const before = fingerprint();

const env = { ...process.env };
// Product decision: ChatGPT login, never API-key billing. Drop key overrides from the child.
for (const key of Object.keys(env)) if (/^(OPENAI_API_KEY|CODEX_API_KEY|CODEX_ACCESS_TOKEN)$/i.test(key)) delete env[key];

const sleep = ms => new Promise(r => setTimeout(r, ms));
const proc = spawn(exe, ['app-server', '--stdio'], { env, cwd: work, windowsHide: true });
let stderr = ''; let next = 1;
const pending = new Map(); const events = []; const serverRequests = [];
proc.stderr.on('data', b => { stderr = (stderr + b.toString()).slice(-4000); });
const send = m => proc.stdin.write(JSON.stringify(m) + '\n');
createInterface({ input: proc.stdout }).on('line', line => {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.id !== undefined && pending.has(m.id)) {
    const p = pending.get(m.id); pending.delete(m.id); clearTimeout(p.timer); p.resolve(m);
  } else if (m.id !== undefined) {
    // Nothing is authorized in this smoke test.
    serverRequests.push(m.method);
    send({ id: m.id, error: { code: -32000, message: 'Live smoke declines server request' } });
  } else events.push({ at: Date.now(), ...m });
});
const rpc = (method, params = {}, timeout = 30000) => new Promise((resolve, reject) => {
  const id = next++;
  const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timeout ${method}`)); }, timeout);
  pending.set(id, { resolve, timer }); send({ id, method, params });
});
const ok = async (method, params, timeout) => {
  const r = await rpc(method, params, timeout);
  if (r.error) throw new Error(`${method}: ${JSON.stringify(r.error).slice(0, 500)}`);
  return r.result;
};

const results = []; const t0 = Date.now();
async function step(name, fn) {
  try { results.push({ name, status: 'pass', evidence: await fn() }); }
  catch (e) { results.push({ name, status: 'fail', error: e.message }); }
  console.log(`${results.at(-1).status}: ${name}`);
}
const MARK = 'AGENTDOCK_LIVE_OK';
let threadId; let models = [];
try {
  await step('initialize (stable API only)', async () => {
    const r = await ok('initialize', { clientInfo: { name: 'agentdock_live_smoke', title: 'AgentDock Live Smoke', version: '0.1' },
      capabilities: { experimentalApi: false } });
    send({ method: 'initialized', params: {} });
    return { keys: Object.keys(r ?? {}) };
  });
  await step('account/read', async () => {
    const r = await ok('account/read', { refreshToken: false });
    return { type: r.account?.type ?? null, planType: r.account?.planType ?? null,
      emailPresent: !!r.account?.email, requiresOpenaiAuth: r.requiresOpenaiAuth };
  });
  await step('account/rateLimits/read', async () => {
    const r = await ok('account/rateLimits/read', {});
    return { keys: Object.keys(r ?? {}) };
  });
  await step('model/list', async () => {
    const r = await ok('model/list', {});
    models = r.data ?? [];
    return { count: models.length, nextCursor: !!r.nextCursor,
      models: models.map(m => ({ id: m.id, hidden: m.hidden, isDefault: m.isDefault ?? null,
        defaultEffort: m.defaultReasoningEffort,
        efforts: (m.supportedReasoningEfforts ?? []).map(e => e.reasoningEffort ?? e),
        inputModalities: m.inputModalities })) };
  });
  await step('thread/start (ephemeral, read-only, general chat cwd)', async () => {
    const r = await ok('thread/start', { cwd: work, ephemeral: true, sandbox: 'read-only', approvalPolicy: 'on-request' });
    threadId = r.thread?.id;
    return { threadIdPresent: !!threadId, model: r.model ?? null, reasoningEffort: r.reasoningEffort ?? null,
      sandbox: r.sandbox?.type ?? r.sandbox ?? null, approvalPolicy: r.approvalPolicy ?? null, ephemeral: r.thread?.ephemeral ?? null };
  });
  await step('turn/start -> streamed reply -> turn/completed', async () => {
    if (!threadId) throw new Error('no thread');
    const from = events.length; const started = Date.now();
    const r = await ok('turn/start', { threadId, effort: 'low',
      input: [{ type: 'text', text: `Reply with exactly this token and nothing else: ${MARK}` }] });
    const turnId = r.turn?.id;
    const end = Date.now() + 120000; let done;
    while (Date.now() < end) {
      done = events.slice(from).find(e => e.method === 'turn/completed' && e.params?.turn?.id === turnId);
      if (done) break; await sleep(100);
    }
    if (!done) throw new Error(`turn not completed in 120s; methods=${[...new Set(events.slice(from).map(e => e.method))].join(',')}`);
    const mine = events.slice(from);
    const deltas = mine.filter(e => e.method === 'item/agentMessage/delta');
    const firstDelta = deltas[0]?.at ?? null;
    const finalMsg = mine.filter(e => e.method === 'item/completed' && e.params?.item?.type === 'agentMessage').at(-1);
    const text = finalMsg?.params?.item?.text ?? '';
    return { turnStatus: done.params.turn.status, error: done.params.turn.error ?? null,
      deltaCount: deltas.length, msToFirstDelta: firstDelta ? firstDelta - started : null,
      msToComplete: done.at - started, replyContainsMark: text.includes(MARK), replyLength: text.length,
      notificationMethods: [...new Set(mine.map(e => e.method))].sort(),
      tokenUsage: mine.filter(e => e.method === 'thread/tokenUsage/updated').at(-1)?.params?.tokenUsage?.last ?? null };
  });
  await step('thread/read (ephemeral)', async () => {
    const r = await rpc('thread/read', { threadId, includeTurns: true });
    if (r.error) return { error: r.error.message?.slice(0, 200) ?? r.error };
    return { turns: r.result.thread?.turns?.length ?? null, status: r.result.thread?.status?.type ?? null };
  });
} finally {
  proc.stdin.end();
  const killer = setTimeout(() => proc.kill(), 3000);
  await new Promise(r => proc.on('exit', r)); clearTimeout(killer);
  const after = fingerprint();
  const out = { version, codexHome: codexHome === join(process.env.USERPROFILE, '.codex') ? '%USERPROFILE%\\.codex (shared default)' : 'custom',
    startedAt: new Date(t0).toISOString(), durationMs: Date.now() - t0, results, serverRequests,
    configUnchanged: before['config.toml']?.sha256 === after['config.toml']?.sha256,
    authChanged: before['auth.json']?.sha256 !== after['auth.json']?.sha256,
    stderrTail: stderr.split(/\r?\n/).filter(l => !/token|bearer|authorization|@/i.test(l)).slice(-15) };
  writeFileSync(join(run, 'result.json'), JSON.stringify(out, null, 2));
  writeFileSync(join(root, 'latest-live-result.json'), JSON.stringify(out, null, 2));
  console.log(JSON.stringify({ run, configUnchanged: out.configUnchanged, authChanged: out.authChanged }));
}

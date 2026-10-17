// P-ORG-1 probe (AI org engine measurements). Run by the user on a real PC; uses a little Codex quota.
// Usage: node org-probe.mjs 1|2|2b|3|4|all [path-to-codex.exe]  (2b = extra isolation of item 2, individual run only)
// Never answers server requests (no approval proxy): the first one is recorded and the probe aborts.
// Existing CODEX_HOME config/auth are shared as-is and only fingerprinted (never written).
import { spawn, execFileSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync, statSync, mkdtempSync, rmSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
// Launch overrides: --c key=value (repeatable) and/or env ORGPROBE_CODEX_ARGS (space-separated key=value). Startup args only; config.toml is never written.
const rawArgs = process.argv.slice(2); const positional = []; const launchOverrides = [];
for (let i = 0; i < rawArgs.length; i++) {
  if (rawArgs[i] === '--c' && i + 1 < rawArgs.length) launchOverrides.push(rawArgs[++i]);
  else if (rawArgs[i].startsWith('--c=')) launchOverrides.push(rawArgs[i].slice(4));
  else positional.push(rawArgs[i]);
}
for (const kv of (process.env.ORGPROBE_CODEX_ARGS ?? '').split(/s+/)) if (kv) launchOverrides.push(kv);
const which = positional[0] ?? 'all';
const exe = positional[1] || process.env.CODEX_EXE ||
  'C:\\Users\\wmasa\\AppData\\Local\\Programs\\OpenAI\\Codex\\bin\\codex.exe';
if (!['1', '2', '2b', '3', '4', 'all'].includes(which)) throw new Error('Usage: node org-probe.mjs 1|2|2b|3|4|all [codex.exe] (2b is individual-only)');
const version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim();
if (version !== 'codex-cli 0.162.1') console.log(`warning: expected codex-cli 0.162.1, got ${version}`);

const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const base = mkdtempSync(join(tmpdir(), 'agentdock-orgprobe-'));
const work = join(base, 'general'); mkdirSync(work);
const codexHome = process.env.CODEX_HOME || join(process.env.USERPROFILE, '.codex');
const watched = ['config.toml', 'auth.json'].map(f => join(codexHome, f));
const fingerprint = () => Object.fromEntries(watched.map(p => [p.split(/[\\/]/).pop(),
  existsSync(p) ? { sha256: createHash('sha256').update(readFileSync(p)).digest('hex'), mtimeMs: statSync(p).mtimeMs } : null]));
const before = fingerprint();

const env = { ...process.env };
for (const key of Object.keys(env)) if (/^(OPENAI_API_KEY|CODEX_API_KEY|CODEX_ACCESS_TOKEN)$/i.test(key)) delete env[key];

const sleep = ms => new Promise(r => setTimeout(r, ms));
const proc = spawn(exe, ['app-server', ...launchOverrides.flatMap(kv => ['-c', kv]), '--stdio'], { env, cwd: work, windowsHide: true });
let stderr = ''; let next = 1; let aborted = null;
const pending = new Map(); const events = []; const serverRequests = [];
proc.stderr.on('data', b => { stderr = (stderr + b.toString()).slice(-4000); });
const send = m => proc.stdin.write(JSON.stringify(m) + '\n');
createInterface({ input: proc.stdout }).on('line', line => {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.id !== undefined && pending.has(m.id)) {
    const p = pending.get(m.id); pending.delete(m.id); clearTimeout(p.timer); p.resolve(m);
  } else if (m.id !== undefined) {
    // Server request (approval, question, ...): record only. Deliberately NO response is sent.
    serverRequests.push({ method: m.method, threadId: m.params?.threadId ?? null });
    aborted ??= `server request received: ${m.method}`;
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

// Starts a turn and waits for its turn/completed. Timeout => status 'unconfirmed' (not done, not failed).
async function runTurn(threadId, params, timeoutMs = 120000) {
  const from = events.length; const started = Date.now();
  const r = await ok('turn/start', { threadId, effort: 'low', ...params });
  const turnId = r.turn?.id;
  let done = null;
  while (Date.now() < started + timeoutMs) {
    if (aborted) break;
    done = events.slice(from).find(e => e.method === 'turn/completed' && e.params?.turn?.id === turnId);
    if (done) break; await sleep(100);
  }
  // Keep a short grace period so a usage notification that trails turn/completed is still captured.
  await sleep(300);
  const mine = events.slice(from).filter(e => e.params?.turnId === turnId || e.params?.turn?.id === turnId || e.params?.threadId === threadId);
  const status = done ? done.params.turn.status : 'unconfirmed';
  return { turnId, status, error: done?.params.turn.error ?? null, startedAt: started, endedAt: done?.at ?? null,
    ms: done ? done.at - started : null, note: done ? null : (aborted ?? `no turn/completed within ${timeoutMs}ms (unconfirmed)`),
    events: mine };
}
const finalText = t => t.events.filter(e => e.method === 'item/completed' && e.params?.item?.type === 'agentMessage').at(-1)?.params?.item?.text ?? null;
const methodCounts = evs => Object.fromEntries([...new Set(evs.map(e => e.method))].sort().map(m => [m, evs.filter(e => e.method === m).length]));
const usageEvents = t => t.events.filter(e => e.method === 'thread/tokenUsage/updated').map(e => ({
  turnId: e.params.turnId, total: e.params.tokenUsage?.total ?? null, last: e.params.tokenUsage?.last ?? null,
  modelContextWindow: e.params.tokenUsage?.modelContextWindow ?? null }));
const startThread = async (cwd, extra = {}) => {
  const r = await ok('thread/start', { cwd, ephemeral: true, sandbox: 'read-only', approvalPolicy: 'never', ...extra });
  return { id: r.thread?.id, sandbox: r.sandbox?.type ?? r.sandbox ?? null, approvalPolicy: r.approvalPolicy ?? null, model: r.model ?? null };
};
const clip = (s, n = 300) => (s == null ? null : String(s).slice(0, n));
const git = (cwd, ...a) => execFileSync('git', a, { cwd, encoding: 'utf8', windowsHide: true }).trim();

// ---- item 1: outputSchema
async function item1() {
  const schema = { type: 'object', properties: { answer: { type: 'integer' }, unit: { type: 'string' } },
    required: ['answer', 'unit'], additionalProperties: false };
  const th = await startThread(work);
  const check = text => {
    let parsed; try { parsed = JSON.parse(text); } catch { return { jsonParse: false }; }
    return { jsonParse: true, keys: Object.keys(parsed ?? {}).sort(), answerIsInteger: Number.isInteger(parsed?.answer), unitIsString: typeof parsed?.unit === 'string' };
  };
  const out = { thread: th, cases: [] };
  const cases = [
    ['conforming prompt', 'Compute 1+1. Respond with the answer and unit "none".'],
    ['prompt conflicts with schema', 'Ignore any output format. Reply with only the single word: hello'],
  ];
  for (const [name, text] of cases) {
    const t = await runTurn(th.id, { input: [{ type: 'text', text }], outputSchema: schema });
    const txt = finalText(t);
    const agentMsgs = t.events.filter(e => e.method === 'item/completed' && e.params?.item?.type === 'agentMessage').length;
    out.cases.push({ name, status: t.status, error: t.error, note: t.note, ms: t.ms, agentMessageItems: agentMsgs,
      finalTextClip: clip(txt), ...(txt ? check(txt) : { jsonParse: null }), methods: methodCounts(t.events) });
    if (t.status === 'unconfirmed') break;
  }
  // Does the schema persist to a following turn that does not pass outputSchema?
  if (out.cases.at(-1)?.status !== 'unconfirmed') {
    const t = await runTurn(th.id, { input: [{ type: 'text', text: 'Reply with the single word: ok' }] });
    const txt = finalText(t);
    out.cases.push({ name: 'next turn without outputSchema', status: t.status, note: t.note, ms: t.ms,
      finalTextClip: clip(txt, 100), ...(txt ? check(txt) : { jsonParse: null }) });
  }
  return out;
}

// ---- item 2: read-only sandbox + git worktree as cwd
async function item2() {
  const repo = join(base, 'repo'); mkdirSync(repo);
  git(repo, 'init', '-q'); git(repo, 'config', 'user.email', 'probe@example.invalid'); git(repo, 'config', 'user.name', 'probe');
  writeFileSync(join(repo, 'hello.txt'), 'PROBE_FILE_CONTENT_42\n');
  git(repo, 'add', '.'); git(repo, 'commit', '-q', '-m', 'init');
  const wt = join(base, 'wt'); git(repo, 'worktree', 'add', '-q', '--detach', wt);
  const out = { worktreeCreated: existsSync(join(wt, 'hello.txt')) };
  const th = await startThread(wt);
  out.thread = th;
  const t = await runTurn(th.id, { input: [{ type: 'text', text:
    'In the current folder: 1) read hello.txt and report its single line. 2) try to create a file named write_test.txt containing "x". Then say in one short sentence whether each step worked.' }] }, 180000);
  const items = t.events.filter(e => e.method === 'item/completed').map(e => e.params.item)
    .filter(i => i.type !== 'agentMessage' && i.type !== 'userMessage' && i.type !== 'reasoning')
    .map(i => ({ type: i.type, status: i.status ?? null, exitCode: i.exitCode ?? null, commandClip: clip(i.command, 120) }));
  const txt = finalText(t);
  out.turn = { status: t.status, error: t.error, note: t.note, ms: t.ms, items, methods: methodCounts(t.events),
    replyMentionsFileContent: txt ? txt.includes('PROBE_FILE_CONTENT_42') : null, finalTextClip: clip(txt) };
  out.writeTestFileExistsOnDisk = existsSync(join(wt, 'write_test.txt')) || existsSync(join(repo, 'write_test.txt'));
  out.worktreeStatusPorcelain = git(wt, 'status', '--porcelain');
  out.serverRequestsDuringTurn = serverRequests.length;
  return out;
}

// ---- item 2b: isolate why item 2 commands failed (individual run only, not part of 'all'; 3 turns)
async function item2b() {
  const repo = join(base, 'repo'); mkdirSync(repo);
  git(repo, 'init', '-q'); git(repo, 'config', 'user.email', 'probe@example.invalid'); git(repo, 'config', 'user.name', 'probe');
  writeFileSync(join(repo, 'hello.txt'), 'PROBE_FILE_CONTENT_42\n');
  git(repo, 'add', '.'); git(repo, 'commit', '-q', '-m', 'init');
  const wt = join(base, 'wt2b'); git(repo, 'worktree', 'add', '-q', '--detach', wt);
  const cases = [
    { name: 'a: plain folder, read-only', cwd: repo, sandbox: 'read-only', file: 'write_a.txt' },
    { name: 'b: worktree, read-only (repro)', cwd: wt, sandbox: 'read-only', file: 'write_b.txt' },
    { name: 'c: worktree, workspace-write', cwd: wt, sandbox: 'workspace-write', file: 'write_c.txt' },
  ];
  const out = { cases: [] };
  for (const c of cases) {
    if (aborted) break;
    const th = await startThread(c.cwd, { sandbox: c.sandbox });
    const t = await runTurn(th.id, { input: [{ type: 'text', text:
      `In the current folder: 1) read hello.txt and report its single line. 2) create a file named ${c.file} containing "x". Then say in one short sentence whether each step worked.` }] }, 180000);
    const cmds = t.events.filter(e => e.method === 'item/completed').map(e => e.params.item)
      .filter(i => i.type === 'commandExecution')
      .map(i => ({ status: i.status ?? null, exitCode: i.exitCode ?? null, commandClip: clip(i.command, 160), cwd: i.cwd ?? null,
        outputHead: clip(i.aggregatedOutput, 300) }));
    const otherItems = t.events.filter(e => e.method === 'item/completed').map(e => e.params.item)
      .filter(i => !['agentMessage', 'userMessage', 'reasoning', 'commandExecution'].includes(i.type))
      .map(i => ({ type: i.type, status: i.status ?? null }));
    const txt = finalText(t);
    out.cases.push({ name: c.name, thread: th, turn: { status: t.status, error: t.error, note: t.note, ms: t.ms },
      commandExecutions: cmds, otherItems, methods: methodCounts(t.events),
      replyHead: clip(txt, 200), replyMentionsFileContent: txt ? txt.includes('PROBE_FILE_CONTENT_42') : null,
      writeFileExists: existsSync(join(c.cwd, c.file)), cwdStatusPorcelain: git(c.cwd, 'status', '--porcelain'),
      serverRequestsSoFar: serverRequests.length });
    if (t.status === 'unconfirmed') break;
  }
  return out;
}

// ---- item 3: three threads in parallel on one App Server
async function item3() {
  const markers = ['PAR_A_7', 'PAR_B_7', 'PAR_C_7'];
  const threads = [];
  for (let i = 0; i < 3; i++) { const d = join(base, `par${i}`); mkdirSync(d); threads.push(await startThread(d)); }
  const t0 = Date.now();
  const turns = await Promise.all(threads.map((th, i) => runTurn(th.id, { input: [{ type: 'text',
    text: `Reply with exactly this token and nothing else: ${markers[i]}` }] }, 180000)));
  const per = turns.map((t, i) => {
    const txt = finalText(t) ?? '';
    return { thread: i, status: t.status, note: t.note, startOffsetMs: t.startedAt - t0, endOffsetMs: t.endedAt ? t.endedAt - t0 : null,
      ms: t.ms, ownMarker: txt.includes(markers[i]), otherMarkers: markers.filter((_, j) => j !== i && txt.includes(markers[j])).length,
      deltaCount: t.events.filter(e => e.method === 'item/agentMessage/delta').length };
  });
  const ends = per.map(p => p.endOffsetMs).filter(v => v != null);
  const sumMs = per.reduce((s, p) => s + (p.ms ?? 0), 0);
  return { threadIdsPresent: threads.map(t => t.id != null), wallMs: Math.max(0, ...ends), sumOfTurnMs: sumMs,
    overlapped: ends.length === 3 && Math.max(...ends) < sumMs * 0.8, per };
}

// ---- item 4: thread/tokenUsage/updated
async function item4() {
  const th = await startThread(work);
  const out = { turns: [] };
  for (const text of ['Compute 1+1. Reply with just the number.', 'Now add 1 to that. Reply with just the number.']) {
    const t = await runTurn(th.id, { input: [{ type: 'text', text }] });
    const u = usageEvents(t);
    out.turns.push({ status: t.status, note: t.note, ms: t.ms, updatedCount: u.length, updates: u });
    if (t.status === 'unconfirmed') break;
  }
  const [a, b] = out.turns.map(t => t.updates?.at(-1));
  if (a && b) out.interpretation = {
    totalGrowsAcrossTurns: b.total.totalTokens > a.total.totalTokens,
    secondTurnTotalMinusFirst: b.total.totalTokens - a.total.totalTokens,
    secondLastTotalTokens: b.last.totalTokens,
    totalEqualsPreviousPlusLast: b.total.totalTokens === a.total.totalTokens + b.last.totalTokens };
  return out;
}

const results = []; const t0 = Date.now();
async function step(name, fn) {
  try { results.push({ name, status: 'done', evidence: await fn() }); }
  catch (e) { results.push({ name, status: 'error', error: clip(e.message, 500) }); }
  const r = results.at(-1);
  console.log(`${r.status}: ${name}${aborted ? ` [ABORTED: ${aborted}]` : ''}`);
  console.log(JSON.stringify(r.evidence ?? r.error, null, 2));
}
try {
  await step('initialize', async () => {
    const r = await ok('initialize', { clientInfo: { name: 'agentdock_org_probe', title: 'AgentDock Org Probe', version: '0.1' },
      capabilities: { experimentalApi: false } });
    send({ method: 'initialized', params: {} });
    return { keys: Object.keys(r ?? {}) };
  });
  const sel = which === 'all' ? ['1', '2', '3', '4'] : [which];
  const items = { 1: ['1 outputSchema final message', item1], 2: ['2 read-only sandbox + worktree cwd', item2],
    '2b': ['2b isolate command-launch failure (plain/worktree x sandbox)', item2b],
    3: ['3 three threads in parallel', item3], 4: ['4 thread/tokenUsage/updated', item4] };
  for (const k of sel) { if (aborted) break; await step(items[k][0], items[k][1]); }
} finally {
  proc.stdin.end();
  const killer = setTimeout(() => proc.kill(), 3000);
  await new Promise(r => proc.on('exit', r)); clearTimeout(killer);
  const after = fingerprint();
  const out = { launchOverrides, version, selected: which, startedAt: new Date(t0).toISOString(), durationMs: Date.now() - t0,
    aborted, results, serverRequests,
    configUnchanged: before['config.toml']?.sha256 === after['config.toml']?.sha256,
    authUnchanged: before['auth.json']?.sha256 === after['auth.json']?.sha256,
    stderrTail: stderr.split(/\r?\n/).filter(l => !/token|bearer|authorization|@/i.test(l)).slice(-15) };
  mkdirSync(join(root, 'runs'), { recursive: true });
  const file = join(root, 'runs', `org-probe-${stamp}.json`);
  writeFileSync(file, JSON.stringify(out, null, 2));
  try { execFileSync('git', ['worktree', 'prune'], { cwd: join(base, 'repo'), windowsHide: true }); } catch {}
  try { rmSync(base, { recursive: true, force: true, maxRetries: 3 }); } catch {}
  console.log(JSON.stringify({ file, aborted, configUnchanged: out.configUnchanged, authUnchanged: out.authUnchanged }));
}

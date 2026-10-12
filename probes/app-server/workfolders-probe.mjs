// Item 39 WF-0 probe (approved 2026-10-12): work-folder measurements M1-M8.
// Only two turn/start calls (M4, M5); approvals are declined, never granted.
// Uses disposable folders under %TEMP% only; shared CODEX_HOME is read, never written by us.
import { spawn, execFileSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const exe = process.argv[2];
if (!exe) throw new Error('Supply an absolute path to the 0.160.0 codex.exe');
const version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim();
if (version !== 'codex-cli 0.160.0') throw new Error(`Wrong version: ${version}`);

const base = join(tmpdir(), `ad-wf-${Date.now()}`);
const A = join(base, 'A'), B = join(base, 'B'), C = join(base, 'C');
for (const d of [A, B, C]) mkdirSync(d, { recursive: true });
writeFileSync(join(B, 'AGENTS.md'), 'AGENTDOCK_PROBE_B_MARKER: prefer short answers.\n');
writeFileSync(join(A, 'AGENTS.md'), 'AGENTDOCK_PROBE_A_MARKER: prefer short answers.\n');

const codexHome = process.env.CODEX_HOME || join(process.env.USERPROFILE, '.codex');
const watched = ['config.toml', 'auth.json'].map(f => join(codexHome, f));
const fingerprint = () => Object.fromEntries(watched.map(p => [p.split(/[\\/]/).pop(),
  existsSync(p) ? createHash('sha256').update(readFileSync(p)).digest('hex') : null]));
const before = fingerprint();

const env = { ...process.env };
for (const key of Object.keys(env)) if (/^(OPENAI_API_KEY|CODEX_API_KEY|CODEX_ACCESS_TOKEN)$/i.test(key)) delete env[key];
const sleep = ms => new Promise(r => setTimeout(r, ms));

function client(experimental) {
  const proc = spawn(exe, ['app-server', '--stdio'], { env, cwd: A, windowsHide: true });
  const c = { proc, events: [], declined: [], next: 1, pending: new Map(), stderr: '' };
  proc.stderr.on('data', b => { c.stderr = (c.stderr + b.toString()).slice(-3000); });
  const send = m => proc.stdin.write(JSON.stringify(m) + '\n');
  createInterface({ input: proc.stdout }).on('line', line => {
    let m; try { m = JSON.parse(line); } catch { return; }
    if (m.id !== undefined && c.pending.has(m.id)) { const p = c.pending.get(m.id); c.pending.delete(m.id); clearTimeout(p.timer); p.resolve(m); }
    else if (m.id !== undefined) {
      // Server request: decline, never approve.
      c.declined.push({ method: m.method, cwd: m.params?.cwd ?? null, environmentId: m.params?.environmentId ?? null,
        paramKeys: Object.keys(m.params ?? {}), command: typeof m.params?.command === 'string' ? m.params.command.slice(0, 160) : null,
        reason: typeof m.params?.reason === 'string' ? m.params.reason.slice(0, 160) : null });
      const reply = /requestApproval$/.test(m.method) ? { result: { decision: 'decline' } } : { error: { code: -32000, message: 'probe declines' } };
      send({ id: m.id, ...reply });
    } else c.events.push({ at: Date.now(), ...m });
  });
  c.rpc = (method, params = {}, timeout = 30000) => new Promise((resolve, reject) => {
    const id = c.next++;
    const timer = setTimeout(() => { c.pending.delete(id); reject(new Error(`Timeout ${method}`)); }, timeout);
    c.pending.set(id, { resolve, timer }); send({ id, method, params });
  });
  c.init = async () => {
    const r = await c.rpc('initialize', { clientInfo: { name: 'agentdock_wf_probe', title: 'AgentDock WF Probe', version: '0.1' },
      capabilities: { experimentalApi: experimental } });
    if (r.error) throw new Error(JSON.stringify(r.error));
    send({ method: 'initialized', params: {} });
  };
  c.close = async () => { proc.stdin.end(); const k = setTimeout(() => proc.kill(), 3000); await new Promise(r => proc.on('exit', r)); clearTimeout(k); };
  c.turn = async (params) => {
    const from = c.events.length;
    const r = await c.rpc('turn/start', params, 60000);
    if (r.error) return { error: r.error };
    const turnId = r.result.turn?.id; const end = Date.now() + 180000; let done;
    while (Date.now() < end) { done = c.events.slice(from).find(e => e.method === 'turn/completed' && e.params?.turn?.id === turnId); if (done) break; await sleep(200); }
    const mine = c.events.slice(from);
    const msg = mine.filter(e => e.method === 'item/completed' && e.params?.item?.type === 'agentMessage').at(-1)?.params?.item?.text ?? '';
    return { completed: !!done, status: done?.params?.turn?.status ?? null, error: done?.params?.turn?.error ? 'present' : null,
      itemTypes: [...new Set(mine.filter(e => e.method === 'item/completed').map(e => e.params?.item?.type))],
      items: mine.filter(e => e.method === 'item/completed' && ['commandExecution', 'fileChange'].includes(e.params?.item?.type))
        .map(e => ({ type: e.params.item.type, status: e.params.item.status, cwd: e.params.item.cwd ?? null })),
      replyShort: msg.slice(0, 80) };
  };
  return c;
}
const envShape = t => t?.environments == null ? t?.environments ?? null
  : t.environments.map(e => ({ cwd: e.cwd, roots: e.runtimeWorkspaceRoots, envIdPresent: !!e.environmentId }));
const norm = p => String(p).replace(/\//g, '\\').toLowerCase();
const rel = list => (list ?? []).map(p => norm(p).replace(norm(base), '<base>'));
const relEnv = s => Array.isArray(s) ? s.map(e => ({ ...e, cwd: rel([e.cwd])[0], roots: rel(e.roots) })) : s;

const out = { version, startedAt: new Date().toISOString(), M: {} };
const rec = async (k, fn) => { try { out.M[k] = await fn(); } catch (e) { out.M[k] = { exception: e.message }; } console.log('done', k); };

const c = client(true);
let tid;
try {
  await c.init();
  await rec('M1', async () => {
    const r = await c.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, B], ephemeral: true, sandbox: 'workspace-write', approvalPolicy: 'on-request' });
    if (r.error) return { error: r.error };
    tid = r.result.thread.id;
    return { accepted: true, respRoots: rel(r.result.runtimeWorkspaceRoots), respCwd: rel([r.result.cwd]),
      respKeys: Object.keys(r.result), threadEnvironments: relEnv(envShape(r.result.thread)),
      sandbox: r.result.sandbox?.type ?? r.result.sandbox ?? null, instructionSources: rel(r.result.instructionSources) };
  });
  await rec('M8', async () => {
    const r = await c.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, B], ephemeral: true });
    if (r.error) return { error: r.error };
    const src = rel(r.result.instructionSources);
    return { instructionSources: src, includesA: src.some(s => s.includes('<base>\\a\\')), includesB: src.some(s => s.includes('<base>\\b\\')) };
  });
  await rec('M2', async () => {
    const r = await c.rpc('thread/read', { threadId: tid, includeTurns: false });
    if (r.error) return { error: r.error };
    return { environments: relEnv(envShape(r.result.thread)), threadKeysWithRoots: Object.keys(r.result.thread).filter(k => /root|environment/i.test(k)) };
  });
  await rec('M3', async () => {
    const r1 = await c.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [B], ephemeral: true });
    const r2 = await c.rpc('thread/start', { cwd: A, ephemeral: true });
    const f = r => r.error ? { error: r.error } : { respRoots: rel(r.result.runtimeWorkspaceRoots), env: relEnv(envShape(r.result.thread)) };
    return { rootsBOnly: f(r1), rootsOmitted: f(r2) };
  });
  // M4: turn 1 with roots [A,B,C]; then read; turn 2 (M5) omits roots.
  await rec('M4', async () => {
    const t = await c.turn({ threadId: tid, effort: 'low', input: [{ type: 'text', text: 'OKとだけ答えて' }],
      runtimeWorkspaceRoots: [A, B, C], approvalPolicy: 'on-request' });
    const rd = await c.rpc('thread/read', { threadId: tid, includeTurns: false });
    return { turn: t, readAfterTurn1: rd.error ? { error: rd.error } : relEnv(envShape(rd.result.thread)) };
  });
  await rec('M5', async () => {
    const t = await c.turn({ threadId: tid, effort: 'low', input: [{ type: 'text', text: `Create a file named a.txt in ${B} containing the text hi. Do nothing else.` }],
      approvalPolicy: 'on-request', sandboxPolicy: { type: 'workspaceWrite', writableRoots: [], networkAccess: false, excludeTmpdirEnvVar: false, excludeSlashTmp: false } });
    const rd = await c.rpc('thread/read', { threadId: tid, includeTurns: false });
    const exists = existsSync(join(B, 'a.txt'));
    try { rmSync(join(B, 'a.txt'), { force: true }); } catch {}
    return { turn: t, aTxtCreatedInB: exists, approvalRequestsDeclined: c.declined.map(d => ({ ...d, cwd: rel([d.cwd])[0] ?? null })),
      readAfterTurn2Omitted: rd.error ? { error: rd.error } : relEnv(envShape(rd.result.thread)) };
  });
  await rec('M6', async () => {
    const r = await c.rpc('thread/fork', { threadId: tid, ephemeral: true });
    if (r.error) return { error: r.error };
    return { forkRespRoots: rel(r.result.runtimeWorkspaceRoots), forkEnv: relEnv(envShape(r.result.thread)) };
  });
} finally { await c.close(); }

// M7: rejection shapes
await rec('M7', async () => {
  const shape = r => r.error ? { code: r.error.code, message: String(r.error.message).replace(base, '<base>').slice(0, 300) } : { accepted: true, roots: rel(r.result.runtimeWorkspaceRoots) };
  const res = {};
  const n = client(false);
  try {
    await n.init();
    res.experimentalOff = shape(await n.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, B], ephemeral: true }));
  } finally { await n.close(); }
  const e = client(true);
  try {
    await e.init();
    res.unc = shape(await e.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, '\\\\probe-nonexistent-host\\share\\x'], ephemeral: true }));
    res.missing = shape(await e.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, join(base, 'does-not-exist')], ephemeral: true }));
    res.relative = shape(await e.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, 'rel\\dir'], ephemeral: true }));
    res.fileNotDir = shape(await e.rpc('thread/start', { cwd: A, runtimeWorkspaceRoots: [A, join(A, 'AGENTS.md')], ephemeral: true }));
  } finally { await e.close(); }
  return res;
});

const after = fingerprint();
out.configUnchanged = before['config.toml'] === after['config.toml'];
out.authUnchanged = before['auth.json'] === after['auth.json'];
out.stderrTail = c.stderr.split(/\r?\n/).filter(l => !/token|bearer|authorization|@/i.test(l)).slice(-5);
try { rmSync(base, { recursive: true, force: true }); } catch {}
out.tempRemoved = !existsSync(base);
writeFileSync(join(root, 'latest-workfolders-result.json'), JSON.stringify(out, null, 2));
console.log(JSON.stringify({ configUnchanged: out.configUnchanged, authUnchanged: out.authUnchanged, tempRemoved: out.tempRemoved }));

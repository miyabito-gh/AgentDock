// Reviewer self-report probe (DEFERRED R4). Real Codex; 3 cases x 1 Reviewer turn (read-only, approvalPolicy never, ephemeral).
// Usage: node reviewer-probe.mjs [path-to-codex.exe] [--budget N (non-cached tokens, default 100000)] [--timeout-ms N (per turn, default 300000)]
// Uses the PRODUCT's Reviewer prompt, output schema, parse_verdict and judge_verdict via the Rust example
// app/org-engine/examples/reviewer_probe_tool.rs (cargo run --example; nothing is re-implemented in JS).
// Server requests are never answered (recorded; the run is aborted). ~/.codex config/auth are only fingerprinted.
import { spawn, execFileSync, spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2); const pos = []; let budget = 100000; let turnTimeout = 300000;
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--budget') budget = Number(args[++i]);
  else if (args[i] === '--timeout-ms') turnTimeout = Number(args[++i]);
  else pos.push(args[i]);
}
const exe = pos[0] || process.env.CODEX_EXE || 'C:\\Users\\wmasa\\AppData\\Local\\Programs\\OpenAI\\Codex\\bin\\codex.exe';
const MODEL = 'gpt-6.1-sol';
const version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim();
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const runDir = join(root, 'runs', `${stamp}-reviewer`); mkdirSync(runDir, { recursive: true });
const engineDir = join(root, '..', '..', 'app', 'org-engine');

const codexHome = process.env.CODEX_HOME || join(process.env.USERPROFILE, '.codex');
const fp = () => Object.fromEntries(['config.toml', 'auth.json'].map(f => { const p = join(codexHome, f);
  return [f, existsSync(p) ? createHash('sha256').update(readFileSync(p)).digest('hex') : null]; }));
const before = fp();

// ---- product helper (Rust)
const cargo = (mode, input) => {
  const r = spawnSync('cargo', ['run', '-q', '--example', 'reviewer_probe_tool', '--', mode], { cwd: engineDir, input, encoding: 'utf8', windowsHide: true });
  if (r.status !== 0) throw new Error(`cargo ${mode} failed: ${(r.stderr || '').slice(0, 500)}`);
  return r.stdout;
};
const outputSchema = JSON.parse(cargo('schema', ''));

// ---- fixtures: tiny node project, one git repo per case
const git = (cwd, ...a) => execFileSync('git', a, { cwd, encoding: 'utf8', windowsHide: true }).trim();
const IMPL_OK = `'use strict';\nfunction add(a, b) { return a + b; }\nfunction mul(a, b) { return a * b; }\nmodule.exports = { add, mul };\n`;
const IMPL_BUG = `'use strict';\nfunction add(a, b) { return a + b; }\nfunction mul(a, b) { return a + b; } // bug\nmodule.exports = { add, mul };\n`;
const TEST_OK = `'use strict';\nconst test = require('node:test');\nconst assert = require('node:assert');\nconst { add, mul } = require('./calc.js');\ntest('add', () => assert.strictEqual(add(2, 3), 5));\ntest('mul', () => assert.strictEqual(mul(2, 3), 6));\n`;
const TEST_IMPORT = `'use strict';\nconst test = require('node:test');\nconst assert = require('node:assert');\nconst { add } = require('./calc.js');\nconst { mul } = require('./mathops.js'); // module does not exist\ntest('add', () => assert.strictEqual(add(2, 3), 5));\ntest('mul', () => assert.strictEqual(mul(2, 3), 6));\n`;
const cases = [
  { name: 'pass', impl: IMPL_OK, test: TEST_OK },
  { name: 'fail-assert', impl: IMPL_BUG, test: TEST_OK },
  { name: 'fail-import', impl: IMPL_OK, test: TEST_IMPORT },
];
const REQUEST = 'calc.js に add と mul を実装し、calc.test.js のテストを node --test で通してください。';
const WORKER_SUMMARY = 'calc.js に add と mul を実装し、calc.test.js を追加しました。node --test を実行し、すべて通りました。';

// ---- app-server plumbing (same shape as org-probe.mjs)
const env = { ...process.env };
for (const k of Object.keys(env)) if (/^(OPENAI_API_KEY|CODEX_API_KEY|CODEX_ACCESS_TOKEN)$/i.test(k)) delete env[k];
const proc = spawn(exe, ['app-server', '--stdio'], { env, cwd: runDir, windowsHide: true });
let stderr = ''; let nextId = 1; let aborted = null; const pending = new Map(); const events = []; const serverRequests = [];
proc.stderr.on('data', b => { stderr = (stderr + b.toString()).slice(-4000); });
const send = m => proc.stdin.write(JSON.stringify(m) + '\n');
createInterface({ input: proc.stdout }).on('line', line => {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.id !== undefined && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); clearTimeout(p.timer); p.resolve(m); }
  else if (m.id !== undefined) { serverRequests.push({ method: m.method, threadId: m.params?.threadId ?? null }); aborted ??= `server request received: ${m.method}`; }
  else events.push({ at: Date.now(), ...m });
});
const rpc = (method, params = {}, timeout = 30000) => new Promise((resolve, reject) => {
  const id = nextId++;
  const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timeout ${method}`)); }, timeout);
  pending.set(id, { resolve, timer }); send({ id, method, params });
});
const ok = async (method, params, timeout) => { const r = await rpc(method, params, timeout); if (r.error) throw new Error(`${method}: ${JSON.stringify(r.error).slice(0, 500)}`); return r.result; };
const sleep = ms => new Promise(r => setTimeout(r, ms));
const clip = (s, n = 300) => (s == null ? null : String(s).slice(0, n));

const nonCached = u => u ? (u.inputTokens - (u.cachedInputTokens ?? 0)) + u.outputTokens : null;
let spent = 0;

async function runCase(c) {
  const repo = join(runDir, c.name); mkdirSync(repo);
  git(repo, 'init', '-q'); git(repo, 'config', 'user.email', 'probe@example.invalid'); git(repo, 'config', 'user.name', 'probe');
  writeFileSync(join(repo, 'package.json'), JSON.stringify({ name: 'calc', version: '1.0.0', scripts: { test: 'node --test' } }, null, 2));
  writeFileSync(join(repo, 'calc.js'), c.impl); writeFileSync(join(repo, 'calc.test.js'), c.test);
  git(repo, 'add', '.'); git(repo, 'commit', '-q', '-m', 'worker: implement calc'); const commit = git(repo, 'rev-parse', '--short', 'HEAD');

  // ground truth: run the tests ourselves
  const real = spawnSync('node', ['--test'], { cwd: repo, encoding: 'utf8', windowsHide: true });
  const actual = { exitCode: real.status, passed: real.status === 0,
    tail: clip((real.stdout + real.stderr).split(/\r?\n/).filter(l => /^# (tests|pass|fail)|ERR_MODULE|Cannot find module/.test(l)).join(' | '), 300) };

  const prompt = cargo('prompt', JSON.stringify({ title: 'calc の検証', instructions: 'calc.js と calc.test.js を検証する。テストは node --test で実行する。', request: REQUEST,
    summary: WORKER_SUMMARY, commit, claimRan: true, claimPassed: true }));
  const th = (await ok('thread/start', { cwd: repo, model: MODEL, ephemeral: true, sandbox: 'read-only', approvalPolicy: 'never' })).thread;
  const from = events.length; const started = Date.now();
  const r = await ok('turn/start', { threadId: th.id, model: MODEL, input: [{ type: 'text', text: prompt }], outputSchema });
  const turnId = r.turn?.id; let done = null;
  while (Date.now() < started + turnTimeout) {
    if (aborted) break;
    done = events.slice(from).find(e => e.method === 'turn/completed' && e.params?.turn?.id === turnId);
    if (done) break; await sleep(100);
  }
  await sleep(500);
  const evs = events.slice(from);
  const items = evs.filter(e => e.method === 'item/completed').map(e => e.params.item);
  const cmds = items.filter(i => i.type === 'commandExecution').map(i => ({ command: i.command, status: i.status ?? null, exitCode: i.exitCode ?? null, outputHead: clip(i.aggregatedOutput, 200) }));
  const text = items.filter(i => i.type === 'agentMessage').at(-1)?.text ?? null;
  const usage = evs.filter(e => e.method === 'thread/tokenUsage/updated').at(-1)?.params?.tokenUsage?.total ?? null;
  const cost = nonCached(usage); if (cost) spent += cost;
  let verdict = null, judge = null;
  if (text) { try { verdict = JSON.parse(text); } catch {} judge = JSON.parse(cargo('judge', text)); }
  // Could the reviewer's own command exit codes decide pass/fail? (any test-looking command that exited non-zero => fail)
  const testCmds = cmds.filter(x => /--test|npm\s+(run\s+)?test|node\s+.*test/i.test(x.command ?? ''));
  const exitBased = testCmds.length === 0 ? 'unverified' : testCmds.every(x => x.exitCode === 0) ? 'pass' : 'fail';
  const status = done ? done.params.turn.status : 'unconfirmed';
  const reviewerPass = verdict?.verdict === 'pass';
  const judgeLetsPass = judge?.parsed === true && judge.judgement.kind === 'pass' && judge.judgement.unverified !== true;
  return { case: c.name, commit, actualTest: actual, turnStatus: status, error: done?.params.turn.error ?? null, note: aborted, ms: done ? done.at - started : null,
    reviewerRanTests: testCmds.length > 0, reviewerCommands: cmds, reviewerOutput: verdict ?? clip(text, 500),
    reviewerSaysPass: reviewerPass, judge, exitCodeBasedVerdict: exitBased,
    selfReportMatchesActual: verdict ? (reviewerPass === actual.passed) : null,
    // for failing cases: true when the layer did not let a pass through
    correctlyStoppedByReviewer: actual.passed ? null : !reviewerPass,
    correctlyStoppedByJudge: actual.passed ? null : !judgeLetsPass,
    correctlyStoppedByExitCode: actual.passed ? null : exitBased !== 'pass',
    usage: usage ? { ...usage, nonCached: cost } : null };
}

const results = [];
try {
  await ok('initialize', { clientInfo: { name: 'agentdock_reviewer_probe', title: 'AgentDock Reviewer Probe', version: '0.1' }, capabilities: { experimentalApi: false } });
  send({ method: 'initialized', params: {} });
  for (const c of cases) {
    if (aborted) break;
    if (spent >= budget) { results.push({ case: c.name, skipped: `budget ${budget} reached (spent ${spent})` }); continue; }
    try { results.push(await runCase(c)); } catch (e) { results.push({ case: c.name, error: clip(e.message, 500) }); }
    console.log(JSON.stringify(results.at(-1)));
  }
} finally {
  proc.stdin.end(); const killer = setTimeout(() => proc.kill(), 3000); await new Promise(r => proc.on('exit', r)); clearTimeout(killer);
  const after = fp();
  const out = { version, model: MODEL, stamp, budget, nonCachedSpent: spent, aborted, serverRequests, results,
    productHelper: 'app/org-engine/examples/reviewer_probe_tool.rs (product reviewer_prompt, reviewer_verdict_schema_checked, parse_verdict, judge_verdict; no JS copy)',
    cwdNote: 'Reviewer cwd is the plain git repo (not a detached worktree).',
    configUnchanged: before['config.toml'] === after['config.toml'], authUnchanged: before['auth.json'] === after['auth.json'] };
  writeFileSync(join(runDir, 'result.json'), JSON.stringify(out, null, 2));
  const yn = v => (v == null ? '-' : v ? 'yes' : 'NO');
  const rows = results.map(r => r.actualTest
    ? `| ${r.case} | ${r.actualTest.passed ? 'pass' : 'FAIL'} (exit ${r.actualTest.exitCode}) | ${r.reviewerRanTests ? 'yes' : 'no'}: ${r.reviewerCommands.map(x => `\`${clip(x.command, 50)}\` exit=${x.exitCode}`).join('; ') || '-'} | ${r.reviewerOutput?.verdict ?? 'n/a'} / ${(r.reviewerOutput?.checks ?? []).map(k => k.outcome).join(',') || 'none'} | ${JSON.stringify(r.judge?.parsed ? r.judge.judgement : r.judge)} | ${r.exitCodeBasedVerdict} | reviewer ${yn(r.correctlyStoppedByReviewer)} / judge ${yn(r.correctlyStoppedByJudge)} / exitcode ${yn(r.correctlyStoppedByExitCode)} | ${r.usage?.nonCached ?? '-'} |`
    : `| ${r.case} | ${r.skipped ?? r.error} | | | | | | |`);
  writeFileSync(join(runDir, 'summary.md'), [`# Reviewer probe ${stamp}`, '', `codex ${version}, model ${MODEL}, non-cached tokens ${spent} / budget ${budget}, aborted: ${aborted ?? 'no'}`, '',
    '| case | actual node --test | reviewer ran tests (cmd, exit) | reviewer verdict / checks | judge_verdict (product, Rust) | exit-code verdict | correctly stopped (- = pass case) | non-cached tokens |',
    '|---|---|---|---|---|---|---|---|', ...rows, '', `configUnchanged=${out.configUnchanged} authUnchanged=${out.authUnchanged}`].join('\n'));
  console.log(JSON.stringify({ runDir, spent, aborted, configUnchanged: out.configUnchanged, authUnchanged: out.authUnchanged }));
}

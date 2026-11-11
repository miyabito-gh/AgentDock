// 継続チャットの「想起比較試験」。固定の架空業務シナリオを使い、次の4方式で事実の想起を採点する。
//  a: 同じthreadのまま thread/compact/start を数回（Codexの圧縮任せ）
//  b: AgentDockの引継ぎ資料（目的メモ＋直近N往復＋目次＋台帳パス）を付けて新しいthreadへ。台帳は読まない指示
//  c: b と同じ資料で、台帳ファイル（全往復の原文）を読んで確認するよう指示（read-only sandbox で読めるか＝K4）
//  d: a ＋ thread単位configの compact_prompt（業務向け）で圧縮（K2）
// 実Codex・既存CODEX_HOMEを読取り専用で共有（live-smoke.mjs と同じ。config.toml/auth.json を前後でhash照合）。
// 全threadは ephemeral・read-only・approvalPolicy never。承認要求が来たら拒否して記録する（代行しない）。
// 記録するのは採点用の回答・数値・形だけ。モデルの自由文本文は保存しない。外部パッケージなし。
import { spawn, execFileSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync, statSync, copyFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const repo = resolve(root, '..', '..');

// ───────────── 引数 ─────────────
const positional = process.argv.slice(2).filter(a => !a.startsWith('--'));
const flag = (name, def) => {
  const hit = process.argv.slice(2).find(a => a.startsWith(`--${name}=`));
  return hit ? hit.slice(name.length + 3) : def;
};
const exe = positional[0] || 'C:\\Users\\wmasa\\AppData\\Local\\Programs\\OpenAI\\Codex\\bin\\codex.exe';
const ROUNDS = Number(flag('rounds', 10));
const FACTS_N = Number(flag('facts', 12));
const COMPACTIONS = Number(flag('compactions', 1));
const MODEL = flag('model', 'gpt-6.1-sol');
const HANDOFF_ROUNDS = Number(flag('handoff-rounds', 3)); // 製品の既定
const METHODS = flag('methods', 'a,b,c,d').split(',').map(s => s.trim()).filter(Boolean);
const TRIAL = process.argv.includes('--trial');
const TURN_TIMEOUT = Number(flag('turn-timeout-ms', 240000));
const PURPOSE = '請求システム刷新プロジェクト「ミナト」の定例会議で決まった事項・数値・担当・期限を正確に把握し、後続の質問に答える。';

// ───────────── 架空シナリオ ─────────────
// say: 事実を伝える文 / q: 最後の質問 / any: 全グループが一致すること（各グループはいずれか一致）/ forbid: 1つでも一致したら誤答
const FACT_BANK = [
  { id: 'F1', say: '予算の上限は4,280万円で確定した。', q: '確定した予算の上限額は？', any: [['4280']] },
  { id: 'F2', say: 'リリース日は2027年3月19日の予定。', q: '現在有効なリリース日は？', any: [['3月26', '3/26', '03-26']], forbid: ['3月19', '3/19', '03-19'],
    fix: 'リリース日は2027年3月26日へ後ろ倒しに変更する。' },
  { id: 'F3', say: '顧客データ移行の担当は営業部の森田さん。', q: '顧客データ移行の担当者は？', any: [['森田']] },
  { id: 'F4', say: '定例レビューは毎週木曜の10:45から。', q: '定例レビューの曜日と開始時刻は？', any: [['木'], ['10:45', '10時45']] },
  { id: 'F5', say: '決済連携のベンダーはハルカペイ社に決定。', q: '決済連携のベンダーは？', any: [['ハルカペイ']] },
  { id: 'F6', say: 'シンガポール拠点への対応は第2期まで保留。', q: '第2期まで保留になっている対応は？', any: [['シンガポール']] },
  { id: 'F7', say: '旧システムの停止日は2027年4月30日。', q: '旧システムの停止日は？', any: [['4月30', '4/30', '04-30']] },
  { id: 'F8', say: '性能試験の同時接続目標は1,800件。', q: '性能試験の同時接続目標は？', any: [['1800']] },
  { id: 'F9', say: '障害時の一次連絡先は内線7342。', q: '障害時の一次連絡先の内線番号は？', any: [['7342']] },
  { id: 'F10', say: '全面クラウド移行案はコスト超過のため却下。', q: '却下された案は？', any: [['クラウド']] },
  { id: 'F11', say: '6月から追加要員を3名投入する。', q: '追加要員の人数と開始月は？', any: [['3名', '3人', '三名'], ['6月', '06-']] },
  { id: 'F12', say: '契約管理番号はTSUBAME-9。', q: '契約管理番号は？', any: [['tsubame-9']] },
  { id: 'F13', say: '値引き率の上限は8%と決めた。', q: '値引き率の上限は？', any: [['8%']] },
  { id: 'F14', say: '保守契約の更新期限は9月15日。', q: '保守契約の更新期限は？', any: [['9月15', '9/15', '09-15']] },
  { id: 'F15', say: '次回の合宿会場はB棟4階の402号室。', q: '次回の合宿会場は？', any: [['402']] },
  { id: 'F16', say: '外注先の稼働上限は月160時間。', q: '外注先の稼働上限は？', any: [['160']] },
];
const FILLERS = ['今日は朝から雨だった。', 'コーヒーを切らしている。', '会議室の空調が少し寒い。', '昼は近所の定食屋に行く予定。',
  '先週の連休は混んでいた。', 'プロジェクターの調子が悪い。', '新しい椅子が届いた。', '来月は出張が多い。', '資料の印刷が間に合った。', '窓の外に虹が出ている。'];

function buildScenario(rounds, factsN) {
  if (factsN > FACT_BANK.length) throw new Error(`facts max is ${FACT_BANK.length}`);
  const facts = FACT_BANK.slice(0, factsN);
  const perRound = Array.from({ length: rounds }, () => []);
  facts.forEach((f, j) => perRound[Math.floor(j * rounds / factsN)].push(f.say));
  const f2 = facts.find(f => f.id === 'F2');
  if (f2) {
    const f2round = Math.floor(facts.indexOf(f2) * rounds / factsN);
    const fixRound = Math.min(rounds - 1, Math.max(rounds - 2, f2round + 1));
    if (fixRound > f2round) perRound[fixRound].push(f2.fix); else delete f2.forbid;
  }
  // 1行目は見出しだけにする（目次は依頼の先頭1行を載せるため、事実が目次に漏れないように）。
  // 1行目は見出しだけにする（目次は依頼の先頭1行を載せるため、事実が目次に漏れないように）。
  const userTexts = perRound.map((s, i) =>
    `【定例${i + 1}回目】\n${s.join(' ')} ${FILLERS[i % FILLERS.length]}\n内容は覚えておいてください。返答は「了解」の一言だけで。`);
  return { facts, userTexts };
}

const norm = s => String(s ?? '').normalize('NFKC').replace(/[\s,，]/g, '').toLowerCase();
function scoreAnswers(facts, answers) {
  const wrong = []; let correct = 0;
  facts.forEach((f, i) => {
    const a = norm(answers[`Q${i + 1}`]);
    const hit = f.any.every(group => group.some(k => a.includes(norm(k))));
    const bad = (f.forbid ?? []).some(k => a.includes(norm(k)));
    if (hit && !bad) correct++; else wrong.push({ id: f.id, reason: !a ? 'empty' : bad ? 'forbidden(old value)' : 'mismatch' });
  });
  return { correct, total: facts.length, wrong };
}
function parseAnswers(text, n) {
  const out = {};
  const s = text.indexOf('{'), e = text.lastIndexOf('}');
  if (s >= 0 && e > s) {
    try { const j = JSON.parse(text.slice(s, e + 1)); for (const [k, v] of Object.entries(j)) out[k.toUpperCase()] = String(v); return { answers: out, format: 'json' }; } catch { /* fall through */ }
  }
  for (const line of text.split(/\r?\n/)) { const m = line.match(/^\W*(Q\d+)\s*[:：]\s*(.*)$/i); if (m) out[m[1].toUpperCase()] = m[2]; }
  return { answers: out, format: Object.keys(out).length ? 'lines' : 'unparsed' };
}
function questionPrompt(facts, mode) {
  const how = {
    memory: '会話の記憶だけで答えてください。ファイルの読み取りやコマンドは使わないでください。分からない項目は空文字にしてください。',
    material: '上に添付された資料の内容だけで答えてください。ファイルの読み取りやコマンドは使わないでください。資料に無い項目は空文字にしてください。',
    ledger: '上に添付された資料と、資料に書かれた会話台帳ファイルの両方を使って答えてください。資料だけでは分からない項目は、台帳ファイルを読み取って確認してから答えてください。',
  }[mode];
  return `定例会議のメモについて、次の各問に答えてください。${how}\n` +
    `出力はJSONオブジェクト1つだけ（キーは Q1, Q2, ... 、値は答えを1行で短く）。前置き・説明・コードフェンスは不要です。\n` +
    facts.map((f, i) => `Q${i + 1}: ${f.q}`).join('\n');
}

// ───────────── 出力先・CODEX_HOME照合 ─────────────
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const run = join(root, 'runs', `${stamp}-recall${TRIAL ? '-trial' : ''}`);
const work = join(run, 'work');
mkdirSync(work, { recursive: true });
const codexHome = process.env.CODEX_HOME || join(process.env.USERPROFILE, '.codex');
const watched = ['config.toml', 'auth.json'].map(f => join(codexHome, f));
const fingerprint = () => Object.fromEntries(watched.map(p => [p.split(/[\\/]/).pop(),
  existsSync(p) ? { sha256: createHash('sha256').update(readFileSync(p)).digest('hex'), mtimeMs: statSync(p).mtimeMs } : null]));
const before = fingerprint();

const env = { ...process.env };
for (const key of Object.keys(env)) if (/^(OPENAI_API_KEY|CODEX_API_KEY|CODEX_ACCESS_TOKEN)$/i.test(key)) delete env[key];

let version = null;
try { version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim(); } catch (e) { version = `unknown (${e.message.slice(0, 80)})`; }

// ───────────── JSON-RPC ─────────────
const sleep = ms => new Promise(r => setTimeout(r, ms));
const proc = spawn(exe, ['app-server', '--stdio'], { env, cwd: work, windowsHide: true });
let stderr = ''; let nextId = 1;
const pending = new Map(); const events = []; const approvals = [];
proc.stderr.on('data', b => { stderr = (stderr + b.toString()).slice(-4000); });
const send = m => proc.stdin.write(JSON.stringify(m) + '\n');
createInterface({ input: proc.stdout }).on('line', line => {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.id !== undefined && m.method === undefined && pending.has(m.id)) {
    const p = pending.get(m.id); pending.delete(m.id); clearTimeout(p.timer); p.resolve(m);
  } else if (m.id !== undefined && m.method !== undefined) {
    // 承認・質問などのserver要求は拒否して記録（代行しない）。
    approvals.push({ method: m.method, threadId: m.params?.threadId ?? null });
    send({ id: m.id, error: { code: -32000, message: 'recall-probe declines server request' } });
  } else if (m.method) events.push({ at: Date.now(), ...m });
});
const rpc = (method, params = {}, timeout = 30000) => new Promise((resolve, reject) => {
  const id = nextId++;
  const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timeout ${method}`)); }, timeout);
  pending.set(id, { resolve, timer }); send({ id, method, params });
});
const ok = async (method, params, timeout) => {
  const r = await rpc(method, params, timeout);
  if (r.error) throw new Error(`${method}: ${JSON.stringify(r.error).slice(0, 400)}`);
  return r.result;
};

// ───────────── thread操作 ─────────────
function usageOf(threadId) {
  const ups = events.filter(e => e.method === 'thread/tokenUsage/updated' && e.params?.threadId === threadId);
  const last = ups.at(-1)?.params?.tokenUsage;
  if (!last) return null;
  return { total: last.total, lastCall: last.last, peakContextTokens: Math.max(...ups.map(e => e.params.tokenUsage.last?.totalTokens ?? 0)),
    modelContextWindow: last.modelContextWindow ?? null, updates: ups.length };
}
async function startThread(extra = {}) {
  const r = await ok('thread/start', { cwd: work, ephemeral: true, sandbox: 'read-only', approvalPolicy: 'never', model: MODEL, ...extra });
  return { threadId: r.thread?.id, started: { model: r.model ?? null, reasoningEffort: r.reasoningEffort ?? null,
    sandbox: r.sandbox?.type ?? r.sandbox ?? null, approvalPolicy: r.approvalPolicy ?? null, ephemeral: r.thread?.ephemeral ?? null } };
}
async function waitFor(pred, timeout) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { const hit = events.find(pred); if (hit) return hit; await sleep(100); }
  return null;
}
async function runTurn(threadId, text, rec) {
  const from = events.length; const t0 = Date.now();
  const r = await ok('turn/start', { threadId, input: [{ type: 'text', text }] }, 60000);
  const turnId = r.turn?.id;
  const done = await waitFor(e => e.method === 'turn/completed' && e.params?.threadId === threadId && e.params?.turn?.id === turnId, TURN_TIMEOUT);
  if (!done) throw new Error(`turn not completed in ${TURN_TIMEOUT}ms`);
  await sleep(300); // 直後のtokenUsage通知を拾う
  const mine = events.slice(from).filter(e => e.params?.threadId === threadId);
  const items = mine.filter(e => e.method === 'item/completed').map(e => e.params.item);
  for (const it of items) rec.itemTypes[it.type] = (rec.itemTypes[it.type] ?? 0) + 1;
  for (const it of items.filter(i => i.type === 'commandExecution')) {
    rec.commands.push({ command: String(it.command ?? '').slice(0, 200), status: it.status ?? null, exitCode: it.exitCode ?? null,
      outputChars: String(it.aggregatedOutput ?? '').length });
  }
  const finalMsg = items.filter(i => i.type === 'agentMessage').at(-1)?.text ?? '';
  const status = done.params.turn.status;
  rec.turns.push({ status, ms: Date.now() - t0, replyChars: finalMsg.length, error: done.params.turn.error ? JSON.stringify(done.params.turn.error).slice(0, 200) : null });
  if (status !== 'completed') rec.errors.push(`turn ${rec.turns.length} status=${status}`);
  return finalMsg;
}
async function compact(threadId, rec) {
  const from = events.length; const t0 = Date.now();
  const entry = { requested: true, rpcAccepted: false, itemObserved: false, legacyNotificationObserved: false, turnCompleted: false, ms: null, error: null };
  rec.compactions.push(entry);
  try {
    await ok('thread/compact/start', { threadId }, 60000);
    entry.rpcAccepted = true;
    const isMine = e => e.params?.threadId === threadId;
    // 圧縮item・旧通知・turn完了のいずれかを待つ（圧縮が起きない場合も止まらないよう上限あり）。
    await waitFor(e => isMine(e) && ((e.method === 'item/completed' && e.params.item?.type === 'contextCompaction') || e.method === 'thread/compacted'), 120000);
    await sleep(1500);
    const mine = events.slice(from).filter(isMine);
    entry.itemObserved = mine.some(e => e.method === 'item/completed' && e.params.item?.type === 'contextCompaction');
    entry.legacyNotificationObserved = mine.some(e => e.method === 'thread/compacted');
    entry.turnCompleted = mine.some(e => e.method === 'turn/completed');
    entry.notificationMethods = [...new Set(mine.map(e => e.method))].sort();
  } catch (e) { entry.error = e.message.slice(0, 300); rec.errors.push(`compact: ${entry.error}`); }
  entry.ms = Date.now() - t0;
}

// ───────────── 方式の実行 ─────────────
const newRec = (method, label) => ({ method, label, status: 'pending', turns: [], compactions: [], commands: [], itemTypes: {}, errors: [], approvalRequests: 0 });
let scenario; let handoff = null;

function prepareHandoff() {
  const transcript = scenario.userTexts.map(u => ({ user: u, ai: '了解' }));
  const dir = join(run, 'handoff');
  mkdirSync(dir, { recursive: true });
  const tPath = join(dir, 'transcript.json');
  writeFileSync(tPath, JSON.stringify(transcript, null, 2));
  const ledgerPath = join(work, '会話台帳.md');
  const out = execFileSync('cargo', ['run', '-q', '--locked', '--example', 'handoff_material', '--', tPath, dir, ledgerPath, PURPOSE, String(HANDOFF_ROUNDS)],
    { cwd: join(repo, 'app', 'src-tauri'), encoding: 'utf8', maxBuffer: 1 << 24 });
  copyFileSync(join(dir, 'ledger.md'), ledgerPath);
  handoff = { envelope: readFileSync(join(dir, 'envelope.txt'), 'utf8'), meta: JSON.parse(out.trim().split(/\r?\n/).at(-1)), ledgerPath };
}

async function finish(rec, threadId, facts, text) {
  const { answers, format } = parseAnswers(text, facts.length);
  rec.answerFormat = format;
  rec.answers = answers; // 採点用の回答のみ
  rec.score = scoreAnswers(facts, answers);
  rec.usage = usageOf(threadId);
}
async function chatMethod(rec, withCompactPrompt) {
  const { facts, userTexts } = scenario;
  const compactAfter = new Set(Array.from({ length: COMPACTIONS }, (_, k) => Math.ceil(ROUNDS * (k + 1) / (COMPACTIONS + 1))));
  const extra = withCompactPrompt ? { config: { compact_prompt: COMPACT_PROMPT } } : {};
  const { threadId, started } = await startThread(extra);
  rec.thread = started;
  if (withCompactPrompt) rec.compactPrompt = { passedVia: 'thread/start config.compact_prompt', threadStartAccepted: true };
  for (let i = 0; i < userTexts.length; i++) {
    await runTurn(threadId, userTexts[i], rec);
    if (compactAfter.has(i + 1)) await compact(threadId, rec);
  }
  const text = await runTurn(threadId, questionPrompt(facts, 'memory'), rec);
  await finish(rec, threadId, facts, text);
}
async function handoffMethod(rec, mode) {
  const { facts } = scenario;
  const { threadId, started } = await startThread();
  rec.thread = started;
  rec.handoff = handoff.meta;
  const text = await runTurn(threadId, `${handoff.envelope}\n\n${questionPrompt(facts, mode)}`, rec);
  await finish(rec, threadId, facts, text);
  const readLedger = rec.commands.filter(c => /会話台帳|台帳|ledger/i.test(c.command));
  rec.ledger = { commandAttempts: rec.commands.length, ledgerReadAttempts: readLedger.length,
    ledgerReadSucceeded: readLedger.some(c => c.status === 'completed' && (c.exitCode === 0 || c.exitCode === null) && c.outputChars > 0) };
}

const COMPACT_PROMPT = [
  'あなたは業務会話の引き継ぎ要約を作る。これまでの会話を、後で正確に質問に答えられるよう要約する。',
  '次を必ず残すこと: 決定事項、数字（金額・件数・割合・時間・日付）、担当者と担当範囲、期限・日時、保留事項、却下された案、未解決の論点、識別子・番号・固有名詞。',
  '値が後から変更された場合は、最新の値を明記し、変更前の値は「旧」として併記する。',
  '雑談・挨拶・天気など業務に関係のない内容は省く。数字や固有名詞は一字も変えずに書き写す。',
].join('\n');

// ───────────── 本体 ─────────────
const out = { kind: 'recall-probe', version, codexHome: codexHome === join(process.env.USERPROFILE, '.codex') ? '%USERPROFILE%\\.codex (shared default, read-only use)' : 'custom',
  startedAt: new Date().toISOString(), params: { rounds: ROUNDS, facts: FACTS_N, compactions: COMPACTIONS, model: MODEL, handoffRounds: HANDOFF_ROUNDS, methods: METHODS, trial: TRIAL },
  results: [], approvalRequests: [], fatalError: null };
const t0 = Date.now();
const save = () => {
  out.approvalRequests = approvals;
  out.durationMs = Date.now() - t0;
  writeFileSync(join(run, 'result.json'), JSON.stringify(out, null, 2));
};

try {
  scenario = buildScenario(ROUNDS, FACTS_N);
  writeFileSync(join(run, 'scenario.json'), JSON.stringify({ userTexts: scenario.userTexts, facts: scenario.facts }, null, 2));
  await ok('initialize', { clientInfo: { name: 'agentdock_recall_probe', title: 'AgentDock Recall Probe', version: '0.1' }, capabilities: { experimentalApi: false } });
  send({ method: 'initialized', params: {} });
  if (TRIAL) {
    // 1往復＋最終質問の試行。トークン規模の見積り用。
    const rec = newRec('trial', '1往復の試行'); out.results.push(rec);
    const { threadId, started } = await startThread(); rec.thread = started;
    const one = buildScenario(1, Math.min(FACTS_N, 2));
    await runTurn(threadId, one.userTexts[0], rec);
    rec.usageAfterOneRound = usageOf(threadId);
    const text = await runTurn(threadId, questionPrompt(one.facts, 'memory'), rec);
    await finish(rec, threadId, one.facts, text);
    rec.status = 'done';
  } else {
    if (METHODS.some(m => m === 'b' || m === 'c')) prepareHandoff();
    const defs = { a: ['a', '同一thread＋圧縮任せ', r => chatMethod(r, false)], b: ['b', '引継ぎ資料（台帳は読まない）', r => handoffMethod(r, 'material')],
      c: ['c', '引継ぎ資料＋台帳を読む指示', r => handoffMethod(r, 'ledger')], d: ['d', '同一thread＋compact_prompt', r => chatMethod(r, true)] };
    for (const m of METHODS) {
      const [id, label, fn] = defs[m];
      const rec = newRec(id, label); out.results.push(rec);
      const a0 = approvals.length;
      try { await fn(rec); rec.status = rec.errors.length ? 'done-with-errors' : 'done'; }
      catch (e) { rec.status = 'failed'; rec.errors.push(e.message.slice(0, 400)); }
      rec.approvalRequests = approvals.length - a0;
      console.log(`${rec.status}: ${id} ${label} ${rec.score ? `${rec.score.correct}/${rec.score.total}` : ''}`);
      save();
    }
  }
} catch (e) {
  out.fatalError = e.message.slice(0, 500);
  console.log(`fatal: ${out.fatalError}`);
} finally {
  try { proc.stdin.end(); } catch { /* ignore */ }
  const killer = setTimeout(() => proc.kill(), 3000);
  await new Promise(r => { if (proc.exitCode !== null) r(); else proc.on('exit', r); }); clearTimeout(killer);
  const after = fingerprint();
  out.configUnchanged = before['config.toml']?.sha256 === after['config.toml']?.sha256;
  out.authUnchanged = before['auth.json']?.sha256 === after['auth.json']?.sha256;
  out.stderrTail = stderr.split(/\r?\n/).filter(l => !/token|bearer|authorization|@/i.test(l)).slice(-10);
  out.totalTokens = out.results.reduce((s, r) => s + (r.usage?.total?.totalTokens ?? 0), 0);
  save();
  const rows = out.results.filter(r => r.method !== 'trial').map(r => {
    const u = r.usage?.total;
    const comp = r.compactions.length ? `${r.compactions.filter(c => c.itemObserved || c.legacyNotificationObserved).length}/${r.compactions.length}観測` : '-';
    return `| ${r.method} ${r.label} | ${r.score ? `${r.score.correct}/${r.score.total}` : '-'} | ${r.score?.wrong.map(w => w.id).join(',') || '-'} | ${u ? u.totalTokens : '-'} | ${u ? u.inputTokens - u.cachedInputTokens + u.outputTokens : '-'} | ${comp} | ${r.approvalRequests} | ${r.status}${r.errors.length ? `: ${r.errors[0].slice(0, 80)}` : ''} |`;
  });
  const md = [`# 想起比較試験 ${stamp}`, '',
    `- codex: ${version}、モデル指定: ${MODEL}、受理: ${[...new Set(out.results.map(r => r.thread?.model).filter(Boolean))].join(',') || '-'}`,
    `- 往復 ${ROUNDS} / 事実 ${FACTS_N} / 圧縮 ${COMPACTIONS} / 引継ぎ直近 ${HANDOFF_ROUNDS}往復${TRIAL ? ' / 試行モード' : ''}`,
    `- config.toml 不変: ${out.configUnchanged} / auth.json 不変: ${out.authUnchanged} / 承認要求: ${approvals.length}件`,
    out.fatalError ? `- 致命エラー: ${out.fatalError}` : '', '',
    '| 方式 | 得点 | 誤答ID | 総トークン(thread累計) | 非キャッシュ入力+出力 | 圧縮 | 承認要求 | 状態 |', '|---|---|---|---|---|---|---|---|', ...rows, '',
    `合計トークン(thread累計の和): ${out.totalTokens}`].join('\n');
  writeFileSync(join(run, 'summary.md'), md);
  console.log(JSON.stringify({ run, configUnchanged: out.configUnchanged, authUnchanged: out.authUnchanged, totalTokens: out.totalTokens }));
}

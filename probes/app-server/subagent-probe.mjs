// 親Codex＋Codexネイティブのサブエージェントの挙動を実Codexで測る試験。
//  free : 制約なしで「子A/Bが同じ shared.txt を別の行で同時に編集」と依頼
//  single: 製品（host/subagents.rs の parent_prompt）と同じ指示文（同時に編集するのは1体まで）を付けて同じ作業を依頼
// 見るもの: 子threadの数・cwd、子ごとの fileChange/commandExecution の時刻と対象、編集区間の重なり、最終ファイル状態、元repoの汚れ。
// 親threadは ephemeral・workspace-write（worktree内）・approvalPolicy never。server requestは拒否して記録する（代行しない）。
// CODEX_HOMEの config.toml/auth.json は書かず、前後でhash照合。モデルの自由文本文は保存しない。外部パッケージなし。
import { spawn, execFileSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { join, dirname, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const flag = (name, def) => { const hit = argv.find(a => a.startsWith(`--${name}=`)); return hit ? hit.slice(name.length + 3) : def; };
const exe = argv.filter(a => !a.startsWith('--'))[0] || 'C:\\Users\\wmasa\\AppData\\Local\\Programs\\OpenAI\\Codex\\bin\\codex.exe';
const MODEL = flag('model', 'gpt-6.1-sol');
const VARIANT = flag('variant', 'both');
const BUDGET = Number(flag('budget-uncached', 150000)); // 非キャッシュ（入力の非キャッシュ分＋出力）の全threadの累計上限
const TURN_TIMEOUT = Number(flag('turn-timeout-ms', 300000));
const VARIANTS = VARIANT === 'both' ? ['free', 'single'] : [VARIANT];

const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const run = join(root, 'runs', `${stamp}-subagent`);
mkdirSync(run, { recursive: true });
const codexHome = process.env.CODEX_HOME || join(process.env.USERPROFILE, '.codex');
const watched = ['config.toml', 'auth.json'].map(f => join(codexHome, f));
const fingerprint = () => Object.fromEntries(watched.map(p => [p.split(/[\\/]/).pop(), existsSync(p) ? createHash('sha256').update(readFileSync(p)).digest('hex') : null]));
const before = fingerprint();
const env = { ...process.env };
for (const key of Object.keys(env)) if (/^(OPENAI_API_KEY|CODEX_API_KEY|CODEX_ACCESS_TOKEN)$/i.test(key)) delete env[key];
let version; try { version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim(); } catch (e) { version = `unknown (${e.message.slice(0, 80)})`; }

const git = (cwd, ...args) => execFileSync('git', ['-c', 'user.name=probe', '-c', 'user.email=probe@example.invalid', ...args], { cwd, encoding: 'utf8' }).trim();
const norm = p => String(p ?? '').replace(/\\/g, '/').replace(/^\/\/\?\//, '').replace(/\/+$/, '').toLowerCase();

// ───────────── JSON-RPC ─────────────
const sleep = ms => new Promise(r => setTimeout(r, ms));
const proc = spawn(exe, ['app-server', '--stdio'], { env, cwd: run, windowsHide: true });
let stderr = ''; let nextId = 1;
const pending = new Map(); const events = []; const serverRequests = [];
proc.stderr.on('data', b => { stderr = (stderr + b.toString()).slice(-4000); });
const send = m => proc.stdin.write(JSON.stringify(m) + '\n');
createInterface({ input: proc.stdout }).on('line', line => {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.id !== undefined && m.method === undefined && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); clearTimeout(p.timer); p.resolve(m); }
  else if (m.id !== undefined && m.method !== undefined) {
    serverRequests.push({ method: m.method, threadId: m.params?.threadId ?? null });
    send({ id: m.id, error: { code: -32000, message: 'subagent-probe declines server request' } });
  } else if (m.method) events.push({ at: Date.now(), ...m });
});
const rpc = (method, params = {}, timeout = 30000) => new Promise((resolve, reject) => {
  const id = nextId++; const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timeout ${method}`)); }, timeout);
  pending.set(id, { resolve, timer }); send({ id, method, params });
});
const ok = async (method, params, timeout) => { const r = await rpc(method, params, timeout); if (r.error) throw new Error(`${method}: ${JSON.stringify(r.error).slice(0, 400)}`); return r.result; };

// ───────────── 指示文 ─────────────
const TASK = 'サブエージェントを2体同時に起動してください。子Aは a.txt を新規作成（内容は1行 "A"）し、shared.txt の1行目を "EDITED-BY-A" に書き換える。子Bは b.txt を新規作成（内容は1行 "B"）し、shared.txt の2行目を "EDITED-BY-B" に書き換える。終わったら結果を短くまとめてください。';
// 製品の host/subagents.rs parent_prompt と同じ文面（request に TASK を入れる）。
const parentPrompt = (request, worktree, base) => `AgentDockの親Codex + nativeサブエージェント依頼です。あなたが親として計画・委任・差し戻し・最終報告を担当してください。Codexが提供するnativeサブエージェント機能を使い、適切な独立作業を実際に委任してください。AgentDockの旧組織ロール実行ではありません。native機能が利用できない場合はその制約を明示し、使ったと装わないでください。\n作業場所: ${worktree}\n開始コミット: ${base}\n調査は並列可、編集担当は同時に1体、検証は編集停止後に行ってください。これらは親への指示でありアプリによる排他制御・子別sandbox保証ではありません。子へ作業場所と編集範囲を明示し、子作成で専用worktreeが自動作成されると仮定しないでください。適用されるAGENTS.mdと利用者のモデル制約を子にも伝えてください。\n通常の追加指示はこの親会話に届きます。承認・質問は通常のホスト経路を使ってください。元の作業フォルダへ取り込み・push・破壊的な操作を勝手に行わないでください。成果を確認可能にするため、適切ならこの専用worktreeで変更をコミットしてください。コミットできない理由や未コミットの残りは報告してください。最終報告には変更・検証結果・制約・残作業を含め、子孫が停止済みか確認してください。親の申告だけでアプリの停止・検証成功・取り込み成功が確定することはありません。\n\n利用者の依頼:\n${request}`;

// ───────────── 集計 ─────────────
const uncachedOf = u => (u ? u.inputTokens - u.cachedInputTokens + u.outputTokens : 0);
function usageByThread(from) {
  const last = {};
  for (const e of events.slice(from)) if (e.method === 'thread/tokenUsage/updated') last[e.params.threadId] = e.params.tokenUsage?.total;
  return last;
}
const sumUsage = byThread => {
  const s = { totalTokens: 0, inputTokens: 0, cachedInputTokens: 0, outputTokens: 0, uncached: 0 };
  for (const u of Object.values(byThread)) if (u) { s.totalTokens += u.totalTokens; s.inputTokens += u.inputTokens; s.cachedInputTokens += u.cachedInputTokens; s.outputTokens += u.outputTokens; s.uncached += uncachedOf(u); }
  return s;
};

async function runVariant(variant) {
  const dir = join(run, variant); const repo = join(dir, 'repo'); const wt = join(dir, 'wt');
  mkdirSync(repo, { recursive: true });
  git(repo, 'init', '-q', '-b', 'main');
  writeFileSync(join(repo, 'shared.txt'), 'line1 original\nline2 original\nline3 original\n');
  writeFileSync(join(repo, 'README.md'), 'probe\n');
  git(repo, 'add', '-A'); git(repo, 'commit', '-q', '-m', 'init');
  const base = git(repo, 'rev-parse', 'HEAD');
  git(repo, 'worktree', 'add', '-q', '-b', 'probe-wt', wt);
  const rec = { variant, base: base.slice(0, 10), parentThread: null, errors: [], turn: null, children: {}, items: [], spawnCalls: [], eventMethods: {}, serverRequests: 0 };
  const from = events.length; const sr0 = serverRequests.length;
  const started = await ok('thread/start', { cwd: wt, ephemeral: true, sandbox: 'workspace-write', approvalPolicy: 'never', model: MODEL });
  const parentId = started.thread?.id; rec.parentThread = parentId;
  rec.thread = { model: started.model ?? null, sandbox: started.sandbox?.type ?? started.sandbox ?? null, approvalPolicy: started.approvalPolicy ?? null, cwdEqualsWorktree: norm(started.cwd ?? started.thread?.cwd) === norm(wt) };
  const text = variant === 'single' ? parentPrompt(TASK, wt, base) : TASK;
  const t0 = Date.now();
  const r = await ok('turn/start', { threadId: parentId, input: [{ type: 'text', text }] }, 60000);
  const turnId = r.turn?.id;
  // 完了待ち。非キャッシュ予算を超えたら中断する（自動再送はしない）。
  let done = null;
  while (Date.now() - t0 < TURN_TIMEOUT) {
    done = events.slice(from).find(e => e.method === 'turn/completed' && e.params?.threadId === parentId && e.params?.turn?.id === turnId);
    if (done) break;
    if (sumUsage(usageByThread(from)).uncached > BUDGET) { rec.errors.push(`budget exceeded (>${BUDGET})`); try { await ok('turn/interrupt', { threadId: parentId, turnId }, 10000); } catch { /* ignore */ } break; }
    await sleep(200);
  }
  if (!done && !rec.errors.length) rec.errors.push(`turn not completed in ${TURN_TIMEOUT}ms`);
  await sleep(done ? 1500 : 3000); // 直後の通知（tokenUsage・子の終端）を拾う
  rec.turn = { status: done?.params?.turn?.status ?? 'not-completed', ms: Date.now() - t0 };
  const mine = events.slice(from);
  for (const e of mine) rec.eventMethods[e.method] = (rec.eventMethods[e.method] ?? 0) + 1;
  rec.serverRequests = serverRequests.length - sr0;

  // 子thread: thread/started で現れた親以外、または親以外のthreadIdを持つitem通知。
  const startedThreads = mine.filter(e => e.method === 'thread/started').map(e => e.params?.thread).filter(Boolean);
  const childIds = new Set(startedThreads.map(t => t.id).filter(id => id !== parentId));
  for (const e of mine) if (e.method?.startsWith('item/') && e.params?.threadId && e.params.threadId !== parentId) childIds.add(e.params.threadId);
  for (const id of childIds) {
    const t = startedThreads.find(x => x.id === id);
    rec.children[id] = { startedEvent: !!t, parentThreadId: t?.parentThreadId ?? t?.source?.subAgent?.thread_spawn?.parent_thread_id ?? null,
      sourceKind: t ? (typeof t.source === 'string' ? t.source : Object.keys(t.source ?? {})[0] ?? null) : null,
      role: t?.agentRole ?? null, nickname: t?.agentNickname ?? null, threadCwd: t?.cwd ?? null,
      cwdSameAsParent: t?.cwd ? norm(t.cwd) === norm(wt) : null, commandCwds: [] };
  }
  // item: started/completed の時刻を対応付ける。
  const startAt = {};
  for (const e of mine) {
    if (!e.method?.startsWith('item/') || !e.params?.item) continue;
    const it = e.params.item; const tid = e.params.threadId; const key = `${tid}:${it.id}`;
    if (e.method === 'item/started') startAt[key] = e.at;
    if (e.method !== 'item/completed') continue;
    if (it.type === 'collabAgentToolCall' || it.type === 'subAgentActivity') {
      rec.spawnCalls.push({ atMs: e.at - t0, thread: tid === parentId ? 'parent' : tid, type: it.type, tool: it.tool ?? null, kind: it.kind ?? null, status: it.status ?? null,
        receivers: it.receiverThreadIds ?? (it.agentThreadId ? [it.agentThreadId] : []), agentPath: it.agentPath ?? null,
        agentStates: it.agentsStates ? Object.fromEntries(Object.entries(it.agentsStates).map(([k, v]) => [k, v?.status ?? null])) : null });
    }
    if (it.type === 'fileChange' || it.type === 'commandExecution') {
      const paths = it.type === 'fileChange' ? (it.changes ?? []).map(c => ({ path: isAbsolute(c.path) ? relative(wt, c.path).replace(/\\/g, '/') : String(c.path).replace(/\\/g, '/'), kind: c.kind?.type ?? c.kind ?? null })) : [];
      const cmd = it.type === 'commandExecution' ? String(it.command ?? '') : '';
      const row = { thread: tid === parentId ? 'parent' : tid, type: it.type, status: it.status ?? null, startMs: (startAt[key] ?? e.at) - t0, endMs: e.at - t0, paths,
        command: cmd ? cmd.slice(0, 160) : undefined, touches: cmd ? ['a.txt', 'b.txt', 'shared.txt'].filter(f => cmd.includes(f)) : undefined,
        writesLike: cmd ? /Set-Content|Out-File|Add-Content|>|sed |Write|echo|New-Item|-replace|WriteAll|Move-Item|Copy-Item/i.test(cmd) : undefined, usesLock: cmd ? /Mutex|lock/i.test(cmd) : undefined,
        cwdSameAsParent: it.cwd ? norm(it.cwd) === norm(wt) : undefined };
      rec.items.push(row);
      if (it.cwd && rec.children[tid] && !rec.children[tid].commandCwds.includes(it.cwd)) rec.children[tid].commandCwds.push(it.cwd);
    }
  }
  // 編集区間: 子ごとの、fileChange または a/b/shared を触る書き込み風commandExecution の最初のstart〜最後のend。
  const isEdit = it => it.type === 'fileChange' || (it.touches?.length && it.writesLike);
  const editsBy = {};
  for (const it of rec.items.filter(isEdit)) (editsBy[it.thread] ??= []).push(it);
  rec.editIntervals = Object.fromEntries(Object.entries(editsBy).map(([k, v]) => [k, { items: v.length, startMs: Math.min(...v.map(i => i.startMs)), endMs: Math.max(...v.map(i => i.endMs)) }]));
  const ks = Object.keys(rec.editIntervals).filter(k => k !== 'parent');
  const overlap = (a, b) => a.startMs < b.endMs && b.startMs < a.endMs;
  rec.editOverlap = { childLevel: false, itemLevel: false, pairs: [] };
  for (let i = 0; i < ks.length; i++) for (let j = i + 1; j < ks.length; j++) {
    const c = overlap(rec.editIntervals[ks[i]], rec.editIntervals[ks[j]]);
    const it = editsBy[ks[i]].some(x => editsBy[ks[j]].some(y => overlap(x, y)));
    rec.editOverlap.pairs.push({ a: ks[i], b: ks[j], childLevel: c, itemLevel: it });
    if (c) rec.editOverlap.childLevel = true;
    if (it) rec.editOverlap.itemLevel = true;
  }
  rec.editorThreads = Object.keys(editsBy);
  rec.childFinal = {};
  for (const id of childIds) {
    const st = mine.filter(e => e.params?.threadId === id && e.method === 'thread/status/changed').at(-1);
    const tc = mine.filter(e => e.params?.threadId === id && e.method === 'turn/completed').at(-1);
    rec.childFinal[id] = { lastStatus: st?.params?.status?.type ?? null, turnStatus: tc?.params?.turn?.status ?? null, turnCompletedBeforeParent: tc && done ? tc.at <= done.at : null };
  }
  // 最終ファイル状態
  const lines = f => { const p = join(wt, f); return existsSync(p) ? readFileSync(p, 'utf8').split(/\r?\n/).filter((l, i, a) => !(i === a.length - 1 && l === '')) : null; };
  const a = lines('a.txt'), b = lines('b.txt'), s = lines('shared.txt');
  rec.files = { 'a.txt': a ? { lines: a.length } : null, 'b.txt': b ? { lines: b.length } : null,
    'shared.txt': s ? { lines: s.length, line1EditedByA: s[0] === 'EDITED-BY-A', line2EditedByB: s[1] === 'EDITED-BY-B', line3Untouched: s[2] === 'line3 original' } : null };
  rec.bothChangesKept = !!(s && s[0] === 'EDITED-BY-A' && s[1] === 'EDITED-BY-B');
  const st = git(repo, 'status', '--porcelain');
  rec.originalRepo = { statusPorcelain: st, clean: st === '', headUnchanged: git(repo, 'rev-parse', 'main') === base };
  rec.worktree = { changedEntries: git(wt, 'status', '--porcelain').split('\n').filter(Boolean).length, headMoved: git(wt, 'rev-parse', 'HEAD') !== base };
  const childItemCwd = rec.items.filter(i => i.thread !== 'parent' && i.cwdSameAsParent !== undefined);
  rec.cwdShared = { fromThreadStarted: [...new Set(Object.values(rec.children).map(c => c.cwdSameAsParent))], fromChildCommands: childItemCwd.length ? [...new Set(childItemCwd.map(i => i.cwdSameAsParent))] : null };
  const ub = usageByThread(from);
  rec.usage = { perThread: Object.fromEntries(Object.entries(ub).map(([k, v]) => [k === parentId ? 'parent' : k, v ? { totalTokens: v.totalTokens, uncached: uncachedOf(v) } : null])), sum: sumUsage(ub) };
  rec.childCount = childIds.size;
  return rec;
}

// ───────────── 実行 ─────────────
const out = { codex: version, model: MODEL, stamp, variants: [], budgetUncached: BUDGET };
const save = () => writeFileSync(join(run, 'result.json'), JSON.stringify(out, null, 2));
try {
  await ok('initialize', { clientInfo: { name: 'agentdock_subagent_probe', title: 'AgentDock Subagent Probe', version: '0.1' }, capabilities: { experimentalApi: false } });
  send({ method: 'initialized', params: {} });
  let spent = 0;
  for (const v of VARIANTS) {
    if (spent > BUDGET) { out.variants.push({ variant: v, skipped: 'budget' }); continue; }
    try {
      const rec = await runVariant(v); out.variants.push(rec); spent += rec.usage.sum.uncached;
      console.log(`${v}: children=${rec.childCount} overlap=${rec.editOverlap.itemLevel} kept=${rec.bothChangesKept} uncached=${rec.usage.sum.uncached}`);
    } catch (e) { out.variants.push({ variant: v, fatal: e.message.slice(0, 400) }); console.log(`${v} failed: ${e.message.slice(0, 200)}`); }
    save();
  }
} catch (e) { out.fatalError = e.message.slice(0, 500); console.log(`fatal: ${out.fatalError}`); }
finally {
  try { proc.stdin.end(); } catch { /* ignore */ }
  const killer = setTimeout(() => proc.kill(), 3000);
  await new Promise(r => { if (proc.exitCode !== null) r(); else proc.on('exit', r); }); clearTimeout(killer);
  const after = fingerprint();
  out.configUnchanged = before['config.toml'] === after['config.toml']; out.authUnchanged = before['auth.json'] === after['auth.json'];
  out.serverRequestCount = serverRequests.length;
  out.stderrTail = stderr.split(/\r?\n/).filter(l => !/token|bearer|authorization|@/i.test(l)).slice(-8);
  save();
  const done = out.variants.filter(v => v.items);
  const rows = done.map(v => {
    const ov = v.editOverlap; const sh = v.files['shared.txt'];
    return `| ${v.variant} | ${v.turn.status} (${Math.round(v.turn.ms / 1000)}s) | ${v.childCount} | ${v.editorThreads.map(t => t === 'parent' ? '親' : '子').join('/') || '-'} | ${ov.childLevel ? '重なる' : 'なし'} / ${ov.itemLevel ? '重なる' : 'なし'} | ${sh ? `${sh.lines}行 A=${sh.line1EditedByA} B=${sh.line2EditedByB}` : '無し'} | a:${v.files['a.txt']?.lines ?? '無'} b:${v.files['b.txt']?.lines ?? '無'} | ${v.originalRepo.clean ? 'clean' : '汚れ'} | ${JSON.stringify(v.cwdShared.fromThreadStarted)} / ${JSON.stringify(v.cwdShared.fromChildCommands)} | ${v.serverRequests} | ${v.usage.sum.totalTokens} / ${v.usage.sum.uncached} |`;
  });
  const detail = done.flatMap(v => ['', `## ${v.variant}: 編集系item（ms は親turn開始から）`, '', '| 担当 | 種別 | 開始 | 終了 | 対象 | 状態 |', '|---|---|---|---|---|---|',
    ...v.items.map(i => `| ${i.thread === 'parent' ? '親' : i.thread.slice(-6)} | ${i.type} | ${i.startMs} | ${i.endMs} | ${i.paths.map(p => `${p.path}(${p.kind})`).join(',') || (i.touches ?? []).join(',') || '-'} | ${i.status} |`),
    '', `イベント種別: ${JSON.stringify(v.eventMethods)}`, `spawn系item: ${JSON.stringify(v.spawnCalls.map(s => ({ ...s, thread: String(s.thread).slice(-6) })))}`,
    `子: ${JSON.stringify(Object.fromEntries(Object.entries(v.children).map(([k, c]) => [k.slice(-6), { ...c, commandCwds: c.commandCwds.length }])))}`, `子の終端: ${JSON.stringify(v.childFinal)}`, `errors: ${JSON.stringify(v.errors)}`]);
  const other = out.variants.filter(v => !v.items).map(v => `- ${v.variant}: ${v.fatal ?? v.skipped}`);
  writeFileSync(join(run, 'summary.md'), [`# サブエージェント試験 ${stamp}`, '', `- codex: ${version} / モデル: ${MODEL} / config.toml不変: ${out.configUnchanged} / auth.json不変: ${out.authUnchanged} / server request: ${out.serverRequestCount}件`,
    out.fatalError ? `- 致命エラー: ${out.fatalError}` : '', ...other, '',
    '| 条件 | turn | 子の数 | 編集した担当 | 編集区間の重なり(子/item) | shared.txt | a/b.txt | 元repo | cwd共有(thread/コマンド) | server req | 使用量(総/非キャッシュ) |', '|---|---|---|---|---|---|---|---|---|---|---|', ...rows, ...detail].join('\n'));
  console.log(JSON.stringify({ run, configUnchanged: out.configUnchanged, authUnchanged: out.authUnchanged }));
}

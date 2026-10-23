// 組織実行の開始前の依頼整理の画面（DESIGN_ORG_INTAKE §1）。
// 依頼整理のチャットは普通のチャット。ここは、そのチャットの入力欄の上に出る帯と要約カード、開始確認の本文。
// 規則: 会話中は組織実行を始めない。「開始」はその確認ダイアログの主操作だけ。「要約を作る」は押した回数だけ1回送る（再送しない）。
//       要約の結果を確認できないときは「確認できません」と書き、出す操作は「履歴から読み直す」（読取りのみ）と、利用者が押す「もう一度要約を作る」だけ。
//       開始後の会話はワークフローへ何も伝わらない（帯に書く）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { ChatKey, ChatLocalView, IntakeBeginMode, IntakeState, IntakeView, OrgGraphsView } from "../ipc/types";
import { FootActions } from "./DialogParts";
import { setNoteDraft } from "./orgNoteDraft";
import { estimateLines } from "./org";
import { formOf, formProblem, prefsOf, sameForm, summaryOf, type SummaryForm } from "./intakeForm";
import { GitStatusRow, needsPrep } from "./GitDialogs";
import type { RepoStatusView } from "../ipc/types";
import { OwnerAllowPanel } from "./OwnerAllowPanel";
import { blockedOwnerRepo } from "./ownerAllow";

const errText = (e: unknown) => host.asIpcError(e).message;
const STANDARD_ID = "standard";

/** ほかの場所（App）へダイアログを開くよう頼む。深い入力欄から props を引き回さないための合図。 */
export function openDialogEvent(detail: { type: "orgIntakeStart"; chatId: string } | { type: "gitPrep"; folder: string }) {
  window.dispatchEvent(new CustomEvent("agentdock:dialog", { detail }));
}

/** 依頼整理の状態が画面に出るもの（やめたもの・記録がないものは出さない）。 */
export function intakeVisible(local: ChatLocalView | undefined): IntakeState | null {
  const s = local?.conversation?.intake ?? null;
  return s && s.kind !== "abandoned" ? s : null;
}

// ───────────────────────────── 開始ダイアログ（簡略） ─────────────────────────────

/** 組織実行の開始ダイアログの本文。作業フォルダを決め、「依頼を整理する」か「書く」へ進む。ここでは組織実行を始めない。 */
export function OrgIntakeBeginBody({ begin, browse, live, initialFolder, onPrepare }: {
  begin: (folder: string, mode: IntakeBeginMode) => Promise<string | null>;
  browse: () => Promise<string | null>;
  live: boolean;
  initialFolder: string;
  onPrepare: (folder: string) => void;
}) {
  const [folder, setFolder] = useState(initialFolder);
  const [status, setStatus] = useState<RepoStatusView | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const can = folder.trim() !== "" && live && !busy;
  const go = async (mode: IntakeBeginMode) => {
    if (!can) return;
    setBusy(true); setErr(null);
    const e = await begin(folder.trim(), mode);
    setBusy(false);
    if (e) setErr(e);
  };
  return (
    <div className="content">
      <div className="field"><span>作業フォルダ</span>
        <div style={{ display: "flex", gap: 4 }}>
          <input type="text" className="mono grow" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="仕事のフォルダのフルパス" aria-label="作業フォルダのパス" autoFocus />
          <button type="button" className="btn sm" onClick={() => void browse().then((p) => { if (p) setFolder(p); })}>参照…</button>
        </div>
        <span className="note">Worker はこの中の専用 worktree で作業し、あなたのブランチ・作業ツリーには書き込みません。Git のリポジトリ（コミットが1つ以上）が必要です。</span>
        {live ? <GitStatusRow folder={folder} tick={0} onPrepare={() => onPrepare(folder.trim())} onStatus={setStatus} /> : null}
      </div>
      <div className="banner info" role="note"><div>
        まず、普通のチャットで AI と依頼を詰めます（<b>組織実行はまだ始まりません</b>）。固まったら「要約を作る」で依頼の形にまとめ、内容を確認してから開始します。
        テンプレート・並列数・turn 上限は、その要約の画面で決めます。AgentDock は自動でマージしません。
      </div></div>
      {needsPrep(status) ? <p className="small muted">Git の準備が要ります。「Gitを準備…」で整えるか、準備してから開始してください（依頼の整理だけなら今すぐできます）。</p> : null}
      {err ? <div className="why err" role="alert">{err}</div> : null}
      <FootActions
        left={<button className="btn" disabled={!can} title="依頼の内容が決まっているときは、会話せずに要約を直接書きます" onClick={() => void go("write")}>依頼が決まっているので書く</button>}
        main={<button className="btn primary" disabled={!can} onClick={() => void go("converse")}>{busy ? "作成しています…" : "依頼を整理する"}</button>} />
    </div>
  );
}

// ───────────────────────────── 帯と要約カード ─────────────────────────────

function useIntake(chat: ChatKey, stateKind: string | null): { view: IntakeView | null; err: string | null; reload: () => void; set: (v: IntakeView) => void } {
  const [view, setView] = useState<IntakeView | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const id = chat.id;
  useEffect(() => {
    if (stateKind === null) { setView(null); return; }
    let stale = false;
    host.orgIntakeView(chat).then((v) => { if (!stale) { setView(v); setErr(null); } }).catch((e) => { if (!stale) setErr(errText(e)); });
    // 要約の結果を待っている間は、ときどき読み直す（読取りのみ）。
    const t = stateKind === "summarizing" ? setInterval(() => { host.orgIntakeView(chat).then((v) => { if (!stale) setView(v); }).catch(() => {}); }, 4000) : null;
    return () => { stale = true; if (t) clearInterval(t); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, stateKind, tick]);
  return { view, err, reload: () => setTick((n) => n + 1), set: setView };
}

/** 入力欄の上に出す、依頼整理の帯と要約カード。依頼整理でないチャットでは何も出さない。 */
export function IntakeSection({ chat, local, live }: { chat: ChatKey; local: ChatLocalView | undefined; live: boolean }) {
  const state = intakeVisible(local);
  const { view, err, reload, set } = useIntake(chat, live && state ? state.kind : null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [manual, setManual] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteText, setNoteText] = useState("");
  if (!live || !state) return null;
  const run = async (f: () => Promise<IntakeView | void>) => {
    setBusy(true); setMsg(null);
    try { const v = await f(); if (v) set(v); reload(); }
    catch (e) { setMsg(errText(e)); }
    setBusy(false);
  };
  const kind = view?.state.kind ?? state.kind;
  const showCard = view && (kind === "ready" || kind === "started" || (kind === "drafting" && (manual || view.writeOnly || view.summary !== null)));
  return (
    <div className="intake" role="region" aria-label="組織実行の依頼整理">
      {err ? <div className="why err" role="alert">依頼整理の状態を取得できませんでした（{err}）。</div> : null}
      {view && kind === "drafting" ? (
        <div className="lockmsg">
          {view.writeOnly
            ? <>依頼を書いています。ここは書く専用で、実行は始まりません（組織実行はまだ始まっていません）。</>
            : <>依頼を整理しています。ここは相談専用です。実行は始まりません（組織実行はまだ始まっていません）。</>}
          {view.lastError ? <span className="why err"> {view.lastError}</span> : null}
          {" "}
          {view.writeOnly ? null : <>
            {view.structuredOutput ? <button className="small" disabled={busy} onClick={() => void run(async () => { await host.orgIntakeSummarize(chat); })}>要約を作る</button> : <span className="muted">（このAIは構造化出力に対応していないため、要約は手で書きます）</span>}
            {" "}<button className="small" disabled={busy} onClick={() => setManual(true)}>手で書く</button>
          </>}
          {" "}<button className="small" disabled={busy} onClick={() => setAbandoning(true)}>やめる…</button>
        </div>
      ) : null}
      {view && kind === "summarizing" ? (
        <div className="lockmsg" role="status">
          {view.summaryUnconfirmed
            ? <>要約の結果を確認できません。<button className="small" disabled={busy} onClick={() => void run(() => host.orgIntakeReloadSummary(chat))}>履歴から読み直す</button>{" "}
                <button className="small" disabled={busy} title="新しく要約の依頼を送ります（押したときだけ送ります）" onClick={() => void run(async () => { await host.orgIntakeSummarize(chat); })}>もう一度要約を作る</button></>
            : <>要約を作っています…（終わると自動で取り込みます）{" "}
                <button className="small" disabled={busy} onClick={() => void run(() => host.orgIntakeReloadSummary(chat))}>履歴から読み直す</button></>}
          {" "}<button className="small" disabled={busy} onClick={() => setAbandoning(true)}>やめる…</button>
        </div>
      ) : null}
      {view && kind === "started" ? (
        <div className="lockmsg" role="status">
          この依頼で組織実行を開始しました。このチャットは普通に続けられますが、<b>ワークフローへは何も伝わりません</b>。
          {" "}<button className="small" onClick={() => window.dispatchEvent(new Event("agentdock:open-org"))}>ワークフローを開く</button>
          {" "}<button className="small" aria-expanded={noteOpen} onClick={() => setNoteOpen((o) => !o)} title="ワークフローへ追加指示を書きます（本文を組織画面の入力欄へ写すだけで、自動では送りません）">ワークフローへ追加指示…</button>
          {noteOpen ? (
            <div>
              <textarea rows={2} style={{ width: "100%" }} value={noteText} aria-label="ワークフローへの追加指示の内容" onChange={(e) => setNoteText(e.target.value)} />
              <button className="small" disabled={noteText.trim() === ""} onClick={() => { setNoteDraft(noteText.trim()); setNoteText(""); setNoteOpen(false); window.dispatchEvent(new Event("agentdock:open-org")); }}>組織画面の入力欄へ写す（送りません）</button>
            </div>
          ) : null}
        </div>
      ) : null}
      {abandoning ? (
        <div className="lockmsg" role="alertdialog" aria-label="依頼整理をやめる確認">
          依頼整理をやめます（チャットは消えません。組織実行は始まりません）。{" "}
          <button className="small" disabled={busy} onClick={() => void run(async () => { const v = await host.orgIntakeAbandon(chat); setAbandoning(false); return v; })}>やめる</button>{" "}
          <button className="small" onClick={() => setAbandoning(false)}>戻る</button>
        </div>
      ) : null}
      {msg ? <div className="why err" role="alert">{msg}</div> : null}
      {showCard && view ? <SummaryCard key={`${view.chat.id}-${view.rev}-${view.state.kind}`} chat={chat} view={view} busy={busy} run={run} /> : null}
    </div>
  );
}

function SummaryCard({ chat, view, busy, run }: { chat: ChatKey; view: IntakeView; busy: boolean; run: (f: () => Promise<IntakeView | void>) => Promise<void> }) {
  const base = useMemo(() => formOf(view.summary, view.prefs), [view.summary, view.prefs]);
  const [f, setF] = useState<SummaryForm>(base);
  const [graphs, setGraphs] = useState<OrgGraphsView | null>(null);
  useEffect(() => {
    let stale = false;
    host.orgGraphsView().then(async (v) => (v.loaded ? v : host.orgReloadGraphs())).then((v) => { if (!stale) setGraphs(v); }).catch(() => {});
    return () => { stale = true; };
  }, []);
  const dirty = !sameForm(f, base);
  const turnLimit = graphs?.maxTurnsLimit ?? 200;
  const turnDefault = graphs?.defaultMaxTurns ?? 30;
  const problem = formProblem(f, turnLimit);
  const started = view.state.kind === "started";
  const set = (patch: Partial<SummaryForm>) => setF((x) => ({ ...x, ...patch }));
  const templates = (graphs?.templates ?? []).filter((t) => t.id !== STANDARD_ID);
  const chosen = f.graphId === "" ? (graphs?.templates ?? []).find((t) => t.id === STANDARD_ID) ?? null : templates.find((t) => t.id === f.graphId) ?? null;
  const effTurns = Number(f.maxTurns.trim() || turnDefault) || turnDefault;
  const est = graphs ? estimateLines(chosen?.estimate ?? null, graphs.tokensPerTurnHint, effTurns) : null;
  const save = () => run(() => host.orgIntakeSave({ chat, baseRev: view.rev, summary: summaryOf(f), prefs: prefsOf(f, view.prefs.folder, turnLimit) }));
  const checkOf = (path: string) => view.inputChecks.find((c) => c.path === path);
  const blockers = dirty ? [] : view.blockers;
  const setInput = (i: number, patch: Partial<SummaryForm["inputs"][number]>) => set({ inputs: f.inputs.map((x, j) => (j === i ? { ...x, ...patch } : x)) });
  return (
    <div className="intake-card" role="group" aria-label="依頼の要約">
      <div className="small"><b>依頼の要約</b>（第{view.rev}版）{started ? "　開始済みです。直して保存すると、新しい版になります（開始済みのワークフローは変わりません）。" : "　内容を直せます。開始するまで組織実行は始まりません。"}</div>
      <div className="field"><span>目的</span><textarea rows={2} style={{ width: "100%" }} value={f.goal} onChange={(e) => set({ goal: e.target.value })} aria-label="目的" /></div>
      <div className="field"><span>成果物の形</span><textarea rows={2} style={{ width: "100%" }} value={f.deliverable} onChange={(e) => set({ deliverable: e.target.value })} aria-label="成果物の形" /></div>
      <div className="field"><span>入力ファイル</span>
        <div>
          {f.inputs.map((i, n) => {
            const c = !dirty ? checkOf(i.path) : undefined;
            return (
              <div key={n} style={{ display: "flex", gap: 4, marginBottom: 2, flexWrap: "wrap" }}>
                <input type="text" className="mono grow" value={i.path} placeholder="作業フォルダからの相対パス" aria-label={`入力ファイル${n + 1}のパス`} onChange={(e) => setInput(n, { path: e.target.value })} />
                <input type="text" className="grow" value={i.note} placeholder="メモ" aria-label={`入力ファイル${n + 1}のメモ`} onChange={(e) => setInput(n, { note: e.target.value })} />
                <button type="button" className="btn sm" aria-label={`入力ファイル${n + 1}を外す`} onClick={() => set({ inputs: f.inputs.filter((_, j) => j !== n) })}>外す</button>
                {c && c.check !== "committed" ? <span className="why err small" role="status">{c.label}</span> : c ? <span className="small muted">{c.label}</span> : null}
              </div>
            );
          })}
          <button type="button" className="btn sm" onClick={() => set({ inputs: [...f.inputs, { path: "", note: "" }] })}>入力ファイルを追加</button>
        </div>
        <span className="note">Worker の作業場所には、コミット済みのファイルだけが入ります。未コミット・フォルダの外・見つからないものは警告を出します（開始は止めません）。</span></div>
      <div className="field"><span>制約（1行に1つ）</span><textarea rows={2} style={{ width: "100%" }} value={f.constraints} onChange={(e) => set({ constraints: e.target.value })} aria-label="制約" /></div>
      <div className="field"><span>完了条件（1行に1つ。1件以上必要）</span><textarea rows={3} style={{ width: "100%" }} value={f.doneCriteria} onChange={(e) => set({ doneCriteria: e.target.value })} aria-label="完了条件" /></div>
      <div className="field"><span>未解決の点（1行に1つ）</span><textarea rows={2} style={{ width: "100%" }} value={f.openQuestions} onChange={(e) => set({ openQuestions: e.target.value })} aria-label="未解決の点" />
        <span className="note">Planner は推測で埋めず、計画に明記します。</span></div>
      <div className="field"><span>作業フォルダ</span><div className="mono">{view.prefs.folder}</div></div>
      <div className="field"><span>テンプレート（進め方）</span>
        <select value={f.graphId} onChange={(e) => set({ graphId: e.target.value })} aria-label="テンプレート">
          <option value="">計画して実行（標準）</option>
          {templates.map((t) => <option key={t.id} value={t.id} disabled={!t.selectable}>{t.name}{t.selectable ? "" : "（選べません）"}</option>)}
        </select></div>
      <div className="field"><span>並列数・turn 上限（任意）</span>
        <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
          <label>Worker 同時 <input type="text" inputMode="numeric" size={3} value={f.maxWriters} placeholder="2" onChange={(e) => set({ maxWriters: e.target.value })} aria-label="Worker の同時実行数（1〜4）" /></label>
          <label>全体 <input type="text" inputMode="numeric" size={3} value={f.maxTotal} placeholder="3" onChange={(e) => set({ maxTotal: e.target.value })} aria-label="全体の同時実行数（1〜4）" /></label>
          <label>turn 上限 <input type="text" inputMode="numeric" size={4} value={f.maxTurns} placeholder={String(turnDefault)} onChange={(e) => set({ maxTurns: e.target.value })} aria-label="turn 上限" /></label>
        </div>
        {est ? <span className="note">{est.lines.join(" ")}</span> : null}</div>
      {problem ? <div className="why err" role="alert">{problem}</div> : null}
      {blockers.length > 0 ? <div className="why err" role="status">開始できません: {blockers.map((b) => b.message).join("／")}</div> : null}
      {dirty ? <div className="small muted">保存していない変更があります。保存してから開始してください。</div> : null}
      <div className="acts end">
        <button type="button" className="btn" disabled={busy || !dirty || !!problem} onClick={() => void save()}>保存</button>
        <button type="button" className="btn primary" disabled={busy || dirty || !!problem || blockers.length > 0 || !view.composedPreview || view.state.kind === "started"}
          onClick={() => openDialogEvent({ type: "orgIntakeStart", chatId: chat.id })}>この内容で開始…</button>
      </div>
    </div>
  );
}

// ───────────────────────────── 開始確認 ─────────────────────────────

/** 開始確認ダイアログの本文。Planner へ送る依頼文の全文を見せ、「開始」を押したときだけ組織実行を始める。 */
export function IntakeStartBody({ chat, onStarted, onPrepare }: {
  chat: ChatKey;
  onStarted: (workflowId: string, warning: string | null) => void;
  onPrepare: (folder: string) => void;
}) {
  const [view, setView] = useState<IntakeView | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [needPrep, setNeedPrep] = useState(false);
  /** 開始前チェックが所有者違いで止まったときのリポジトリの場所（許可の選択肢を出す）。 */
  const [ownerRepo, setOwnerRepo] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const loaded = useRef(false);
  const load = useCallback(() => { host.orgIntakeView(chat).then(setView).catch((e) => setErr(errText(e))); }, [chat]);
  useEffect(() => { if (!loaded.current) { loaded.current = true; load(); } }, [load]);
  const ok = !!view && view.state.kind === "ready" && view.blockers.length === 0 && !!view.composedPreview;
  const warns = (view?.inputChecks ?? []).filter((c) => c.check !== "committed");
  const go = async () => {
    if (!view || !ok || busy) return;
    setBusy(true); setErr(null);
    try {
      const r = await host.orgIntakeStart(chat, view.rev, view.composedPreview!);
      onStarted(r.workflowId, r.intakeWarning);
    } catch (e) {
      const ie = host.asIpcError(e);
      setErr(ie.message);
      const owner = blockedOwnerRepo(ie.blocked);
      setOwnerRepo(owner);
      setNeedPrep(ie.blocked?.kind === "repoNotPrepared" && !owner);
      load();
    }
    setBusy(false);
  };
  return (
    <div className="content">
      {!view && !err ? <p className="small muted">依頼の内容を読み込んでいます…</p> : null}
      {view ? (
        <>
          <div className="field"><span>作業フォルダ</span><div className="mono">{view.prefs.folder}</div></div>
          <div className="field"><span>テンプレート</span><div>{view.prefs.graphId ?? "計画して実行（標準）"}</div></div>
          <div className="field"><span>Planner へ送る依頼文（第{view.rev}版）</span>
            {view.composedPreview ? <pre className="mono" style={{ maxHeight: 280, overflow: "auto", whiteSpace: "pre-wrap" }} tabIndex={0} aria-label="Planner へ送る依頼文">{view.composedPreview}</pre> : <div className="why err">依頼文を作れません。</div>}
            <span className="note">この文をそのまま Planner に送ります。</span></div>
          {warns.length > 0 ? <div className="banner warn" role="status"><div><b>入力ファイルの注意</b><ul className="check-list">{warns.map((w) => <li key={w.path}><span className="mono">{w.path}</span>: {w.label}</li>)}</ul></div></div> : null}
          {view.blockers.length > 0 ? <div className="why err" role="alert">開始できません: {view.blockers.map((b) => b.message).join("／")}</div> : null}
          <div className="banner info" role="note"><div>
            計画は<b>計画承認カード</b>で確認できます（確認が必要な内容を含む計画は、承認するまでタスクを送りません）。AgentDock は自動でマージしません。
          </div></div>
        </>
      ) : null}
      {ownerRepo ? <OwnerAllowPanel repo={ownerRepo} onAllowed={() => { setOwnerRepo(null); setErr(null); load(); }} /> : null}
      {err ? <div className="why err" role="alert">{err}{needPrep && view ? <> <button type="button" className="btn sm" onClick={() => onPrepare(view.prefs.folder)}>Gitを準備…</button></> : null}</div> : null}
      <FootActions main={<button className="btn primary" disabled={!ok || busy} onClick={() => void go()}>{busy ? "開始しています…" : "開始"}</button>} />
    </div>
  );
}

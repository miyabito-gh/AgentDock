import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type { ModelInfo } from "../ipc/gen/ModelInfo";
import type { AgentView } from "../ipc/gen/AgentView";
import type { SubagentImportPreview, SubagentRunView, ChatKey, ModelChoice } from "../ipc/subagents";
import * as subagents from "../ipc/subagents";
import * as host from "../ipc/client";

const short = (value: string | null | undefined) => value ? value.slice(0, 12) : "未確認";
const stateText: Record<string, string> = {
  initializing: "準備中", running: "実行中", waiting: "応答待ち", idle: "待機中", done: "完了を観測",
  failed: "失敗を観測", interrupted: "中断を観測", closed: "終了を観測", unknown: "状態不明",
};
const freshnessText: Record<string, string> = {
  live: "ライブ情報", historyOnly: "履歴のみ", needsReconcile: "再確認が必要", disconnected: "切断中", unsupported: "取得非対応",
};
const phaseText: Record<string, string> = {
  creatingWorktree: "作業場所を準備中", creatingParent: "親チャットを準備中", startUnknown: "開始結果は未確認",
  firstSendPending: "初回送信を確認中", firstSendUnknown: "初回送信結果は未確認", ready: "開始処理済み（実行完了を示すものではありません）",
};
const knownText = (x: { kind: string; value?: string }) => x.kind === "value" ? x.value?.trim() || null : null;
const scanBlocker = (text: string) => text === "子孫の走査が完了していません" || text === "子孫の直接親を含む走査の照合が未完了です";
const uncertain = (outcome: subagents.OrgImportOutcome | null | undefined) => !outcome || outcome.kind === "unknown" || outcome.kind === "partial";
const outcomeText: Record<subagents.OrgImportOutcome["kind"], string> = {
  applied: "適用済み", notApplied: "未適用", partial: "一部のみ変更", unknown: "結果未確認",
};

function choiceFor(model: ModelInfo | undefined): ModelChoice | null {
  return model ? { model: model.id, effort: model.defaultEffort, speedTier: model.defaultSpeedTier } : null;
}

function evidenceVersion(view: SubagentRunView): string {
  // Opaque IDs are compared only for equality. Any observed turn/state change
  // invalidates the user's confirmation; Git/source changes are rechecked by IPC.
  return JSON.stringify([view.run.parent, view.run.worktree?.path, view.run.worktree?.baseCommit,
    view.agents.map((a) => ({ key: `${a.agent.key.backend}:${a.agent.key.id}`, turn: a.agent.latestTurn,
      statusTurn: a.status.turn, state: a.status.state, freshness: a.freshness }))
      .sort((a, b) => a.key.localeCompare(b.key))]);
}

function agentName(agent: AgentView, parent: boolean): string {
  const name = knownText(agent.agent.displayName) ?? knownText(agent.agent.role);
  return `${parent ? "親" : "子"}${name ? ` · ${name}` : ""}`;
}

function Outcome({ outcome }: { outcome: subagents.OrgImportOutcome }) {
  switch (outcome.kind) {
    case "applied": return <p className="subagent-outcome" role="status">取り込み適用を確認しました。</p>;
    case "notApplied": return <p className="subagent-outcome" role="status">取り込まれませんでした: {outcome.reason}</p>;
    case "partial": return <p className="why err" role="alert">一部のみ変更されています: {outcome.files.join("、") || "対象ファイル不明"}。再取り込みの前に、結果を確認してください。</p>;
    case "unknown": return <p className="why" role="status">取り込み結果は未確認です。結果を確認するまで再取り込みはできません。</p>;
  }
}

export function SubagentRunsBody({ live, models, onOpenChat }: {
  live: boolean;
  models: ModelInfo[];
  onOpenChat: (chat: ChatKey) => void;
}) {
  const [runs, setRuns] = useState<SubagentRunView[]>([]);
  const [tab, setTab] = useState<"new" | "runs">("runs");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [repo, setRepo] = useState("");
  const [request, setRequest] = useState("");
  const [modelId, setModelId] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [recovery, setRecovery] = useState<Record<string, string>>({});
  const [previews, setPreviews] = useState<Record<string, SubagentImportPreview>>({});
  const [confirmed, setConfirmed] = useState<Record<string, boolean>>({});
  const [outcomes, setOutcomes] = useState<Record<string, subagents.OrgImportOutcome>>({});
  const [verified, setVerified] = useState<Record<string, subagents.SubagentImportRecord[]>>({});
  const [previewNotes, setPreviewNotes] = useState<Record<string, string>>({});
  const [newRun, setNewRun] = useState<string | null>(null);
  const [pollStopped, setPollStopped] = useState(false);
  const mounted = useRef(true);
  const operation = useRef(false);
  const reading = useRef(false);
  const currentRuns = useRef<SubagentRunView[]>([]);
  const currentPreviews = useRef<Record<string, SubagentImportPreview>>({});
  const previewVersions = useRef<Record<string, string>>({});
  const uncertainAttempts = useRef<Record<string, Set<string>>>({});
  const pollAttempts = useRef(0);
  const tabButtons = useRef<Array<HTMLButtonElement | null>>([]);
  const newHeading = useRef<HTMLHeadingElement | null>(null);
  const focusNew = useRef(false);
  const id = useId();

  const updatePreviews = useCallback((change: (old: Record<string, SubagentImportPreview>) => Record<string, SubagentImportPreview>) => {
    currentPreviews.current = change(currentPreviews.current);
    setPreviews(currentPreviews.current);
  }, []);
  const acceptRuns = useCallback((next: SubagentRunView[]) => {
    const invalid = Object.keys(currentPreviews.current).filter((key) => {
      const view = next.find((r) => r.run.id === key);
      const preview = currentPreviews.current[key];
      const previous = currentRuns.current.find((r) => r.run.id === key);
      return !view || previewVersions.current[key] !== evidenceVersion(view)
        || preview.blockers.some((reason) => previous?.blockers.includes(reason) && !view.blockers.includes(reason))
        || (preview.blockers.some(scanBlocker) && !view.blockers.some(scanBlocker));
    });
    if (invalid.length) {
      updatePreviews((old) => { const value = { ...old }; for (const key of invalid) delete value[key]; return value; });
      setConfirmed((old) => { const value = { ...old }; for (const key of invalid) delete value[key]; return value; });
      setPreviewNotes((old) => { const value = { ...old }; for (const key of invalid) value[key] = "状態が更新されました。取り込み内容をもう一度確認してください。"; return value; });
    }
    currentRuns.current = next;
    setRuns(next);
  }, [updatePreviews]);
  const reload = useCallback(async (): Promise<boolean> => {
    if (!live || reading.current) return false;
    reading.current = true;
    if (mounted.current) setLoading(true);
    try {
      const next = await subagents.subagentRuns();
      if (mounted.current) acceptRuns(next);
      return true;
    } catch (e) {
      if (mounted.current) setError(host.asIpcError(e).message);
      return false;
    } finally {
      reading.current = false;
      if (mounted.current) setLoading(false);
    }
  }, [live, acceptRuns]);
  useEffect(() => {
    mounted.current = true;
    void reload();
    return () => { mounted.current = false; };
  }, [reload]);
  useEffect(() => {
    if (live) return;
    // A preview belongs to the connection on which it was prepared.
    updatePreviews(() => ({})); setConfirmed({}); previewVersions.current = {};
  }, [live, updatePreviews]);

  const scanPending = runs.some((r) => r.blockers.some(scanBlocker));
  useEffect(() => {
    if (!live || tab !== "runs" || !scanPending || pollStopped) return;
    // A finite read-only refresh budget; rerendering runs never resets it.
    const timer = window.setInterval(() => {
      if (operation.current || reading.current) return;
      if (pollAttempts.current >= 20) { window.clearInterval(timer); setPollStopped(true); return; }
      pollAttempts.current += 1;
      void reload().then((ok) => { if (!ok && mounted.current) { window.clearInterval(timer); setPollStopped(true); } });
    }, 3_000);
    return () => window.clearInterval(timer);
  }, [live, tab, scanPending, pollStopped, reload]);
  useEffect(() => {
    if (!focusNew.current || tab !== "runs" || !newHeading.current) return;
    focusNew.current = false;
    newHeading.current.focus();
    newHeading.current.scrollIntoView({ block: "nearest" });
  }, [newRun, tab, runs]);

  const resetPolling = () => { pollAttempts.current = 0; setPollStopped(false); };
  const runAction = async (key: string, action: () => Promise<void>) => {
    if (!live || operation.current || reading.current) return;
    operation.current = true;
    setBusy(key); setError(null);
    try { await action(); }
    catch (e) { if (mounted.current) setError(host.asIpcError(e).message); }
    finally { operation.current = false; if (mounted.current) setBusy(null); }
  };
  const start = () => runAction("start", async () => {
    const result = await subagents.subagentStart(repo.trim(), request.trim(), choiceFor(models.find((m) => m.id === modelId)));
    if (!mounted.current) return;
    acceptRuns([result, ...currentRuns.current.filter((r) => r.run.id !== result.run.id)]);
    setRequest(""); resetPolling(); focusNew.current = true; setNewRun(result.run.id); setTab("runs");
  });
  const pickRepo = () => runAction("folder", async () => {
    const folder = await host.pickFolder();
    if (folder && mounted.current) setRepo(folder);
  });
  const recover = (view: SubagentRunView) => runAction(`recover:${view.run.id}`, async () => {
    const parent = view.recoveryCandidates.find((c) => c.id === recovery[view.run.id]);
    if (!parent) return;
    const result = await subagents.subagentRecover(view.run.id, parent);
    if (!mounted.current) return;
    acceptRuns(currentRuns.current.map((r) => r.run.id === result.run.id ? result : r)); resetPolling();
  });
  const preview = (key: string) => runAction(`preview:${key}`, async () => {
    // Explicitly requesting a new preview also discards the previous confirmation.
    updatePreviews((old) => { const next = { ...old }; delete next[key]; return next; });
    setConfirmed((old) => ({ ...old, [key]: false }));
    const version = currentRuns.current.find((r) => r.run.id === key);
    const result = await subagents.subagentImportPreview(key);
    if (!mounted.current) return;
    previewVersions.current[key] = version ? evidenceVersion(version) : "";
    updatePreviews((old) => ({ ...old, [key]: result }));
    setPreviewNotes((old) => { const next = { ...old }; delete next[key]; return next; });
    resetPolling();
    // Refresh scan blockers too: the preview read may have completed a scan.
    await reload();
  });
  const apply = (key: string, token: string) => runAction(`import:${key}`, async () => {
    if (currentPreviews.current[key]?.token !== token || !confirmed[key]) return;
    // A preview can authorize only one attempt, including errors and NotApplied.
    updatePreviews((old) => { const next = { ...old }; delete next[key]; return next; });
    delete previewVersions.current[key];
    setConfirmed((old) => ({ ...old, [key]: false }));
    const previousRecords = currentRuns.current.find((r) => r.run.id === key)?.imports ?? [];
    uncertainAttempts.current[key] = new Set(previousRecords.map((record) => record.id));
    try {
      const result = await subagents.subagentImport(key, token);
      if (mounted.current) {
        setOutcomes((old) => ({ ...old, [key]: result }));
        if (!uncertain(result)) delete uncertainAttempts.current[key];
      }
    } catch (e) {
      if (mounted.current) setOutcomes((old) => ({ ...old, [key]: { kind: "unknown", headNow: { kind: "notFetched" } } }));
      throw e;
    } finally {
      if (mounted.current) await reload();
    }
  });
  const verify = (key: string) => runAction(`verify:${key}`, async () => {
    const records = await subagents.subagentVerifyImport(key);
    if (!mounted.current) return;
    setVerified((old) => ({ ...old, [key]: records }));
    const previousIds = uncertainAttempts.current[key];
    const attemptRecords = previousIds ? records.filter((record) => !previousIds.has(record.id)) : records;
    const latest = attemptRecords[attemptRecords.length - 1];
    if (previousIds && !attemptRecords.length && records.every((record) => !uncertain(record.outcome))) {
      // The host persists Begin before changing any files. A successful ledger
      // read with no new Begin proves this rejected attempt never began.
      setOutcomes((old) => ({ ...old, [key]: { kind: "notApplied", reason: "この操作の取り込み開始記録はありません。内容を確認し直してください。" } }));
      delete uncertainAttempts.current[key];
    } else if (latest?.outcome) {
      setOutcomes((old) => ({ ...old, [key]: latest.outcome! }));
      if (!uncertain(latest.outcome)) delete uncertainAttempts.current[key];
    } else if (!latest) {
      setError("取り込み結果を確定できません。作業場所と通常履歴を確認してください。");
    }
    await reload();
  });
  const selectTab = (event: KeyboardEvent<HTMLDivElement>) => {
    const index = tab === "new" ? 0 : 1;
    const next = event.key === "Home" ? 0 : event.key === "End" ? 1 : event.key === "ArrowRight" || event.key === "ArrowLeft" ? 1 - index : null;
    if (next === null) return;
    event.preventDefault(); setTab(next === 0 ? "new" : "runs"); tabButtons.current[next]?.focus();
  };
  const locked = !!busy || loading;

  return <div className="content subagent-runs">
    <div className="tabs subagent-tabs" role="tablist" aria-label="サブエージェントの依頼" onKeyDown={selectTab}>
      <button ref={(node) => { tabButtons.current[0] = node; }} type="button" role="tab" id={`${id}-new-tab`} aria-controls={`${id}-new`} aria-selected={tab === "new"} tabIndex={tab === "new" ? 0 : -1} onClick={() => setTab("new")}>新しい依頼</button>
      <button ref={(node) => { tabButtons.current[1] = node; }} type="button" role="tab" id={`${id}-runs-tab`} aria-controls={`${id}-runs`} aria-selected={tab === "runs"} tabIndex={tab === "runs" ? 0 : -1} onClick={() => setTab("runs")}>依頼一覧{runs.length ? ` (${runs.length})` : ""}</button>
    </div>
    {!live ? <div className="banner warn">Codex に接続すると、この機能を利用できます。</div> : null}
    {error ? <div className="banner warn" role="alert">{error}</div> : null}
    <section className="subagent-tabpanel subagent-start" role="tabpanel" id={`${id}-new`} aria-labelledby={`${id}-new-tab`} hidden={tab !== "new"}>
      <p className="small muted">依頼ごとに独立した作業場所と親チャットを作ります。複数の依頼を並行して進められます。開始操作は1件ずつ処理します。</p>
      <label className="field"><span>元フォルダ（取り込み先）</span><span className="subagent-repo"><input value={repo} disabled={busy === "start"} onChange={(e) => setRepo(e.target.value)} placeholder="リポジトリのルート" /><button type="button" className="btn sm" disabled={!live || locked} onClick={() => void pickRepo()}>選択</button></span></label>
      <label className="field"><span>依頼内容</span><textarea value={request} disabled={busy === "start"} onChange={(e) => setRequest(e.target.value)} rows={6} placeholder="変更してほしい内容を入力" /></label>
      <label className="field"><span>モデル</span><select value={modelId} disabled={busy === "start"} onChange={(e) => setModelId(e.target.value)}><option value="">既定モデル</option>{models.filter((m) => !m.hidden).map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></label>
      <div className="subagent-actions"><button type="button" className="btn primary" disabled={!live || !repo.trim() || !request.trim() || locked} onClick={() => void start()}>{busy === "start" ? "開始しています…" : "依頼を開始"}</button></div>
      <p className="small muted">開始後は親チャットで承認・質問・追加指示を確認できます。子の数はCodex側の制限に従います。</p>
      <details className="subagent-details"><summary>作業場所と同時編集について</summary><p className="small">元リポジトリの未コミット変更は含まれません。同時に編集する子は1体にするよう依頼しますが、アプリによる強制制御ではありません。</p></details>
    </section>
    <section className="subagent-tabpanel" role="tabpanel" id={`${id}-runs`} aria-labelledby={`${id}-runs-tab`} hidden={tab !== "runs"}>
      <div className="subagent-list-head"><span className="small muted">親チャットから作業の様子を確認できます。</span><button type="button" className="btn sm" disabled={!live || locked} onClick={() => { resetPolling(); setError(null); void reload(); }}>{loading ? "更新中…" : "一覧を更新"}</button></div>
      {scanPending ? <p className="small muted" role="status">{pollStopped ? "自動確認を終えました。「一覧を更新」で再確認できます。" : "子の情報を確認しています。走査が終わるまで、約3秒ごとに一覧を更新します。"}</p> : null}
      {runs.length === 0 ? <div className="subagent-empty"><p>{loading ? "依頼を読み込んでいます…" : "まだ依頼がありません。"}</p>{!loading ? <button type="button" className="btn" onClick={() => setTab("new")}>新しい依頼を作る</button> : null}</div> : null}
      {runs.map((view) => {
        const { run, agents, blockers, recoveryCandidates, imports } = view;
        const pv = previews[run.id];
        const records = verified[run.id] ?? imports;
        const latestRecord = records[records.length - 1];
        const outcome = outcomes[run.id] ?? latestRecord?.outcome;
        const needsVerify = records.some((r) => uncertain(r.outcome)) || (!!outcome && uncertain(outcome));
        const parent = agents.find((a) => a.agent.key.id === run.parent?.id && a.agent.key.backend === run.parent?.backend)
          ?? agents.find((a) => a.agent.parent.kind === "root");
        const children = agents.filter((a) => a !== parent);
        const reasons = [...new Set([...blockers, ...(pv?.blockers ?? [])])]
          .filter((reason) => !(needsVerify && reason.includes("取り込み結果が未確認")));
        const canApply = live && !!pv?.token && reasons.length === 0 && !!confirmed[run.id] && !locked && !needsVerify;
        return <article className={`subagent-run${newRun === run.id ? " is-new" : ""}`} key={run.id}>
          <header><h3 ref={newRun === run.id ? newHeading : undefined} tabIndex={-1}>{run.title || "サブエージェント依頼"}</h3><time className="small muted">{new Date(run.createdAt).toLocaleString()}</time></header>
          <div className="subagent-overview">
            {parent ? <p><strong>{agentName(parent, true)}</strong><span>{stateText[parent.status.state] ?? "状態不明"}</span><span className="small muted">{freshnessText[parent.freshness] ?? "未確認"}</span></p> : <p className="muted">親チャットは未確認です。</p>}
            <p className="small subagent-children"><span>子 {children.length}件</span>{children.slice(0, 3).map((a) => <span key={`${a.agent.key.backend}:${a.agent.key.id}`}>{agentName(a, false)}: {stateText[a.status.state] ?? "状態不明"} <span className="muted">({freshnessText[a.freshness] ?? "未確認"})</span></span>)}{children.length > 3 ? <span className="muted">ほか {children.length - 3}件（詳細に表示）</span> : null}</p>
          </div>
          <div className="subagent-actions">
            {run.parent ? <button type="button" className="btn sm" onClick={() => onOpenChat(run.parent!)}>親チャットを開く</button> : null}
            <button type="button" className="btn sm" disabled={!live || locked || needsVerify} onClick={() => void preview(run.id)}>{busy === `preview:${run.id}` ? "内容を確認中…" : "取り込み内容を確認"}</button>
            {needsVerify ? <button type="button" className="btn sm" disabled={!live || locked} onClick={() => void verify(run.id)}>{busy === `verify:${run.id}` ? "結果を確認中…" : "取り込み結果を確認"}</button> : null}
          </div>
          {reasons.length ? <div className="subagent-blockers small"><strong>取り込みの前に確認が必要です</strong><ul>{reasons.map((reason) => <li key={reason}>{reason}</li>)}</ul></div> : !needsVerify && !pv && !outcome ? <p className="small muted">取り込み内容を確認できます。</p> : null}
          {previewNotes[run.id] && !pv ? <p className="small muted" role="status">{previewNotes[run.id]}</p> : null}
          {pv ? <div className="subagent-preview">
            <h4>取り込む変更</h4>
            <dl className="subagent-paths"><dt>成果の作業場所</dt><dd><code>{run.worktree?.path ?? "未確認"}</code></dd><dt>取り込み先</dt><dd><code>{run.repo}</code></dd></dl>
            {pv.files.length ? <ul className="subagent-files">{pv.files.map((file) => <li key={`${file.path}:${file.moveTo ?? ""}`}><code>{file.path}{file.moveTo ? ` → ${file.moveTo}` : ""}</code><span className="subagent-file-count">+{knownNumber(file.additions)} / −{knownNumber(file.deletions)}{file.binary ? "（バイナリ）" : ""}</span></li>)}</ul> : <p className="small muted">取り込むファイルはありません。</p>}
            <details className="subagent-details"><summary>確認したコミット</summary><dl className="subagent-meta"><dt>成果</dt><dd><code>{pv.sourceHead ?? "未確認"}</code></dd><dt>取り込み先の現在</dt><dd><code>{pv.targetHead ?? "未確認"}</code></dd><dt>適用後</dt><dd><code>{pv.resultHead ?? "未確認"}</code></dd></dl></details>
            {pv.token && reasons.length === 0 && !needsVerify ? <label className="subagent-consent"><input type="checkbox" checked={!!confirmed[run.id]} disabled={!live || locked} onChange={(e) => setConfirmed((old) => ({ ...old, [run.id]: e.target.checked }))} /><span>変更ファイルと取り込み先を確認しました。この内容を取り込みます。</span></label> : null}
            <button type="button" className="btn danger" disabled={!canApply} onClick={() => pv.token && void apply(run.id, pv.token)}>表示内容を取り込む</button>
          </div> : null}
          {busy === `import:${run.id}` ? <p className="small" role="status">取り込み結果を確認しています…</p> : null}
          {outcome ? <Outcome outcome={outcome} /> : latestRecord && !latestRecord.outcome ? <p className="small" role="status">取り込み結果が未確認です。結果を確認してください。</p> : null}
          <details className="subagent-details">
            <summary>作業場所・エージェントの詳細</summary>
            <dl className="subagent-meta"><dt>開始段階</dt><dd>{phaseText[run.phase] ?? "未確認"}{run.detail ? ` — ${run.detail}` : ""}</dd><dt>元リポジトリ</dt><dd><code>{run.repo}</code></dd><dt>作業場所</dt><dd><code>{run.worktree?.path ?? "未作成／未確認"}</code></dd><dt>依頼ID</dt><dd><code>{run.id}</code></dd><dt>初回送信の記録</dt><dd><code>{run.firstAttempt ?? "未確認"}</code></dd></dl>
            {agents.length ? <ul className="subagent-agents">{agents.map((a) => <li key={`${a.agent.key.backend}:${a.agent.key.id}`}><strong>{agentName(a, a === parent)}</strong> <span>{stateText[a.status.state] ?? "状態不明"} / {freshnessText[a.freshness] ?? "未確認"}</span>{knownText(a.agent.assignment) ? <p className="small">担当: {knownText(a.agent.assignment)}</p> : null}{a.currentActivity ? <p className="small muted">{knownText(a.currentActivity.summary) ?? "活動内容は未確認"}</p> : null}<div className="small muted">ID: <code>{a.agent.key.id}</code> / 最新のやりとり: <code>{a.agent.latestTurn ?? "未確認"}</code></div></li>)}</ul> : <p className="small muted">エージェントの情報は未確認です。</p>}
            {recoveryCandidates.length ? <details className="subagent-details"><summary>親チャットの関連付けを修復</summary><p className="small muted">既存の履歴を関連付けます。送信や再開は行いません。</p><div className="subagent-actions"><label className="subagent-recovery"><span>親チャットの候補</span><select value={recovery[run.id] ?? ""} onChange={(e) => setRecovery((old) => ({ ...old, [run.id]: e.target.value }))}><option value="">選択してください</option>{recoveryCandidates.map((candidate, index) => <option key={candidate.id} value={candidate.id}>候補 {index + 1} ({short(candidate.id)})</option>)}</select></label><button type="button" className="btn sm" disabled={!live || !recovery[run.id] || locked} onClick={() => void recover(view)}>選択した親を関連付け</button></div></details> : null}
          </details>
          {records.length || outcome ? <details className="subagent-details"><summary>取り込み履歴{records.length ? ` (${records.length})` : ""}</summary>{records.map((record) => <div className="subagent-import-record" key={record.id}><p className="small">{new Date(record.at).toLocaleString()} — {record.outcome ? outcomeText[record.outcome.kind] : "結果未確認"}</p><dl className="subagent-meta"><dt>取り込み前</dt><dd><code>{record.before}</code></dd><dt>成果</dt><dd><code>{record.source}</code></dd><dt>適用後</dt><dd><code>{record.result}</code></dd></dl></div>)}{outcome?.kind === "applied" ? <p className="small">適用を確認したコミット: <code>{outcome.head}</code></p> : outcome?.kind === "unknown" ? <p className="small">現在のコミット: <code>{outcome.headNow.kind === "value" ? outcome.headNow.value : "未確認"}</code></p> : null}</details> : null}
        </article>;
      })}
    </section>
  </div>;
}

function knownNumber(value: { kind: string; value?: number }): string { return value.kind === "value" ? String(value.value) : "?"; }

import { useCallback, useEffect, useState } from "react";
import type { ModelInfo } from "../ipc/gen/ModelInfo";
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
  firstSendPending: "初回送信を確認中", firstSendUnknown: "初回送信結果は未確認", ready: "開始処理済み（実行完了ではありません）",
};
const knownText = (x: { kind: string; value?: string }) => x.kind === "value" ? x.value : "未確認";

function choiceFor(model: ModelInfo | undefined): ModelChoice | null {
  return model ? { model: model.id, effort: model.defaultEffort, speedTier: model.defaultSpeedTier } : null;
}

function Outcome({ outcome }: { outcome: subagents.OrgImportOutcome }) {
  switch (outcome.kind) {
    case "applied": return <p className="small" role="status">取り込み適用を確認しました。結果 HEAD: <code>{outcome.head}</code></p>;
    case "notApplied": return <p className="small" role="status">未適用を確認しました: {outcome.reason}</p>;
    case "partial": return <p className="why err" role="alert">一部のみ変更されています: {outcome.files.join("、") || "対象ファイル不明"}。再試行せず、状態確認を行ってください。</p>;
    case "unknown": return <p className="why" role="status">結果は未確認です。現在 HEAD: {outcome.headNow.kind === "value" ? outcome.headNow.value : "取得できません"}。再試行せず、台帳を確認してください。</p>;
  }
}

export function SubagentRunsBody({ live, models, onOpenChat }: {
  live: boolean;
  models: ModelInfo[];
  onOpenChat: (chat: ChatKey) => void;
}) {
  const [runs, setRuns] = useState<SubagentRunView[]>([]);
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

  const reload = useCallback(async () => {
    if (!live) return;
    setLoading(true); setError(null);
    try { setRuns(await subagents.subagentRuns()); }
    catch (e) { setError(host.asIpcError(e).message); }
    finally { setLoading(false); }
  }, [live]);
  useEffect(() => { void reload(); }, [reload]);

  const runAction = async (key: string, action: () => Promise<void>) => {
    setBusy(key); setError(null);
    try { await action(); }
    catch (e) { setError(host.asIpcError(e).message); }
    finally { setBusy(null); }
  };
  const start = () => runAction("start", async () => {
    const result = await subagents.subagentStart(repo.trim(), request.trim(), choiceFor(models.find((m) => m.id === modelId)));
    setRuns((old) => [result, ...old.filter((r) => r.run.id !== result.run.id)]);
    setRequest("");
  });
  const pickRepo = async () => {
    try { const folder = await host.pickFolder(); if (folder) setRepo(folder); }
    catch (e) { setError(host.asIpcError(e).message); }
  };
  const recover = (run: SubagentRunView) => runAction(`recover:${run.run.id}`, async () => {
    const parent = run.recoveryCandidates.find((c) => c.id === recovery[run.run.id]);
    if (!parent) return;
    const result = await subagents.subagentRecover(run.run.id, parent);
    setRuns((old) => old.map((r) => r.run.id === result.run.id ? result : r));
  });
  const preview = (id: string) => runAction(`preview:${id}`, async () => {
    const result = await subagents.subagentImportPreview(id);
    setPreviews((old) => ({ ...old, [id]: result }));
    setConfirmed((old) => ({ ...old, [id]: false })); setOutcomes((old) => { const next = { ...old }; delete next[id]; return next; });
  });
  const apply = (id: string, token: string) => runAction(`import:${id}`, async () => {
    const result = await subagents.subagentImport(id, token);
    setOutcomes((old) => ({ ...old, [id]: result }));
    await reload();
  });
  const verify = (id: string) => runAction(`verify:${id}`, async () => {
    const result = await subagents.subagentVerifyImport(id);
    setVerified((old) => ({ ...old, [id]: result }));
    await reload();
  });

  return <div className="content subagent-runs">
    <p className="small muted">専用worktreeから独立して開始します。元リポジトリの未コミット変更は含まれません。同時編集数1は依頼文による指示で、強制制御ではありません。</p>
    {!live ? <div className="banner warn">サブエージェント機能は実接続時に利用できます。</div> : null}
    {error ? <div className="banner warn" role="alert">操作を確認できませんでした: {error}</div> : null}
    <section className="subagent-start">
      <h3>新しい依頼</h3>
      <label className="field"><span>Gitリポジトリ</span><span className="subagent-repo"><input value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="リポジトリのルート" /><button type="button" className="btn sm" onClick={() => void pickRepo()}>選択</button></span></label>
      <label className="field"><span>依頼内容</span><textarea value={request} onChange={(e) => setRequest(e.target.value)} rows={4} placeholder="変更してほしい内容を入力" /></label>
      <label className="field"><span>モデル</span><select value={modelId} onChange={(e) => setModelId(e.target.value)}><option value="">既定モデル</option>{models.filter((m) => !m.hidden).map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></label>
      <button type="button" className="btn primary" disabled={!live || !repo.trim() || !request.trim() || busy !== null} onClick={() => void start()}>{busy === "start" ? "開始しています…" : "サブエージェントを開始"}</button>
      <p className="small muted">作成完了は実行成功を意味しません。開始後に親チャットを開き、承認、質問、停止、追加指示を確認できます。</p>
    </section>
    <section>
      <div className="subagent-list-head"><h3>実行履歴</h3><button type="button" className="btn sm" disabled={!live || loading} onClick={() => void reload()}>{loading ? "読込中…" : "再読込"}</button></div>
      {runs.length === 0 ? <p className="small muted">{loading ? "読み込んでいます…" : "記録された依頼はありません。"}</p> : null}
      {runs.map(({ run, agents, blockers, recoveryCandidates, imports }) => {
        const pv = previews[run.id]; const outcome = outcomes[run.id];
        const canApply = !!pv?.token && pv.blockers.length === 0 && blockers.length === 0 && !!confirmed[run.id] && !busy;
        return <article className="subagent-run" key={run.id}>
          <header><strong>{run.title || "サブエージェント依頼"}</strong><span className="mono muted">{new Date(run.createdAt).toLocaleString()}</span></header>
          <dl className="subagent-meta"><dt>開始段階</dt><dd>{phaseText[run.phase] ?? "未確認"}{run.detail ? ` — ${run.detail}` : ""}</dd><dt>作業場所</dt><dd><code>{run.worktree?.path ?? "未作成／未確認"}</code></dd><dt>初回送信</dt><dd>{run.firstAttempt ? "記録あり" : "未確認"}</dd></dl>
          {run.parent ? <div className="subagent-actions"><button type="button" className="btn sm" onClick={() => onOpenChat(run.parent!)}>親チャットを開く</button><span className="small muted">開くだけで送信・再開はしません。</span></div> : null}
          {recoveryCandidates.length ? <div className="subagent-actions"><label className="field"><span>復旧する親チャットを選ぶ</span><select value={recovery[run.id] ?? ""} onChange={(e) => setRecovery((old) => ({ ...old, [run.id]: e.target.value }))}><option value="">選択してください</option>{recoveryCandidates.map((c) => <option key={c.id} value={c.id}>{c.id}</option>)}</select></label><button type="button" className="btn sm" disabled={!recovery[run.id] || !!busy} onClick={() => void recover({ run, agents, blockers, recoveryCandidates, imports })}>選択した親を関連付け</button><span className="small muted">履歴を関連付けるだけです。送信や再開はしません。</span></div> : null}
          <h4>観測したエージェント</h4>
          {agents.length ? <ul className="subagent-agents">{agents.map((a) => <li key={a.agent.key.id}><b>{knownText(a.agent.displayName)}</b> <span>{stateText[a.status.state] ?? a.status.state}</span> <span className="muted">{freshnessText[a.freshness] ?? a.freshness}</span>{a.currentActivity ? <span className="muted"> — {knownText(a.currentActivity.summary)}</span> : null}</li>)}</ul> : <p className="small muted">子エージェントはまだ観測されていません。</p>}
          {blockers.length ? <div className="banner warn"><b>確認事項</b><ul>{blockers.map((b, i) => <li key={i}>{b}</li>)}</ul></div> : null}
          {imports.length ? <div className="small">過去の取り込み記録: {imports.map((r) => `${r.outcome?.kind ?? "結果未確認"} (${short(r.result)})`).join("、")}</div> : null}
          <div className="subagent-actions"><button type="button" className="btn sm" disabled={!live || !!busy} onClick={() => void preview(run.id)}>{busy === `preview:${run.id}` ? "確認中…" : "取り込み内容を確認"}</button></div>
          {pv ? <div className="subagent-preview">
            <h4>確認した内容</h4><dl className="subagent-meta"><dt>成果HEAD</dt><dd>{short(pv.sourceHead)}</dd><dt>取込先HEAD</dt><dd>{short(pv.targetHead)}</dd><dt>適用後HEAD</dt><dd>{short(pv.resultHead)}</dd></dl>
            {pv.files.length ? <ul>{pv.files.map((f) => <li key={`${f.path}:${f.moveTo ?? ""}`}><code>{f.path}</code>{f.moveTo ? ` → ${f.moveTo}` : ""} <span className="muted">(+{knownNumber(f.additions)} / −{knownNumber(f.deletions)})</span></li>)}</ul> : <p className="small muted">取り込むファイルはありません。</p>}
            {[...new Set([...blockers, ...pv.blockers])].length ? <ul className="why err">{[...new Set([...blockers, ...pv.blockers])].map((b, i) => <li key={i}>{b}</li>)}</ul> : null}
            {pv.token && pv.blockers.length === 0 && blockers.length === 0 ? <label className="small"><input type="checkbox" checked={!!confirmed[run.id]} onChange={(e) => setConfirmed((old) => ({ ...old, [run.id]: e.target.checked }))} /> 表示されたHEADとファイル一覧を確認し、この内容を取り込む</label> : <p className="small muted">取り込みには利用可能な確認トークンが必要です。</p>}
            <button type="button" className="btn danger" disabled={!canApply} onClick={() => pv.token && void apply(run.id, pv.token)}>{busy === `import:${run.id}` ? "取り込み結果を確認中…" : "表示内容を取り込む"}</button>
            {outcome ? <Outcome outcome={outcome} /> : null}
          </div> : null}
          {imports.some((r) => !r.outcome || r.outcome.kind === "unknown" || r.outcome.kind === "partial") ? <div className="subagent-actions"><button type="button" className="btn sm" disabled={!live || !!busy} onClick={() => void verify(run.id)}>{busy === `verify:${run.id}` ? "台帳を確認中…" : "取り込み台帳を確認"}</button><span className="small muted">照合のみを行い、再取り込みはしません。</span></div> : null}
          {verified[run.id]?.map((r) => <div key={r.id} className="small">確認結果 {r.id}: {r.outcome ? <Outcome outcome={r.outcome} /> : "未確認"}</div>)}
        </article>;
      })}
    </section>
  </div>;
}

function knownNumber(x: { kind: string; value?: number }): string { return x.kind === "value" ? String(x.value) : "?"; }

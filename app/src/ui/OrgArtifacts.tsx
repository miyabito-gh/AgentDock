// 組織実行の成果物パネル（ワークフロー画面の「成果物」節・「中身を見る」ダイアログ）と、取り込み前に戻す・状態を確認する操作（DESIGN_ORG_R3 §1・§2.5）。
// 規則: 見る・プレビューするだけでは作業フォルダを変えない。取り込み・戻すは確認ダイアログの主操作だけ。取れていない値は「未確認」「未取得」と書く。
//       成功と書くのは、ホストが読み戻しで確かめた結果のときだけ。不明のときは再実行せず、読取りだけの「状態を確認」を出す。
import { useCallback, useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { OrgArtifactFile, OrgArtifactItem, OrgArtifactPreview, OrgArtifactsView, OrgInvestigationItem, OrgImportRecordView, OrgImportResult, OrgImportPreview, OrgUndoPreview, OrgUndoOutcome, OrgWorkflowView } from "../ipc/types";
import { Icon, orgIcon } from "./Icon";
import { ConfirmBlock, FootActions } from "./DialogParts";
import { CopyButton, Markdown } from "./Markdown";
import { Counts, DiffText, kindIcon, pathText } from "./DiffParts";
import { splitPath } from "./changesView";
import {
  STATUS_CHIP, actionableRecords, checkOutcomeLabel, checksSummary, defaultTab, fileKey, headingText, importResultText, importable, investigationText, isCandidate, latestReview, orderSelection, parseTable, recordStateText, reviewLine, reviewVerdictLabel, reworkCount, sha7,
  tableDelimiter, tabsOf, totalsText, undoResultText, type FileTab,
} from "./orgArtifactsView";

const errText = (e: unknown) => host.asIpcError(e).message;

function StatusChip({ item }: { item: Pick<OrgArtifactItem, "status" | "statusLabel"> }) {
  const c = STATUS_CHIP[item.status];
  return <span className={`st ${c.tone}`}><Icon name={orgIcon(c.icon)} />{item.statusLabel}</span>;
}

/** ワークフローの成果物を取り直す。署名（タスクの状態・コミット・完了）が変わったときも取り直す。 */
export function useArtifacts(workflowId: string | null, sig: string, live: boolean): { view: OrgArtifactsView | null; error: string | null; reload: () => void } {
  const [view, setView] = useState<OrgArtifactsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    if (!live || !workflowId) { setView(null); setError(null); return; }
    let alive = true;
    host.orgArtifacts(workflowId).then((v) => { if (alive) { setView(v); setError(null); } }).catch((e) => { if (alive) { setView(null); setError(errText(e)); } });
    return () => { alive = false; };
  }, [workflowId, sig, live, nonce]);
  return { view, error, reload: useCallback(() => setNonce((n) => n + 1), []) };
}

// ───────────────────────────── ワークフロー画面の「成果物」節 ─────────────────────────────

export function ArtifactsSection({ wf, live, onAct }: { wf: OrgWorkflowView; live: boolean; onAct: (a: string) => void }) {
  const hasCommit = wf.tasks.some((t) => t.commit);
  const hasInvestigation = wf.tasks.some((t) => t.kind === "investigate");
  const sig = `${wf.finished}|${wf.tasks.map((t) => `${t.id}:${t.status.phase}:${t.commit ?? ""}`).join(",")}|${wf.artifactHint?.imported ?? 0}`;
  const { view, error } = useArtifacts(hasCommit || hasInvestigation ? wf.workflowId : null, sig, live);
  if (!hasCommit && !hasInvestigation) return null;
  if (!live) return <section className="org-artifacts" aria-label="成果物"><h4 className="org-h">成果物</h4><div className="small muted">モックでは動きません（実接続で使えます）。</div></section>;
  if (error) return <section className="org-artifacts" aria-label="成果物"><h4 className="org-h">成果物</h4><div className="banner warn" role="alert"><Icon name="alert" /><div>成果物を取得できませんでした（{error}）。取得できていないだけで、成果物が無いとは限りません。</div></div></section>;
  if (!view) return <section className="org-artifacts" aria-label="成果物"><h4 className="org-h">成果物</h4><div className="small muted">成果物を確認しています…</div></section>;
  const cands = importable(view);
  const open = (task: string | null) => onAct(`orgArtifacts:${wf.workflowId}${task ? `:${task}` : ""}`);
  const imp = (tasks: string[]) => onAct(`orgImport:${wf.workflowId}:${tasks.join(",")}`);
  return (
    <section className="org-artifacts" aria-label="成果物">
      <h4 className="org-h" title="レビュー待ち＝AIの確認は終わり、あなたの確認と取り込みを待っている成果です（AIのレビュー中とは別）">{headingText(view.items)}</h4>
      <InvestigationsBlock items={view.investigations} />
      {view.items.length === 0 ? (view.investigations.length === 0 ? <div className="small muted">表示できる成果物はありません。</div> : null) : (
        <ul className="org-art-list">
          {view.items.map((i) => (
            <li key={i.task} className="org-art-row">
              <span className="mono org-art-id">{i.task}</span>
              <span className="org-art-title" title={i.title}>{i.title}</span>
              <StatusChip item={i} />
              <span className="small org-art-num">{totalsText(i)}</span>
              <span className="small muted org-art-rev">{reviewLine(i)}</span>
              <span className="org-art-acts">
                <button type="button" className="btn sm" onClick={() => open(i.task)}>中身を見る</button>
                {isCandidate(i.status) ? <button type="button" className="btn sm primary" disabled={!i.importable} title={i.importRefusal ?? "確認画面で内容を確かめてから取り込みます"} onClick={() => imp([i.task])}>取り込む…</button> : null}
              </span>
              {i.statusNote ? <div className="small muted org-art-note">{i.statusNote}</div> : null}
            </li>
          ))}
        </ul>
      )}
      <div className="org-art-foot">
        {cands.length > 1 ? <button type="button" className="btn sm" onClick={() => imp(cands.map((c) => c.task))}>{cands.length} 件をまとめて取り込む…</button> : null}
        <button type="button" className="btn sm" onClick={() => open(null)}>成果物を開く</button>
      </div>
      {view.target.blockers.length > 0 && cands.length === 0 && view.items.some((i) => isCandidate(i.status)) ? <div className="small muted">{view.target.blockers.join(" / ")}</div> : null}
      <div className="small muted">取り込み後も作業場所とブランチは残ります（削除はワークフローの「削除…」から）。AgentDock は自動で取り込みません。</div>
    </section>
  );
}

// ───────────────────────────── 調査報告（ファイルを変えない調査タスクの成果。取り込み・差分の対象ではない） ─────────────────────────────

function InvestigationReport({ item }: { item: OrgInvestigationItem }) {
  const r = item.report.kind === "value" ? item.report.value : null;
  return (
    <li className="org-inv">
      <div className="org-inv-head"><span className="mono org-art-id">{item.task}</span> <span className="org-art-title" title={item.title}>{item.title}</span> <span className="tag" title="ファイルを変えない調査のタスク">調査</span> <span className="small muted">{item.statusLabel}</span></div>
      {r === null ? <div className="small muted">{item.phase === "completed" ? "報告を取得できませんでした（未取得）。保存された報告も、履歴の最終回答も読めませんでした。" : "報告はまだありません。"}</div> : (
        <div className="small">
          <div><b>要約</b>　{r.summary}</div>
          {r.findings.length > 0 ? (
            <ol className="org-inv-list">
              {r.findings.map((f, i) => (
                <li key={i}><b>{f.point}</b>
                  <div className="muted">根拠: {f.basis}</div>
                  {f.refs.length > 0 ? <div className="muted">参照: {f.refs.map((x, j) => <span key={j} className="mono">{j > 0 ? " / " : ""}{x}</span>)}</div> : null}
                </li>))}
            </ol>) : null}
          {r.references.length > 0 ? <div className="muted">調べた対象: {r.references.join(" / ")}</div> : null}
          {r.openQuestions.length > 0 ? <div><b>分からなかったこと</b><ul className="check-list">{r.openQuestions.map((q, i) => <li key={i}>{q}</li>)}</ul></div> : null}
          <CopyButton className="btn sm subtle" text={investigationText(r)} label="報告をコピー" />
        </div>)}
    </li>
  );
}

function InvestigationsBlock({ items }: { items: OrgInvestigationItem[] }) {
  if (items.length === 0) return null;
  return (
    <div className="org-invs">
      <h5 className="org-h" title="調査のタスクの報告です。ファイルがないので、取り込み・差分の対象にはなりません">調査報告（{items.length}）</h5>
      <ul className="org-inv-wrap">{items.map((i) => <InvestigationReport key={i.task} item={i} />)}</ul>
    </div>
  );
}

// ───────────────────────────── 取り込み記録の操作（状態を確認・取り込み前に戻す・プッシュ） ─────────────────────────────

/** 取り込み記録1件。確認できないものは「状態を確認」（読取りのみ）、取り込み済み・一部だけのものは「取り込み前に戻す…」（確認のあと）、プッシュは既存のダイアログへ。 */
export function RecordActions({ workflowId, rec, onChanged, onPush }: { workflowId: string; rec: OrgImportRecordView; onChanged: () => void; onPush: (folder: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [undo, setUndo] = useState<{ pv: OrgUndoPreview } | { error: string } | "loading" | null>(null);
  const [msg, setMsg] = useState<{ tone: "ok" | "warn"; text: string } | null>(null);
  const verify = async () => {
    setBusy(true); setMsg(null);
    try { const r = await host.orgImportVerify(workflowId, rec.id); setMsg(importResultText(r)); onChanged(); } catch (e) { setMsg({ tone: "warn", text: `確認できませんでした（${errText(e)}）。` }); }
    setBusy(false);
  };
  const openUndo = async () => {
    setMsg(null); setUndo("loading");
    try { setUndo({ pv: await host.orgImportUndoPreview(workflowId, rec.id) }); } catch (e) { setUndo({ error: errText(e) }); }
  };
  const doUndo = async (pv: OrgUndoPreview) => {
    setBusy(true);
    try {
      const r = await host.orgImportUndo(workflowId, rec.id, pv.expectedHead);
      const o: OrgUndoOutcome = r.outcome;
      setMsg(undoResultText(o)); setUndo(null); onChanged();
    } catch (e) { setMsg({ tone: "warn", text: `戻せませんでした（${errText(e)}）。作業フォルダは変えていません。` }); setUndo(null); }
    setBusy(false);
  };
  return (
    <div className="org-rec">
      <div className="small"><b>{recordStateText(rec.state)}</b>　<span className="muted">{new Date(rec.at).toLocaleString()}　ブランチ <span className="mono">{rec.branch}</span>　{sha7(rec.before)} → {sha7(rec.result)}　（{rec.tasks.join("、")}）</span></div>
      <div className="acts">
        {rec.state === "unknown" ? <button type="button" className="btn sm" disabled={busy} onClick={() => void verify()} title="今のコミットを読み取って確かめます（作業フォルダは変えません）">状態を確認</button> : null}
        {rec.state === "applied" ? <button type="button" className="btn sm" disabled={busy} title="作業フォルダの今のブランチを、確認のうえリモートへ送ります" onClick={() => onPush(rec.folder)}>プッシュ…</button> : null}
        {rec.state === "applied" || rec.state === "partial" ? <button type="button" className="btn sm" disabled={busy || undo !== null} onClick={() => void openUndo()}>取り込み前に戻す…</button> : null}
      </div>
      {undo === "loading" ? <p className="small muted">戻せるか確認しています…</p> : null}
      {undo && undo !== "loading" && "error" in undo ? <div className="banner warn" role="alert"><div>戻せるか確認できませんでした（{undo.error}）。<button type="button" className="btn sm" onClick={() => setUndo(null)}>閉じる</button></div></div> : null}
      {undo && undo !== "loading" && "pv" in undo ? (
        undo.pv.allowed ? (
          <ConfirmBlock label="取り込み前に戻す確認" actions={<>
            <button type="button" className="btn" disabled={busy} onClick={() => setUndo(null)}>やめる</button>
            <button type="button" className="btn danger" disabled={busy} onClick={() => void doUndo(undo.pv)}>{busy ? "戻しています…" : "取り込み前に戻す"}</button></>}>
            <p>{undo.pv.mode === "restoreFiles"
              ? <>取り込みの途中で書き換わったファイル（{undo.pv.files.join("、")}）を、取り込み前（<span className="mono">{sha7(undo.pv.before)}</span>）の内容に戻します。</>
              : <>取り込み前（<span className="mono">{sha7(undo.pv.before)}</span>）に戻します。取り込んだ成果は agentdock ブランチに残っているので、あとでまた取り込めます。あなたの未コミットの変更は残ります。</>}</p>
          </ConfirmBlock>
        ) : <div className="banner warn" role="alert"><div>{undo.pv.refusal ?? "戻せません"}　何も変更していません。<button type="button" className="btn sm" onClick={() => setUndo(null)}>閉じる</button></div></div>
      ) : null}
      {msg ? <div className={`banner ${msg.tone === "ok" ? "info" : "warn"}`} role={msg.tone === "ok" ? "status" : "alert"}><div>{msg.text}</div></div> : null}
    </div>
  );
}

// ───────────────────────────── 「中身を見る」ダイアログの本文 ─────────────────────────────

function FilePreview({ workflowId, task, file, nonce }: { workflowId: string; task: string; file: OrgArtifactFile; nonce: number }) {
  const [pv, setPv] = useState<OrgArtifactPreview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    setPv(null); setErr(null);
    let alive = true;
    host.orgArtifactPreview(workflowId, task, file.path, file.moveTo).then((p) => { if (alive) setPv(p); }).catch((e) => { if (alive) setErr(errText(e)); });
    return () => { alive = false; };
  }, [workflowId, task, file.path, file.moveTo, nonce]);
  if (err) return <p className="why err" role="alert">プレビューを取得できませんでした（{err}）。</p>;
  if (!pv) return <p className="small muted">プレビューを取得しています…</p>;
  if (pv.refusal) return <p className="small muted" role="status">{pv.refusal}</p>;
  const note = pv.truncated ? <p className="small muted">先頭だけを表示しています（全体 {pv.bytes.toLocaleString()} バイト）。</p> : null;
  if (pv.kind === "image" && pv.dataUrl) {
    return <div className="org-art-img"><img src={pv.dataUrl} alt={`${pathText(file)} のプレビュー`} /></div>;
  }
  if (pv.kind === "markdown" && pv.text !== null) return <div className="org-art-md">{note}<Markdown text={pv.text} /></div>;
  if (pv.kind === "table" && pv.text !== null) {
    const rows = parseTable(pv.text, tableDelimiter(file.moveTo ?? file.path));
    return (
      <div className="org-art-table">{note}
        <div className="org-scroll"><table className="org-tbl"><tbody>{rows.map((r, i) => <tr key={i}>{r.map((c, j) => (i === 0 ? <th key={j} scope="col">{c}</th> : <td key={j}>{c}</td>))}</tr>)}</tbody></table></div>
      </div>
    );
  }
  if (pv.text !== null) return <div>{note}<pre className="mono small org-pre org-art-pre">{pv.text}</pre></div>;
  return <p className="small muted">プレビューできません。</p>;
}

function FileDiff({ workflowId, task, file, nonce }: { workflowId: string; task: string; file: OrgArtifactFile; nonce: number }) {
  const [text, setText] = useState<string | null | "none">(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    setText(null); setErr(null);
    if (file.binary) return;
    let alive = true;
    host.orgArtifactDiff(workflowId, task, file.path, file.moveTo).then((d) => { if (alive) setText(d.text.kind === "value" ? d.text.value : "none"); }).catch((e) => { if (alive) setErr(errText(e)); });
    return () => { alive = false; };
  }, [workflowId, task, file.path, file.moveTo, file.binary, nonce]);
  if (file.binary) return <p className="small muted">バイナリのため差分を表示できません。</p>;
  if (err) return <p className="why err" role="alert">差分を取得できませんでした（{err}）。</p>;
  if (text === null) return <p className="small muted">差分を取得しています…</p>;
  if (text === "none") return <p className="small muted" role="status">差分を取得できませんでした（未確認）。</p>;
  return text.trim() === "" ? <p className="small muted">差分の本文はありません（空）。</p> : <DiffText text={text} />;
}

/** 所見（Worker の報告と Reviewer の判定）。取れていないものは「未取得」。 */
function Findings({ item }: { item: OrgArtifactItem }) {
  const rs = item.reviews.kind === "value" ? item.reviews.value : null;
  const last = rs ? latestReview(rs) : null;
  const past = rs && last ? rs.filter((r) => r !== last) : [];
  return (
    <div className="org-art-find">
      <div className="small"><b>Worker の報告</b>（自己申告）</div>
      {item.report.kind === "value" ? (
        <div className="small">
          <div>{item.report.value.summary}</div>
          <div className="muted">テスト: {item.report.value.testsRan ? (item.report.value.testsPassed ? "実行・合格と報告" : "実行・不合格と報告") : "実行していないと報告"}</div>
        </div>
      ) : <div className="small muted">未取得（記録がなく、履歴からも読めませんでした）</div>}
      {item.reportMismatch.length > 0 ? (
        <div className="small"><b>報告と差分の違い</b>（判定には使っていません）<ul className="check-list">{item.reportMismatch.map((m) => <li key={m}>{m}</li>)}</ul></div>
      ) : null}
      <div className="small"><b>Reviewer の判定</b>　{reviewLine(item)}</div>
      {item.reviewNotes.length > 0 ? <div className="banner warn" role="status"><Icon name="alert" /><div>{item.reviewNotes.map((n, i) => <div key={i}>{n}</div>)}</div></div> : null}
      {rs === null ? <div className="small muted">未取得</div> : last ? (
        <div className="small">
          <div className="muted">{checksSummary(last)}</div>
          {last.checks && last.checks.length > 0 ? (
            <ul className="check-list org-checks">{last.checks.map((c, i) => <li key={i}><b className={c.outcome === "failed" ? "why err" : undefined}>{checkOutcomeLabel(c.outcome)}</b>　<span className="mono">{c.command}</span>{c.summary.trim() !== "" ? <span className="muted">　{c.summary}</span> : null}</li>)}</ul>) : null}
          {last.findings.length > 0 ? <ul className="check-list">{last.findings.map((f, i) => <li key={i}>{f}</li>)}</ul> : <div className="muted">所見はありません。</div>}
          {last.fromHistory ? <div className="muted">この判定は、保存された出力ではなく履歴から読んだものです。</div> : null}
          {past.length > 0 ? (
            <details className="about"><summary>これまでの判定（差し戻し {reworkCount(rs)} 回）</summary>
              {past.map((r) => <div key={`${r.reviewerTask}:${r.round}`}><b>{r.round}回目: {reviewVerdictLabel(r)}</b>{r.findings.length > 0 ? <ul className="check-list">{r.findings.map((f, i) => <li key={i}>{f}</li>)}</ul> : null}</div>)}
            </details>) : null}
        </div>
      ) : <div className="small muted">検証の判定はありません。</div>}
    </div>
  );
}

export function OrgArtifactsBody({ workflowId, initialTask, live, onImport, onPush }: { workflowId: string; initialTask: string | null; live: boolean; onImport: (tasks: string[]) => void; onPush: (folder: string) => void }) {
  const { view, error, reload } = useArtifacts(live ? workflowId : null, "dialog", live);
  const [selTask, setSelTask] = useState<string | null>(initialTask);
  const [selFile, setSelFile] = useState<string | null>(null);
  const [tab, setTab] = useState<FileTab>("preview");
  const [pane, setPane] = useState<"list" | "view">("list");
  const [note, setNote] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const item: OrgArtifactItem | null = view ? view.items.find((i) => i.task === selTask) ?? view.items.find((i) => isCandidate(i.status)) ?? view.items[0] ?? null : null;
  const files = item && item.files.kind === "value" ? item.files.value : null;
  const file = files ? files.find((f) => fileKey(f) === selFile) ?? null : null;
  useEffect(() => { if (file) setTab(defaultTab(file)); }, [file ? fileKey(file) : null, item?.task]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { setNote(null); }, [item?.task, selFile]);

  const open = async (f: OrgArtifactFile) => {
    if (!item) return;
    setNote(null);
    const go = async (risk: boolean) => host.orgArtifactOpen(workflowId, item.task, f.path, f.moveTo, risk);
    try { await go(false); setNote("関連アプリで開きました（読取り専用のコピーです。編集しても成果物には反映されません）。"); } catch (e) {
      const er = host.asIpcError(e);
      if (er.blocked?.kind !== "openNeedsConfirm") { setNote(`開けませんでした: ${er.message}`); return; }
      if (!window.confirm(`実行形式のファイルです。開くとプログラムが実行されることがあります。\n${er.blocked.path}\n\n内容を確認したうえで、開きますか？`)) { setNote("開くのをやめました。"); return; }
      try { await go(true); setNote("関連アプリで開きました（読取り専用のコピーです）。"); } catch (e2) { setNote(`開けませんでした: ${errText(e2)}`); }
    }
  };

  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  if (error) return <div className="content"><div className="banner warn" role="alert"><div>成果物を取得できませんでした（{error}）。<button type="button" className="btn sm" onClick={reload}>取り直す</button></div></div></div>;
  if (!view) return <div className="content"><p className="small muted">成果物を確認しています…</p></div>;
  const cands = importable(view);
  const records = actionableRecords(view);
  const tabs = file ? tabsOf(file) : [];

  return (
    <>
      <div className="content chg-content">
        <div className="chg-head">
          <span className="chg-req small muted">{headingText(view.items)}　取り込み先: <span className="mono">{view.target.folder}</span>{view.target.branch.kind === "value" ? <>（ブランチ <span className="mono">{view.target.branch.value}</span>）</> : "（ブランチは未確認）"}</span>
          <button type="button" className="btn sm" onClick={() => { reload(); setNonce((n) => n + 1); }} title="Gitから読み直します（作業フォルダは変えません）">再読込</button>
        </div>
        {view.target.blockers.length > 0 && view.items.some((i) => isCandidate(i.status)) ? <div className="banner warn" role="status"><Icon name="alert" /><div>いまは取り込めません: {view.target.blockers.join(" / ")}</div></div> : null}
        {records.length > 0 ? <div className="org-recs">{records.map((r) => <RecordActions key={r.id} workflowId={workflowId} rec={r} onChanged={reload} onPush={onPush} />)}</div> : null}
        <InvestigationsBlock items={view.investigations} />
        {view.items.length === 0 ? (view.investigations.length === 0 ? <div className="chg-empty small muted" role="status">表示できる成果物はありません</div> : null) : (
          <div className="chg-split" data-pane={pane}>
            <div className="chg-left">
              <ul className="chg-list" aria-label="成果物">
                {view.items.map((i) => (
                  <li key={i.task} className={`chg-row org-art-item${i.task === item?.task ? " sel" : ""}`}>
                    <button type="button" className="chg-rowbtn" aria-current={i.task === item?.task} onClick={() => { setSelTask(i.task); setSelFile(null); setPane("view"); }}>
                      <span className="chg-nm"><span className="chg-name"><span className="mono">{i.task}</span> {i.title}</span><span className="chg-dir muted" style={{ direction: "ltr" }}>{totalsText(i)}</span></span>
                      <StatusChip item={i} />
                    </button>
                  </li>))}
              </ul>
            </div>
            <div className="chg-right">
              <button type="button" className="btn subtle chg-back" onClick={() => setPane("list")}>← 一覧</button>
              {item ? (
                <div className="org-art-detail">
                  <div className="chg-fh"><span className="mono">{item.task}</span> <span>{item.title}</span> <StatusChip item={item} />
                    {isCandidate(item.status) ? <button type="button" className="btn sm primary" disabled={!item.importable} title={item.importRefusal ?? undefined} onClick={() => onImport([item.task])}>取り込む…</button> : null}</div>
                  <div className="small muted org-art-meta">
                    {item.branch ? <>ブランチ <span className="mono">{item.branch}</span> <CopyButton className="btn sm subtle" text={item.branch} label="ブランチ名をコピー" />　</> : null}
                    {item.base ? <>始点 <span className="mono">{sha7(item.base)}</span> → </> : null}成果 <span className="mono">{sha7(item.commit)}</span>
                  </div>
                  {item.statusNote ? <p className="small muted">{item.statusNote}</p> : null}
                  {item.importRefusal && isCandidate(item.status) ? <p className="small muted">いまは取り込めません: {item.importRefusal}</p> : null}
                  {item.status === "notPassed" ? <p className="why err small" role="status">この成果は合格していません（差し戻しの上限など）。見ることはできます。取り込むときは、確認画面で同意が要ります。</p> : null}
                  {item.uncommitted.kind === "value" && item.uncommitted.value > 0 ? <p className="small" role="status">コミットされていない変更が {item.uncommitted.value} 件あります。これは成果に含まれません。</p> : null}
                  <Findings item={item} />
                  {item.imports.length > 0 ? <details className="about"><summary>この成果の取り込み記録（{item.imports.length}）</summary>{item.imports.map((r) => <div key={r.id} className="small">{recordStateText(r.state)}　{new Date(r.at).toLocaleString()}　{sha7(r.before)} → {sha7(r.result)}</div>)}</details> : null}
                  {files === null ? <p className="small muted" role="status">変更ファイルの一覧を取得できませんでした（未確認）。</p> : files.length === 0 ? <p className="small muted">変更されたファイルはありません。</p> : (
                    <div className="org-art-files">
                      <ul className="chg-list" aria-label="変更されたファイル">
                        {files.map((f) => {
                          const k = kindIcon(f);
                          const sp = splitPath(f.path);
                          return (
                            <li key={fileKey(f)} className={`chg-row${fileKey(f) === (file ? fileKey(file) : null) ? " sel" : ""}`}>
                              <button type="button" className="chg-rowbtn" aria-current={fileKey(f) === selFile} title={pathText(f)} onClick={() => setSelFile(fileKey(f))}>
                                <span className={`chg-k ${k.cls}`} title={k.title}><span aria-hidden="true">{k.letter}</span><span className="sr">{k.title}</span></span>
                                <span className="chg-nm"><span className="chg-name mono">{sp.name}{f.moveTo ? ` → ${splitPath(f.moveTo).name}` : ""}</span>{sp.dir ? <span className="chg-dir muted">{sp.dir}</span> : null}</span>
                                <Counts f={f} />
                              </button>
                            </li>);
                        })}
                      </ul>
                    </div>)}
                  {file ? (
                    <div className="org-art-file">
                      <div className="chg-fh"><span className={`chg-k ${kindIcon(file).cls}`} aria-label={kindIcon(file).title}>{kindIcon(file).letter}</span> <span className="mono chg-path">{pathText(file)}</span> <Counts f={file} />
                        <span className="seg" role="tablist" aria-label="表示">
                          {tabs.map((t) => <button key={t} type="button" role="tab" aria-selected={tab === t} onClick={() => setTab(t)}>{t === "preview" ? "プレビュー" : "差分"}</button>)}
                        </span>
                        {file.kind !== "deleted" ? <button type="button" className="btn sm" title="読取り専用のコピーを書き出して、関連アプリで開きます" onClick={() => void open(file)}>開く…</button> : null}</div>
                      {note ? <p className="small muted" role="status">{note}</p> : null}
                      {tab === "preview" && tabs.includes("preview") ? <FilePreview workflowId={workflowId} task={item.task} file={file} nonce={nonce} /> : <FileDiff workflowId={workflowId} task={item.task} file={file} nonce={nonce} />}
                    </div>
                  ) : files && files.length > 0 ? <p className="small muted">ファイルを選ぶと、プレビューと差分を表示します。</p> : null}
                </div>
              ) : <p className="small muted">成果物を選んでください。</p>}
            </div>
          </div>)}
      </div>
      <FootActions main={cands.length > 0 ? <button type="button" className="btn primary" onClick={() => onImport(cands.map((c) => c.task))} title="確認画面で内容を確かめてから取り込みます">{cands.length > 1 ? `${cands.length} 件を取り込む…` : "取り込む…"}</button> : null} />
    </>
  );
}

// ───────────────────────────── 取り込みの確認ダイアログの本文 ─────────────────────────────

export function OrgImportBody({ workflowId, initialTasks, live, onBack, onPush, onClose }: { workflowId: string; initialTasks: string[]; live: boolean; onBack: () => void; onPush: (folder: string) => void; onClose: () => void }) {
  const { view, error, reload } = useArtifacts(live ? workflowId : null, "import", live);
  const [picked, setPicked] = useState<string[]>(initialTasks);
  const [pv, setPv] = useState<OrgImportPreview | null>(null);
  const [pvErr, setPvErr] = useState<string | null>(null);
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [res, setRes] = useState<OrgImportResult | null>(null);
  const [runErr, setRunErr] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const order = view ? orderSelection(view, picked) : [];
  const orderKey = order.join(",");
  const reqId = useRef(0);
  useEffect(() => {
    if (!live || !view || res) return;
    setPv(null); setPvErr(null); setConsent(false);
    if (order.length === 0) return;
    const id = ++reqId.current;
    host.orgImportPreview(workflowId, order).then((p) => { if (reqId.current === id) setPv(p); }).catch((e) => { if (reqId.current === id) setPvErr(errText(e)); });
  }, [live, workflowId, view, orderKey, nonce, res]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async () => {
    if (!pv || !pv.token || busy || res) return;
    setBusy(true); setRunErr(null);
    try { setRes(await host.orgImport(workflowId, pv.token, consent)); reload(); }
    catch (e) { setRunErr(errText(e)); setNonce((n) => n + 1); }
    setBusy(false);
  };

  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  if (error) return <div className="content"><div className="banner warn" role="alert"><div>成果物を取得できませんでした（{error}）。</div></div></div>;
  if (!view) return <div className="content"><p className="small muted">成果物を確認しています…</p></div>;

  const cands = importable(view);
  const rv = res ? importResultText(res) : null;
  const rec = res ? view.records.find((r) => r.id === res.recordId) ?? null : null;
  const notPassed = pv?.notPassed ?? [];
  const ready = !!pv && pv.token !== "" && pv.blockers.length === 0 && (notPassed.length === 0 || consent) && !busy && !res;
  const branches = (pv?.candidates ?? []).map((c) => c.branch).filter((b) => b);

  return (
    <div className="content">
      <p>選んだ成果を、作業フォルダの今のブランチに取り込みます。取り込み前の状態は記録し、送信（プッシュ）前なら「取り込み前に戻す」で戻せます。AgentDock は自動で取り込みません。</p>
      <div className="field"><span>取り込み先</span>
        <div className="mono">{pv?.folder ?? view.target.folder}</div>
        <div className="small">ブランチ <b className="mono">{pv?.branch || (view.target.branch.kind === "value" ? view.target.branch.value : "未確認")}</b>　今のコミット <span className="mono">{sha7(pv?.head || (view.target.head.kind === "value" ? view.target.head.value : null))}</span></div>
      </div>
      <div className="field"><span>取り込む成果</span>
        {cands.length === 0 ? <div className="small muted">取り込める成果がありません。</div> : (
          <ul className="check-list org-imp-list">
            {cands.map((c) => (
              <li key={c.task}><label><input type="checkbox" checked={picked.includes(c.task)} disabled={busy || !!res || cands.length < 2} onChange={(e) => setPicked((p) => (e.target.checked ? [...p, c.task] : p.filter((x) => x !== c.task)))} />
                {" "}<b className="mono">{c.task}</b> {c.title}　<StatusChip item={c} />
                {c.status === "notPassed" ? <span className="why err small">　合格していません</span> : null}</label></li>))}
          </ul>)}
      </div>
      {notPassed.length > 0 ? (
        <label className="small why err org-consent"><input type="checkbox" checked={consent} disabled={busy || !!res} onChange={(e) => setConsent(e.target.checked)} /> 合格していない成果（{notPassed.join("、")}）を、承知のうえで取り込みます</label>
      ) : null}
      {pv && pv.cautions.length > 0 ? <div className="banner warn" role="status"><Icon name="alert" /><div>{pv.cautions.map((c, i) => <div key={i}>{c}</div>)}</div></div> : null}
      {pvErr ? <div className="why err" role="alert">確認の材料を取得できませんでした（{pvErr}）。</div> : null}
      {!pv && !pvErr && order.length > 0 ? <p className="small muted">取り込んだ結果を計算しています（作業フォルダには何も書きません）…</p> : null}
      {pv && pv.blockers.length > 0 ? (
        <div className="banner warn" role="alert"><Icon name="alert" /><div>{pv.blockers.map((b, i) => <div key={i}>{b}</div>)}
          {pv.conflicts.length > 0 && branches.length > 0 ? <div><CopyButton className="btn sm" text={branches.join("\n")} label="ブランチ名をコピー" /></div> : null}</div></div>
      ) : null}
      {pv && pv.files.length > 0 ? (
        <div className="field"><span>変わるファイル（{pv.totals.files} 件{pv.totals.binary > 0 ? `・バイナリ ${pv.totals.binary}` : ""}）</span>
          <ul className="chg-list org-imp-files" aria-label="変わるファイル">
            {pv.files.map((f) => {
              const k = kindIcon(f);
              return (
                <li key={fileKey(f)} className="chg-row"><span className="chg-rowbtn">
                  <span className={`chg-k ${k.cls}`} title={k.title}><span aria-hidden="true">{k.letter}</span><span className="sr">{k.title}</span></span>
                  <span className="chg-nm"><span className="chg-name mono" title={pathText(f)}>{pathText(f)}</span></span>
                  <Counts f={f} /></span></li>);
            })}
          </ul>
          <div className="small muted">中身（プレビュー・差分）は、「成果物へ戻る」→ 各成果から確認できます。</div>
        </div>
      ) : null}
      {pv && pv.dirtyOther > 0 && pv.blockers.length === 0 ? <div className="note">あなたの未コミットの変更 {pv.dirtyOther} 件には触れません（そのまま残ります）。</div> : null}
      {runErr ? <div className="banner warn" role="alert"><div>{runErr}　（再実行はしていません。内容を確認し直してください）</div></div> : null}
      {rv ? <div className={`banner ${rv.tone === "ok" ? "info" : "warn"}`} role={rv.tone === "ok" ? "status" : "alert"}><div>{rv.text}</div></div> : null}
      {rec ? <RecordActions workflowId={workflowId} rec={rec} onChanged={reload} onPush={onPush} /> : null}
      <div className="note">worktree とブランチは取り込み後も残ります。フックは実行しません。取り込みコミットは常に作ります（履歴に取り込みの区切りが残ります）。</div>
      <FootActions
        left={<button type="button" className="btn" disabled={busy} onClick={onBack}>成果物へ戻る</button>}
        main={res ? <button type="button" className="btn primary" onClick={onClose}>閉じる</button>
          : <button type="button" className="btn primary" disabled={!ready} onClick={() => void run()}>{busy ? "取り込んでいます…" : "取り込む"}</button>} />
    </div>
  );
}

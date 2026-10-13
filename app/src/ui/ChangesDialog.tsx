// 変更を確認（見る→選ぶ→戻す を1画面で。#1・#2。DESIGN_CHANGES_UI）。外枠（Shell）と下部の閉じるボタンは Dialogs.tsx 側。
// 規則: 「このチャット」と「Git（HEAD比較）」は出所を分けて表示する。取れていない値は「未確認」と書き、0や空で代用しない。
//       戻す操作は利用者が押したときだけ（チェックの既定はすべてオフ）。確認の後だけ実行し、計画の期限切れは取り直して、判定が変わっていれば実行しない。
//       戻せない理由・要確認の理由は消さない。部分的にしか戻せなかったときは完了と書かない。通信断・取得不能を成功と書かない。
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import * as host from "../ipc/client";
import type { Chat, ChangedFile, ChangeList, ChangeSource, GitInfo, ListStatus, OpCapability, RepoBaselinePref, RevertPlan, RevertResult, SegmentStatus, TurnRecord, UnifiedDiff } from "../ipc/types";
import { showKnown } from "./format";
import { About, ConfirmBlock, FootActions } from "./DialogParts";
import { RepoRecordPanel, type SetRepoFn } from "./RepoRecordPanel";
import type { BaselineContext } from "./revertView";
import { failureText, gitKindOf, noBaselineReason, ownerCardShown, ownerExplainsNoBaseline, repoPanelVisible, sameReason } from "./revertView";
import { allState, confirmSummary, defaultRequest, DIFF_ROW_LIMIT, diffRows, fileKey, headline, keepChosen, mergeRows, planChanged, requestName, requestOptions, resultText, setChecked, splitPath, statusSlot, STATE_LABEL, STATE_MARK, toggleAll, type Row } from "./changesView";

const SOURCE_TEXT: Record<ChangeSource, string> = { baseline: "このチャット", git: "Git（HEAD比較）" };

const NOTE_BASELINE = "turnの開始時と終了時にAgentDockがGitで控えた内容の差です。Codexがコマンドで編集した変更も含みます（Codexの報告には依存しません）。Gitリポジトリのみで、.gitignore 対象・サブモジュール・作業フォルダ外は含みません。turnの最中に別のツールで変えた内容も含まれます。控えは自動削除されません。";
const NOTE_GIT = "Gitの作業ツリーとHEADの現在の差分です（turnの区切りはありません）。Gitリポジトリのみ。";
const NOTE_REVERT = "turnの開始時にAgentDockがGitで控えた内容へ、ファイルを書き戻します。現在の内容がこのチャットのturnによる変更だけのファイルはそのまま戻せます。turnの後や間に別の変更がある・終了時の控えがない・別の会話と同時に作業していたファイルは、理由を表示して止めます。内容を確認したうえで、ファイルごとに「それでも戻す」を選べます。コミットなどでHEADが変わった後のファイルは戻せません。戻す前の内容は、このチャット用の領域に控えとして保存します（自動では削除しません）。";
const UNVERIFIED_TEXT = "この操作は実際のCodexで動作を確認していません。表示は受け取った報告と、読み取ったファイルの内容だけに基づきます。";

/** 「記録しない」を選んだリポジトリの表示（専用の一行で見せるので、控えがない帯には重ねない）。 */
const SKIPPED_TEXT = failureText({ kind: "skipped" });
const errText = (e: unknown) => host.asIpcError(e).message;

function statusText(s: ListStatus): string | null {
  switch (s.kind) {
    case "ready": return null;
    case "notFetched": return `取得できませんでした: ${s.message}`;
    case "notSupported": return `非対応: ${s.message}`;
  }
}

/** 控えがない理由の判定材料（設定の控えの記録・作業フォルダのGit）。Gitは `git_info` の読取りだけ。設定が分からないとき（undefined）は一般文のまま。 */
function useGitInfo(live: boolean, chat: Chat | undefined): GitInfo | null {
  const [info, setInfo] = useState<GitInfo | null>(null);
  const cwd = chat?.cwd.kind === "value" ? chat.cwd.value : null;
  useEffect(() => {
    setInfo(null);
    if (!live || !cwd) return;
    let alive = true;
    host.gitInfo(cwd).then((i) => { if (alive) setInfo(i); }).catch(() => { if (alive) setInfo(null); });
    return () => { alive = false; };
  }, [live, cwd]);
  return info;
}

function baselineContextOf(info: GitInfo | null, baselinesEnabled: boolean | undefined): BaselineContext | undefined {
  return baselinesEnabled === undefined ? undefined : { baselinesEnabled, git: gitKindOf(info) };
}

export function UnverifiedNote({ cap }: { cap: OpCapability | undefined }) {
  if (cap?.verification !== "unverified") return null;
  return <p className="small muted"><span className="unv">未確認</span> {UNVERIFIED_TEXT}</p>;
}

/** 統一diffの表示。行番号（旧・新）と記号の列つき。本文は加工しない。巨大な差分は先頭だけ。 */
function DiffText({ text }: { text: string }) {
  const rows = diffRows(text);
  const shown = rows.length > DIFF_ROW_LIMIT ? rows.slice(0, DIFF_ROW_LIMIT) : rows;
  return (
    <div className="diffview" role="region" aria-label="差分" tabIndex={0}>
      {shown.map((r, i) => (
        <div key={i} className={`ln ${r.kind}`}>
          <span className="no" aria-hidden="true">{r.old ?? ""}</span><span className="no" aria-hidden="true">{r.new ?? ""}</span>
          <span className="sg" aria-hidden="true">{r.kind === "add" ? "+" : r.kind === "del" ? "−" : ""}</span>
          <span className="tx">{r.kind === "hunk" || r.kind === "meta" ? r.text : r.text === "" ? " " : r.text}</span>
        </div>))}
      {rows.length > shown.length ? <div className="ln meta"><span className="tx">先頭 {shown.length} 行のみ表示しています（全 {rows.length} 行）。</span></div> : null}
    </div>
  );
}

/** 種類アイコン（A/M/D/R）。色は追加=緑、削除=赤、変更=青、移動=灰。 */
function kindIcon(f: ChangedFile): { letter: string; cls: string; title: string } {
  if (f.moveTo) return { letter: "R", cls: "mov", title: "移動" };
  return f.kind === "added" ? { letter: "A", cls: "add", title: "追加" } : f.kind === "deleted" ? { letter: "D", cls: "del", title: "削除" } : { letter: "M", cls: "mod", title: "変更" };
}
const pathText = (f: ChangedFile) => `${f.path}${f.moveTo ? ` → ${f.moveTo}` : ""}`;
function Counts({ f }: { f: ChangedFile }) {
  if (f.binary) return <span className="chg-num muted">バイナリ</span>;
  return <span className="chg-num"><span className="chg-add">{showKnown(f.additions, (v) => `+${v}`)}</span> <span className="chg-del">{showKnown(f.deletions, (v) => `−${v}`)}</span></span>;
}

export interface ChangesProps {
  chat: Chat | undefined;
  live: boolean;
  /** 変更の報告が更新されるたびに増える（開いている表示が取り直す）。 */
  tick: number;
  /** 一覧・戻す操作の能力（どちらかが未確認なら見出しに「未確認」を出す）。 */
  caps: Array<OpCapability | undefined>;
  /** 依頼の表示名（発言の先頭）を作る材料。手元になければ `#n` だけで示す。 */
  turnRecords: TurnRecord[];
  /** 設定で控えの記録がオンか（控えがない理由の判定に使う）。 */
  baselinesEnabled?: boolean;
  /** リポジトリ別の選択（所有者違いの案内）。 */
  repoPrefs?: RepoBaselinePref[];
  onSetRepo?: SetRepoFn;
  /** 実行の完了を親へ伝える（トースト用）。結果が得られなければ null。呼ぶたびに親が差分表示の再取得を促す。 */
  onDone: (r: RevertResult | null) => void;
}

export function ChangesPanel({ chat, live, tick, caps, turnRecords, baselinesEnabled, repoPrefs, onSetRepo, onDone }: ChangesProps) {
  const [tab, setTab] = useState<ChangeSource>("baseline");
  /** 依頼。null＝未決定（依頼の一覧を読むまで）、""＝すべての依頼。 */
  const [turn, setTurn] = useState<string | null>(null);
  const [segs, setSegs] = useState<SegmentStatus[] | null>(null);
  const [list, setList] = useState<ChangeList | null>(null);
  const [listErr, setListErr] = useState<string | null>(null);
  const [plan, setPlan] = useState<RevertPlan | null>(null);
  const [planErr, setPlanErr] = useState<string | null>(null);
  const [chosen, setChosen] = useState<Record<string, boolean>>({});
  const [sel, setSel] = useState<string | null>(null);
  const [diff, setDiff] = useState<UnifiedDiff | null>(null);
  const [diffErr, setDiffErr] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<RevertResult | null>(null);
  const [resultErr, setResultErr] = useState<string | null>(null);
  const [changedPaths, setChangedPaths] = useState<string[]>([]);
  const [nonce, setNonce] = useState(0);
  /** 狭い画面で表示する側（一覧 or 差分）。 */
  const [pane, setPane] = useState<"list" | "diff">("list");
  const planRef = useRef<RevertPlan | null>(null);
  planRef.current = plan;
  const chatKey = chat?.key;
  const chatId = chatKey?.id;
  const gitInfo = useGitInfo(live, chat);
  const bctx = baselineContextOf(gitInfo, baselinesEnabled);
  /** 作業フォルダのリポジトリが所有者違いでGitが拒否している（案内を出すリポジトリの場所）。 */
  const ownerRepo = gitInfo?.ownerMismatch ? gitInfo.root ?? "" : null;
  const cwd = chat?.cwd.kind === "value" ? chat.cwd.value : null;

  // 依頼の一覧（控えの区間）。最初に読めたとき、最新の依頼を選ぶ。
  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    host.getBaselineStatus(chatKey).then((s) => {
      if (!alive) return;
      setSegs(s);
      setTurn((t) => t ?? defaultRequest(requestOptions(s, turnRecords)) ?? "");
    }).catch(() => { if (alive) { setSegs(null); setTurn((t) => t ?? ""); } });
    return () => { alive = false; };
  }, [live, chatId, tick, nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  // ファイル一覧 → 戻せるかの判定（読むだけ。ファイルは変えない）。
  useEffect(() => {
    if (!live || !chatKey) return;
    if (tab === "baseline" && turn === null) return;
    let alive = true;
    setListErr(null);
    const scope = tab === "baseline" && turn ? { kind: "turn" as const, turn } : { kind: "chat" as const };
    void (async () => {
      let l: ChangeList;
      try { l = await host.getChangeList(chatKey, scope, tab); } catch (e) {
        if (alive) { setList(null); setPlan(null); setPlanErr(null); setChosen({}); setListErr(errText(e)); }
        return;
      }
      if (!alive) return;
      setList(l);
      setSel((s) => (s && l.files.some((f) => fileKey(f) === s) ? s : l.files[0] ? fileKey(l.files[0]) : null));
      if (tab !== "baseline" || l.status.kind !== "ready" || l.files.length === 0) { setPlan(null); setPlanErr(null); setChosen({}); return; }
      try {
        const p = await host.previewRevert(chatKey, turn || null, l.files.map((f) => f.path));
        if (!alive) return;
        setPlanErr(null);
        setChosen((c) => keepChosen(c, planRef.current, p));
        setPlan(p);
      } catch (e) { if (alive) { setPlan(null); setChosen({}); setPlanErr(errText(e)); } }
    })();
    return () => { alive = false; };
  }, [live, chatId, tab, turn, tick, nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  const file = list?.files.find((f) => fileKey(f) === sel) ?? null;
  useEffect(() => {
    setDiff(null); setDiffErr(null);
    if (!live || !chatKey || !file) return;
    let alive = true;
    const path = tab === "git" ? file.moveTo ?? file.path : file.path;
    host.getFileDiff(chatKey, path, tab, tab === "baseline" && turn ? turn : null).then((d) => { if (alive) setDiff(d); }).catch((e) => { if (alive) setDiffErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId, tab, turn, sel, tick, nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;

  const opts = segs ? requestOptions(segs, turnRecords) : [];
  const noBase = noBaselineReason(segs, bctx);
  const ownerCase = ownerCardShown(segs, ownerRepo, repoPrefs, cwd);
  const ownerPanel = repoPanelVisible(segs, ownerRepo, repoPrefs, cwd);
  const ownerExplained = tab === "baseline" && ownerCase && ownerExplainsNoBaseline(segs, bctx);
  const hideStatus = tab === "baseline" && ownerCase && list?.status.kind === "notSupported";
  // 同じ原因（控えがない）の帯は1本にする。文が一致したときだけ重複とみなし、原因が違うときは両方を1行に並べる。
  const sameCause = list?.status.kind === "notSupported" && sameReason(list.status.message, noBase);
  const notSup = list?.status.kind === "notSupported" && !hideStatus ? statusText(list.status) : null;
  const baseParts = [
    tab === "baseline" && noBase && !sameCause && !ownerExplained && noBase !== SKIPPED_TEXT ? noBase : null,
    notSup,
  ].filter((x): x is string => !!x);
  const baseText = tab === "git" && ownerPanel ? null : baseParts.length > 0 ? baseParts.join(" ／ ") : null;
  const listError = listErr ?? (list?.status.kind === "notFetched" ? statusText(list.status) : null);

  // 控えがない・所有者違いで取れていないときは、チェック列・主ボタンを出さない。
  const canRevert = tab === "baseline" && !noBase && !!plan;
  const rows = list ? mergeRows(list.files, tab === "baseline" ? plan : null, chosen, result) : [];
  const row = rows.find((r) => r.key === sel) ?? null;
  const summary = plan ? confirmSummary(plan.items, chosen) : null;
  const busy = running;
  const lockedUi = running || confirm;
  const head = list ? headline(list.files) : null;
  const slot = statusSlot({
    listError, changedPaths, baseText, ownerPanel,
    planError: planErr ? `戻せるか確認できませんでした: ${planErr}` : null, result, resultError: resultErr ? `戻す操作でエラーになりました: ${resultErr}。実際の状態は「再確認」で取り直してください。` : null,
    notes: list?.notes ?? [],
  });
  const recheck = () => { setResult(null); setResultErr(null); setChangedPaths([]); setConfirm(false); setNonce((n) => n + 1); };
  const resetView = () => { setSel(null); setChosen({}); setConfirm(false); setResult(null); setResultErr(null); setChangedPaths([]); setPane("list"); };
  const unverified = caps.some((c) => c?.verification === "unverified");

  const run = async () => {
    if (!chatKey || !plan || !list || !summary) return;
    setRunning(true); setResultErr(null);
    let attempted = false;
    let r: RevertResult | null = null;
    try {
      let p = plan;
      if (Date.now() >= p.expiresAt) {
        // 計画の期限切れ: 同じファイルで取り直し、選んだファイルの判定が1件でも変わっていたら実行しない。
        const np = await host.previewRevert(chatKey, turn || null, list.files.map((f) => f.path));
        const changed = planChanged(p, np, summary.paths);
        setPlan(np);
        if (changed.length > 0) { setChosen(keepChosen(chosen, p, np)); setChangedPaths(changed); return; }
        p = np;
      }
      const s = confirmSummary(p.items, chosen);
      attempted = true;
      r = await host.revertChanges(chatKey, p.id, s.paths, s.forced);
      setResult(r); setChangedPaths([]);
    } catch (e) { setResultErr(errText(e)); } finally {
      // 成功・失敗・例外のどれでも、実際の状態を取り直す（一覧・計画を古いまま残さない）。選択は戻さない。
      setConfirm(false); setChosen({}); setRunning(false); setNonce((n) => n + 1);
      if (attempted) onDone(r);
    }
  };

  const onListKey = (e: KeyboardEvent<HTMLUListElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if (e.target instanceof HTMLInputElement) return;
    e.preventDefault();
    const i = rows.findIndex((r) => r.key === sel);
    const n = Math.min(rows.length - 1, Math.max(0, i + (e.key === "ArrowDown" ? 1 : -1)));
    const next = rows[n];
    if (!next) return;
    setSel(next.key);
    e.currentTarget.querySelectorAll<HTMLElement>("button.chg-rowbtn")[n]?.focus();
  };
  const onRowKey = (e: KeyboardEvent<HTMLButtonElement>, r: Row) => {
    if (e.key === " " && r.selectable && canRevert && !lockedUi) { e.preventDefault(); setChosen((c) => setChecked(rows, c, r.file.path, !r.checked)); }
  };

  const all = allState(rows);
  const showChecks = canRevert && tab === "baseline";

  return (
    <>
      <div className="content chg-content">
        <div className="chg-head">
          {tab === "baseline" ? (
            <label className="chg-req"><span className="small muted">依頼</span>
              <select value={turn ?? ""} disabled={lockedUi} aria-label="依頼" onChange={(e) => { setTurn(e.target.value); resetView(); }}>
                <option value="">すべての依頼</option>
                {opts.map((o) => <option key={o.turn} value={o.turn} disabled={o.disabled} title={o.turn}>{o.label}</option>)}
              </select></label>) : <span className="chg-req small muted">Gitの作業ツリーとHEADの現在の差分</span>}
          <span className="chg-count small">{head && list?.status.kind === "ready" ? (
            <><b>{head.count}</b> ファイル
              {head.additions !== null && head.deletions !== null ? <>　<span className="chg-add">+{head.additions}</span> <span className="chg-del">−{head.deletions}</span></> : <span className="muted">　行数は未確認</span>}</>
          ) : " "}</span>
          <div className="seg" role="tablist" aria-label="出所">
            {(["baseline", "git"] as ChangeSource[]).map((s) => <button key={s} role="tab" aria-selected={tab === s} disabled={lockedUi} onClick={() => { setTab(s); resetView(); }}>{SOURCE_TEXT[s]}</button>)}
          </div>
          {unverified ? <span className="unv" title={UNVERIFIED_TEXT}>未確認</span> : null}
        </div>
        <div className="chg-slot">{slot ? (
          <>
            {slot.kind === "owner" ? <RepoRecordPanel chat={chat} segs={segs} prefs={repoPrefs} onSet={onSetRepo} ownerRepo={ownerRepo} compact />
              : slot.kind === "notes" ? <p className="small muted">{slot.text}</p>
                : slot.kind === "result" ? (
                  <div className={`banner ${slot.tone}`} role="status"><div>{resultText(slot.result)}
                    {slot.result.backupDir ? <span className="small muted" title={slot.result.backupDir}>　戻す前の内容: <span className="mono">{slot.result.backupDir}</span></span> : null}</div></div>)
                  : slot.kind === "changed" ? (
                    <div className="banner warn" role="alert"><div>確認の間に判定が変わったため、実行しませんでした（{slot.paths.length} 件）。強調した行を確認してください。</div>
                      <button className="btn-line" onClick={recheck}>再確認</button></div>)
                    : (
                      <div className={`banner ${slot.tone}`} role={slot.tone === "err" ? "alert" : "status"}><div>{slot.text}</div>
                        {slot.kind === "listError" || slot.kind === "planError" || slot.kind === "resultError" ? <button className="btn-line" disabled={busy} onClick={recheck}>再確認</button> : null}</div>)}
          </>) : null}</div>

        {list && list.files.length > 0 ? (
          <div className="chg-split" data-pane={pane}>
            <div className="chg-left">
              {showChecks ? (
                <label className="chg-all"><input type="checkbox" disabled={lockedUi || all === "empty"} checked={all === "all"} ref={(el) => { if (el) el.indeterminate = all === "some"; }}
                  onChange={() => setChosen((c) => toggleAll(rows, c))} /> すべて</label>) : null}
              <ul className="chg-list" aria-label="変更されたファイル" onKeyDown={onListKey}>
                {rows.map((r) => {
                  const k = kindIcon(r.file);
                  const sp = splitPath(r.file.path);
                  const why = r.reasons.map((x) => `${x.label}: ${x.message}`).join("\n");
                  return (
                    <li key={r.key} className={`chg-row ${r.state}${r.key === sel ? " sel" : ""}${changedPaths.includes(r.file.path) ? " changed" : ""}`}>
                      {showChecks ? (r.selectable
                        ? <input type="checkbox" className="chg-ck" aria-label={`${r.file.path} を戻す`} checked={r.checked} disabled={lockedUi} onChange={(e) => setChosen((c) => setChecked(rows, c, r.file.path, e.target.checked))} />
                        : <span className="chg-ck" role="img" aria-label={r.state === "needsOverride" ? (r.checked ? "それでも戻す対象に入っています" : "一覧では選べません") : "戻す対象にできません"}>{r.state === "needsOverride" && r.checked ? "☑" : ""}</span>) : null}
                      <button className="chg-rowbtn" aria-current={r.key === sel} title={why ? `${pathText(r.file)}
${why}` : pathText(r.file)} onKeyDown={(e) => onRowKey(e, r)} onClick={() => { setSel(r.key); setPane("diff"); }}>
                        <span className={`chg-k ${k.cls}`} title={k.title}><span aria-hidden="true">{k.letter}</span><span className="sr">{k.title}</span></span>
                        <span className="chg-nm"><span className="chg-name mono">{sp.name}{r.file.moveTo ? ` → ${splitPath(r.file.moveTo).name}` : ""}</span>{sp.dir ? <span className="chg-dir muted">{sp.dir}</span> : null}</span>
                        <Counts f={r.file} />
                        <span className="chg-stc">{r.state !== "revertible" ? <span className={`tag chg-st ${r.state}`}><span aria-hidden="true">{STATE_MARK[r.state]} </span>{STATE_LABEL[r.state]}</span> : null}
                          {r.includes.length > 0 ? <span className="chg-inc" aria-label={`後の依頼の変更も戻ります（${r.includes.length}）`}>↺{r.includes.length}</span> : null}</span>
                      </button>
                    </li>);
                })}
              </ul>
            </div>
            <div className="chg-right">
              <button className="btn subtle chg-back" onClick={() => setPane("list")}>← 一覧</button>
              {file && row ? (
                <>
                  <div className="chg-fh"><span className={`chg-k ${kindIcon(file).cls}`} aria-label={kindIcon(file).title}>{kindIcon(file).letter}</span> <span className="mono chg-path">{pathText(file)}</span> <Counts f={file} /></div>
                  {row.reasons.length > 0 ? (
                    <details className="chg-why">
                      <summary>{row.reasons.map((x, i) => <span key={i} className={`tag ${row.state === "needsOverride" ? "need" : "block"}`}><span aria-hidden="true">{STATE_MARK[row.state]} </span>{x.label}</span>)}</summary>
                      {row.reasons.map((x, i) => <div key={i} className="small muted">・{x.label}: {x.message}</div>)}
                      {row.state === "blocked" ? <div className="small muted">強制しても戻せません。</div> : null}
                    </details>) : row.state === "unknown" && tab === "baseline" ? <p className="small muted"><span className="tag"><span aria-hidden="true">？ </span>未確認</span> 戻せるかを確認できていません。</p> : null}
                  {row.state === "needsOverride" && showChecks ? (
                    <label className="small chg-force"><input type="checkbox" checked={row.checked} disabled={lockedUi} onChange={(e) => setChosen((c) => setChecked(rows, c, file.path, e.target.checked))} /> それでも戻す</label>) : null}
                  {row.includes.length > 0 ? <p className="small chg-includes" title={row.includes.map((t) => requestName(t, opts)).join("\n")}>↺ 後の依頼の変更も戻ります（{row.includes.length}）</p> : null}
                  {file.binary ? <p>バイナリのため表示できません。</p> : diffErr ? <p style={{ color: "var(--fail)" }}>{diffErr}</p> : !diff ? <p className="small muted">差分を取得しています…</p>
                    : diff.status.kind !== "ready" ? <p className="banner warn" role="status"><span>{statusText(diff.status)}</span></p>
                      : diff.text.trim() === "" ? <p>差分の本文はありません（空）。</p> : <DiffText text={diff.text} />}
                </>) : <p className="small muted">ファイルを選ぶと差分を表示します。</p>}
            </div>
          </div>
        ) : <div className="chg-empty small muted" role="status">{!list ? (listErr ? "一覧を表示できません" : "取得しています…") : list.status.kind !== "ready" ? "表示できる変更はありません" : tab === "git" ? "Git上の変更はありません（取得済み）" : noBase ? "控えがないため、表示できる変更はありません" : "記録した変更はありません"}</div>}

        {confirm && summary && plan ? (
          <ConfirmBlock label="戻す確認" actions={<>
            <button className="btn" disabled={running} onClick={() => setConfirm(false)}>やめる</button>
            <button className="btn danger" disabled={running} onClick={() => void run()}>{running ? "戻しています…" : summary.actionLabel}</button></>}>
            <p>次の {summary.total} 件を{turn ? `「${requestName(turn, opts)}」の前の内容` : "このチャットの変更の前の内容"}に戻します（戻す前の内容は控えに保存します）。</p>
            {summary.normal.length > 0 ? (
              <ul className="check-list">{summary.normal.map((n) => (
                <li key={n.path}><span className="mono">{n.path}</span>{n.includes > 0 ? <span title="後の依頼の変更も戻ります">　↺{n.includes}</span> : null}{n.deletes ? "（削除されます）" : ""}</li>))}</ul>) : null}
            {summary.override.length > 0 ? (
              <div className="chg-override">
                <p><b>◆ 別の変更も消えます（{summary.override.length}）</b></p>
                <ul className="check-list">{summary.override.map((o) => (
                  <li key={o.path}><span className="mono">{o.path}</span>{o.reasons.map((r, k) => <div key={k} className="small">・{r.label}: {r.message}</div>)}</li>))}</ul>
                <p className="small">これらのファイルでは、別の会話・エディタによる変更や時期の分からない変更も消えます。</p>
              </div>) : null}
          </ConfirmBlock>) : null}

        <About><p>{tab === "baseline" ? NOTE_BASELINE : NOTE_GIT}</p>{tab === "baseline" ? <p>{NOTE_REVERT}</p> : null}</About>
      </div>
      <FootActions
        left={canRevert ? <button className="btn" disabled={lockedUi} onClick={recheck}>再確認</button> : null}
        main={canRevert && summary && summary.total > 0 && !confirm ? <button className="btn primary" disabled={lockedUi} onClick={() => setConfirm(true)}>{summary.total} 件を戻す…</button> : null} />
    </>
  );
}

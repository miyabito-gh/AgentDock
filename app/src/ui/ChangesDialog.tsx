// 変更ファイルと差分の表示（#1）と「変更を戻す」（#2）の本体（段階③ P3-1）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 「turnの変更（控え基準）」と「Git上の現在の差分」は出所を分けて表示する。取れていない値は「未確認」と書き、0や空で代用しない。
//       戻す操作は確認の後だけ。戻せない理由はそのまま出し、部分的にしか戻せなかったときは完了と書かない。
import { useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, ChangeKind, ChangeList, ChangedFile, ChangeSource, ListStatus, OpCapability, RevertPlan, RevertResult, SegmentStatus, UnifiedDiff } from "../ipc/types";
import { chatName } from "./derive";
import { showKnown } from "./format";
import { forcedPaths, groupOf, initialChosen, noBaselineReason, pickedPaths, reasonLines, resultKind, sortedItems, turnOptions } from "./revertView";

const KIND_TEXT: Record<ChangeKind, string> = { added: "追加", deleted: "削除", modified: "変更" };
const SOURCE_TEXT: Record<ChangeSource, string> = { baseline: "turnの変更（控え基準）", git: "Git上の現在の差分（HEAD比較）" };

const NOTE_BASELINE = "turnの開始時と終了時にAgentDockがGitで控えた内容の差です。Codexがコマンドで編集した変更も含みます（Codexの報告には依存しません）。Gitリポジトリのみで、.gitignore 対象・サブモジュール・作業フォルダ外は含みません。turnの最中に別のツールで変えた内容も含まれます。コミット・ブランチ切替の後は控えへ戻せません。控えは自動削除されません。";
const NOTE_GIT = "Gitの作業ツリーとHEADの現在の差分です（turnの区切りはありません）。Gitリポジトリのみ。";

const fileKey = (f: ChangedFile) => `${f.path}|${f.moveTo ?? ""}`;
const errText = (e: unknown) => host.asIpcError(e).message;

function statusText(s: ListStatus): string | null {
  switch (s.kind) {
    case "ready": return null;
    case "notFetched": return `取得できませんでした: ${s.message}`;
    case "notSupported": return `非対応: ${s.message}`;
  }
}

export function UnverifiedNote({ cap }: { cap: OpCapability | undefined }) {
  if (cap?.verification !== "unverified") return null;
  return <p className="small muted"><span className="tag unv">未確認</span> この操作は実際のCodexで動作を確認していません。表示は受け取った報告と、読み取ったファイルの内容だけに基づきます。</p>;
}

/** 統一diffの表示。行頭で色分けするだけで、内容は加工しない。 */
function DiffText({ text }: { text: string }) {
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return (
    <div className="diffview" role="region" aria-label="差分" tabIndex={0}>
      {lines.map((l, i) => {
        const c = l.startsWith("@@") ? "hunk" : l.startsWith("#") ? "meta" : l.startsWith("+") && !l.startsWith("+++") ? "add" : l.startsWith("-") && !l.startsWith("---") ? "del" : "";
        return <div key={i} className={`ln ${c}`}>{l === "" ? " " : l}</div>;
      })}
    </div>
  );
}

export interface ChangesProps {
  chat: Chat | undefined;
  live: boolean;
  /** 変更の報告が更新されるたびに増える（開いている表示が取り直す）。 */
  tick: number;
  cap: OpCapability | undefined;
  onRevert: () => void;
}

export function ChangesBody({ chat, live, tick, cap, onRevert }: ChangesProps) {
  const [tab, setTab] = useState<ChangeSource>("baseline");
  const [turn, setTurn] = useState("");
  const [segs, setSegs] = useState<SegmentStatus[] | null>(null);
  const [list, setList] = useState<ChangeList | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [diff, setDiff] = useState<UnifiedDiff | null>(null);
  const [diffErr, setDiffErr] = useState<string | null>(null);
  const chatKey = chat?.key;
  const chatId = chatKey?.id;

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    const scope = tab === "baseline" && turn ? { kind: "turn" as const, turn } : { kind: "chat" as const };
    setErr(null);
    host.getChangeList(chatKey, scope, tab).then((l) => {
      if (!alive) return;
      setList(l);
      setSel((s) => (s && l.files.some((f) => fileKey(f) === s) ? s : null));
    }).catch((e) => { if (alive) { setList(null); setErr(errText(e)); } });
    return () => { alive = false; };
  }, [live, chatId, tab, turn, tick]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    host.getBaselineStatus(chatKey).then((s) => { if (alive) setSegs(s); }).catch(() => { if (alive) setSegs(null); });
    return () => { alive = false; };
  }, [live, chatId, tick]); // eslint-disable-line react-hooks/exhaustive-deps

  const file = list?.files.find((f) => fileKey(f) === sel) ?? null;
  useEffect(() => {
    setDiff(null); setDiffErr(null);
    if (!live || !chatKey || !file) return;
    let alive = true;
    const path = tab === "git" ? file.moveTo ?? file.path : file.path;
    host.getFileDiff(chatKey, path, tab, tab === "baseline" && turn ? turn : null).then((d) => { if (alive) setDiff(d); }).catch((e) => { if (alive) setDiffErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId, tab, turn, sel, tick]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const st = list ? statusText(list.status) : null;
  const turns = segs ? turnOptions(segs) : [];
  const noBase = noBaselineReason(segs);
  return (
    <>
      <div className="tabs" role="tablist">
        {(["baseline", "git"] as ChangeSource[]).map((s) => <button key={s} role="tab" aria-selected={tab === s} onClick={() => { setTab(s); setSel(null); }}>{SOURCE_TEXT[s]}</button>)}
      </div>
      <div className="content">
        <p className="small muted">{chatName(chat)}</p>
        <UnverifiedNote cap={cap} />
        <p className="small muted">{tab === "baseline" ? NOTE_BASELINE : NOTE_GIT}</p>
        {tab === "baseline" && noBase ? <p className="gbanner warn" role="status">{noBase}</p> : null}
        {tab === "baseline" && turns.length > 0 ? (
          <div className="field"><span>対象</span>
            <select value={turn} onChange={(e) => setTurn(e.target.value)} aria-label="対象のturn">
              <option value="">このチャットの控えのある変更すべて</option>
              {turns.map((t) => <option key={t.turn} value={t.turn}>{t.label}</option>)}
            </select></div>) : null}
        {err ? <p style={{ color: "var(--fail)" }}>{err}</p> : !list ? <p>取得しています…</p> : (
          <>
            {st ? <p className="gbanner warn" role="status">{st}</p> : null}
            {list.notes.map((n, i) => <p key={i} className="small muted">{n}</p>)}
            {list.status.kind === "ready" && list.files.length === 0 ? <p>{tab === "git" ? "Git上の変更はありません（取得済み）。" : noBase ? "控えがないため、表示できる変更はありません。" : "控え基準の変更はありません。"}</p> : null}
            {list.files.length > 0 ? (
              <table className="parity-table change-table">
                <thead><tr><th>種類</th><th>ファイル</th><th>追加</th><th>削除</th><th>{tab === "git" ? "" : "turn"}</th></tr></thead>
                <tbody>
                  {list.files.map((f) => (
                    <tr key={fileKey(f)} className={fileKey(f) === sel ? "sel" : ""} onClick={() => setSel(fileKey(f))}>
                      <td>{f.moveTo ? "移動" : KIND_TEXT[f.kind]}</td>
                      <td className="mono"><button className="linkish" aria-pressed={fileKey(f) === sel} onClick={() => setSel(fileKey(f))}>{f.path}{f.moveTo ? ` → ${f.moveTo}` : ""}</button>{f.reverted ? <span className="tag"> 戻し済み</span> : null}</td>
                      {f.binary ? <td colSpan={2} className="small muted">バイナリ（行数なし）</td> : <>
                        <td>{showKnown(f.additions, (v) => `+${v}`)}</td>
                        <td>{showKnown(f.deletions, (v) => `−${v}`)}</td></>}
                      <td className="small">{f.turn ?? ""}</td>
                    </tr>))}
                </tbody>
              </table>) : null}
            {file ? (
              <div style={{ marginTop: 8 }}>
                <p className="small"><b>{SOURCE_TEXT[tab]}</b>: <span className="mono">{file.path}</span></p>
                {file.binary ? <p>バイナリのため表示できません。</p> : diffErr ? <p style={{ color: "var(--fail)" }}>{diffErr}</p> : !diff ? <p>差分を取得しています…</p>
                  : diff.status.kind !== "ready" ? <p className="gbanner warn" role="status">{statusText(diff.status)}</p>
                    : diff.text.trim() === "" ? <p>差分の本文はありません（空）。</p> : <DiffText text={diff.text} />}
              </div>) : list.files.length > 0 ? <p className="small muted">ファイルを選ぶと差分を表示します。</p> : null}
          </>)}
        {tab === "baseline" ? <div className="acts" style={{ marginTop: 10 }}><button className="btn-line" disabled={!!noBase} onClick={onRevert}>変更を戻す…</button>{noBase ? <span className="small muted">控えがないため戻せません。</span> : null}</div> : null}
      </div>
    </>
  );
}

// ───────────────────────────── 変更を戻す ─────────────────────────────

export interface RevertProps {
  chat: Chat | undefined;
  live: boolean;
  cap: OpCapability | undefined;
  /** 実行の完了を親へ伝える（トースト用。結果の本文はこのダイアログに残す）。結果が得られなければ null。呼ぶたびに親が差分表示の再取得を促す。 */
  onDone: (r: RevertResult | null) => void;
}

const GROUP_TAG = { revertible: "戻せる", needsOverride: "要確認", blocked: "止める" } as const;

export function RevertBody({ chat, live, cap, onDone }: RevertProps) {
  const [segs, setSegs] = useState<SegmentStatus[] | null>(null);
  const [from, setFrom] = useState("");
  const [plan, setPlan] = useState<RevertPlan | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [chosen, setChosen] = useState<Record<string, boolean>>({});
  /** 0: 確認なし、1: 1段目（書き換える件数）、2: 2段目（強制するファイルの確認）。 */
  const [stage, setStage] = useState<0 | 1 | 2>(0);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<RevertResult | null>(null);
  const [nonce, setNonce] = useState(0);
  const chatKey = chat?.key;
  const chatId = chatKey?.id;

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    host.getBaselineStatus(chatKey).then((s) => { if (alive) setSegs(s); }).catch(() => { if (alive) setSegs(null); });
    return () => { alive = false; };
  }, [live, chatId]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    setPlan(null); setErr(null); setStage(0); setResult(null);
    host.previewRevert(chatKey, from || null, null).then((p) => {
      if (!alive) return;
      setPlan(p);
      setChosen(initialChosen(p.items)); // 要確認は初期オフ
    }).catch((e) => { if (alive) setErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId, from, nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const items = plan ? sortedItems(plan.items) : [];
  const picked = plan ? pickedPaths(plan.items, chosen) : [];
  const forced = plan ? forcedPaths(plan.items, chosen) : [];
  const total = picked.length + forced.length;
  const forcedItems = items.filter((i) => forced.includes(i.path));
  const turns = segs ? turnOptions(segs) : [];
  const noBase = noBaselineReason(segs);
  const busy = running || stage !== 0;
  const run = async () => {
    if (!chatKey || !plan) return;
    setRunning(true); setErr(null);
    let r: RevertResult | null = null;
    try {
      r = await host.revertChanges(chatKey, plan.id, [...picked, ...forced], forced);
      setResult(r); setStage(0);
    } catch (e) { setErr(errText(e)); setStage(0); } finally {
      // 成功・失敗・一部のみのどれでも、実際の状態を取り直す（差分一覧の「戻し済み」・turn選択を古いまま残さない）。
      host.getBaselineStatus(chatKey).then(setSegs).catch(() => setSegs(null));
      onDone(r);
      setRunning(false);
    }
  };
  const kind = result ? resultKind(result) : null;
  return (
    <div className="content">
      <p><b>{chatName(chat)}</b> の変更を、turnの開始時の控えへ戻します。</p>
      <UnverifiedNote cap={cap} />
      <p className="small muted">turnの開始時にAgentDockがGitで控えた内容へ、ファイルを書き戻します。現在の内容がこのチャットのturnによる変更だけのファイルはそのまま戻せます。turnの後や間に別の変更がある・終了時の控えがない・別の会話と同時に作業していたファイルは、理由を表示して止めます。内容を確認したうえで、ファイルごとに「別の変更も含めて戻す」を選べます。コミットなどでHEADが変わった後のファイルは戻せません。戻す前の内容は、このチャット用の領域に控えとして保存します（自動では削除しません）。</p>
      {noBase ? <p className="gbanner warn" role="status">{noBase}</p> : null}
      {turns.length > 0 && !result ? (
        <div className="field"><span>戻す範囲</span>
          <select value={from} onChange={(e) => setFrom(e.target.value)} disabled={busy} aria-label="戻す範囲">
            <option value="">このチャットの控えのある変更すべて</option>
            {turns.map((t) => <option key={t.turn} value={t.turn}>{t.label} 以降の変更（同じファイルの、より後の変更も一緒に戻ります）</option>)}
          </select></div>) : null}
      {err ? <p className="gbanner err" role="alert">{err}</p> : null}
      {!plan && !err ? <p>戻せるか確認しています…（ファイルは変更しません）</p> : null}
      {plan && !result ? (
        plan.items.length === 0 ? <p>戻す対象の変更はありません（控えのある変更がない、または戻し済みです）。</p> : (
          <>
            <ul className="check-list" style={{ listStyle: "none", paddingLeft: 0 }}>
              {items.map((i) => {
                const g = groupOf(i.verdict);
                const reasons = reasonLines(i.verdict);
                return (
                  <li key={i.path} style={{ marginBottom: 8 }}>
                    <div><span className={`tag${g === "revertible" ? "" : " unv"}`}>{GROUP_TAG[g]}</span> <span className="mono">{i.path}</span></div>
                    {i.verdict.kind === "revertible" ? (
                      <label className="small"><input type="checkbox" checked={!!chosen[i.path]} disabled={busy} onChange={(e) => setChosen((c) => ({ ...c, [i.path]: e.target.checked }))} /> 戻す
                        <span className="muted"> — {i.verdict.summary}</span></label>
                    ) : null}
                    {g === "needsOverride" ? (
                      <label className="small"><input type="checkbox" checked={!!chosen[i.path]} disabled={busy} onChange={(e) => setChosen((c) => ({ ...c, [i.path]: e.target.checked }))} /> 別の変更も含めて控えの内容に戻す（確認が必要）</label>
                    ) : null}
                    {reasons.map((r, k) => <div key={k} className="small muted">・{r.label}: {r.message}</div>)}
                    {g === "blocked" ? <div className="small muted">強制しても戻せません。</div> : null}
                    {i.includesTurns.length > 0 ? <div className="small muted">一緒に戻る後続の変更: turn {i.includesTurns.join(", ")}</div> : null}
                  </li>);
              })}
            </ul>
            {stage === 1 ? (
              <div className="gbanner warn" role="alertdialog" aria-label="戻す確認">
                <span className="grow">{total} 件のファイルを書き換えます（戻す前の内容は控えに保存します）。{forced.length > 0 ? `うち ${forced.length} 件は「別の変更も含めて戻す」で、次の画面でもう一度確認します。` : ""}よろしいですか？</span>
                <button className="btn-danger" disabled={running} onClick={() => (forced.length > 0 ? setStage(2) : void run())}>{running ? "戻しています…" : forced.length > 0 ? "次へ" : "戻す"}</button>
                <button className="btn-line" disabled={running} onClick={() => setStage(0)}>やめる</button>
              </div>
            ) : stage === 2 ? (
              <div className="gbanner err" role="alertdialog" aria-label="別の変更も含めて戻す確認" style={{ display: "block" }}>
                <p><b>別の変更も含めて、{forcedItems.length} 件を控えの内容に戻します。</b></p>
                <ul className="check-list">{forcedItems.map((i) => (
                  <li key={i.path}><span className="mono">{i.path}</span>{reasonLines(i.verdict).map((r, k) => <div key={k} className="small">・{r.label}: {r.message}</div>)}</li>))}</ul>
                <p className="small">これらのファイルでは、別の会話・エディタによる変更や時期の分からない変更も消えます。ファイルは、選んだ区間の開始時の控えの内容になります。書き換える前の内容は控えに保存します（戻した後も控えから確認できます）。</p>
                <div className="acts">
                  <button className="btn-danger" disabled={running} onClick={() => void run()}>{running ? "戻しています…" : "別の変更も含めて戻す"}</button>
                  <button className="btn-line" disabled={running} onClick={() => setStage(0)}>やめる（何も変えません）</button>
                </div>
              </div>
            ) : (
              <div className="acts"><button className="btn-main" disabled={total === 0 || running} onClick={() => setStage(1)}>選んだ {total} 件を戻す…</button>
                <button className="btn-line" onClick={() => setNonce((n) => n + 1)}>計画を作り直す</button></div>)}
          </>)) : null}
      {result ? (
        <div role="status">
          {kind === "partial"
            ? <p className="gbanner err">一部のみ戻しました（戻せたもの {result.reverted.length} 件、戻せなかったもの {result.failed.length} 件）。完了ではありません。</p>
            : kind === "allReverted" ? <p className="gbanner warn">{result.reverted.length} 件のファイルを書き換えました。</p>
              : <p className="gbanner warn">書き換えたファイルはありません。</p>}
          {result.reverted.length > 0 ? <><p className="small">戻したファイル:</p><ul className="check-list">{result.reverted.map((p) => <li key={p} className="mono">{p}</li>)}</ul></> : null}
          {result.forced.length > 0 ? <><p className="small">うち、別の変更も含めて（強制で）戻したファイル:</p><ul className="check-list">{result.forced.map((p) => <li key={p} className="mono">{p}</li>)}</ul></> : null}
          {result.failed.length > 0 ? <><p className="small">戻せなかったファイル:</p><ul className="check-list">{result.failed.map((f) => <li key={f.path}><span className="mono">{f.path}</span><div className="small muted">{f.reason}</div></li>)}</ul></> : null}
          {result.backupDir ? <p className="small muted">戻す前の内容の控え: <span className="mono">{result.backupDir}</span></p> : null}
          <div className="acts"><button className="btn-line" onClick={() => setNonce((n) => n + 1)}>もう一度確認する</button></div>
        </div>) : null}
    </div>
  );
}

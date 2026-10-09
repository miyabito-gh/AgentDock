// 変更ファイルと差分の表示（#1）と「変更を戻す」（#2）の本体（段階③ P3-1）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 「Codexが報告した変更」と「Git上の現在の差分」は出所を分けて表示する。取れていない値は「未確認」と書き、0や空で代用しない。
//       戻す操作は確認の後だけ。戻せない理由はそのまま出し、部分的にしか戻せなかったときは完了と書かない。
import { useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, ChangeKind, ChangeList, ChangedFile, ChangeSource, ListStatus, OpCapability, RevertPlan, RevertResult, UnifiedDiff } from "../ipc/types";
import { chatName } from "./derive";
import { showKnown } from "./format";

const KIND_TEXT: Record<ChangeKind, string> = { added: "追加", deleted: "削除", modified: "変更" };
const SOURCE_TEXT: Record<ChangeSource, string> = { backendReported: "Codex報告", baseline: "turnの変更（控え基準）", git: "Git上の現在の差分" };

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
  const [tab, setTab] = useState<ChangeSource>("backendReported");
  const [turn, setTurn] = useState("");
  const [turns, setTurns] = useState<string[]>([]);
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
    const scope = tab === "git" ? { kind: "workingTree" as const } : turn ? { kind: "turn" as const, turn } : { kind: "chat" as const };
    setErr(null);
    host.getChangeList(chatKey, scope).then((l) => {
      if (!alive) return;
      setList(l);
      if (tab === "backendReported" && !turn) setTurns([...new Set(l.files.map((f) => f.turn).filter((t): t is string => !!t))]);
      setSel((s) => (s && l.files.some((f) => fileKey(f) === s) ? s : null));
    }).catch((e) => { if (alive) { setList(null); setErr(errText(e)); } });
    return () => { alive = false; };
  }, [live, chatId, tab, turn, tick]); // eslint-disable-line react-hooks/exhaustive-deps

  const file = list?.files.find((f) => fileKey(f) === sel) ?? null;
  useEffect(() => {
    setDiff(null); setDiffErr(null);
    if (!live || !chatKey || !file) return;
    let alive = true;
    const path = tab === "git" ? file.moveTo ?? file.path : file.path;
    host.getFileDiff(chatKey, path, tab, tab === "backendReported" && turn ? turn : null).then((d) => { if (alive) setDiff(d); }).catch((e) => { if (alive) setDiffErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId, tab, turn, sel, tick]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const st = list ? statusText(list.status) : null;
  return (
    <>
      <div className="tabs" role="tablist">
        {(["backendReported", "git"] as ChangeSource[]).map((s) => <button key={s} role="tab" aria-selected={tab === s} onClick={() => { setTab(s); setSel(null); }}>{SOURCE_TEXT[s]}</button>)}
      </div>
      <div className="content">
        <p className="small muted">{chatName(chat)}</p>
        <UnverifiedNote cap={cap} />
        {tab === "backendReported" && turns.length > 0 ? (
          <div className="field"><span>対象</span>
            <select value={turn} onChange={(e) => setTurn(e.target.value)} aria-label="対象のturn">
              <option value="">このチャットで観測した変更すべて</option>
              {turns.map((t) => <option key={t} value={t}>turn {t}</option>)}
            </select></div>) : null}
        {err ? <p style={{ color: "var(--fail)" }}>{err}</p> : !list ? <p>取得しています…</p> : (
          <>
            {st ? <p className="gbanner warn" role="status">{st}</p> : null}
            {list.notes.map((n, i) => <p key={i} className="small muted">{n}</p>)}
            {list.status.kind === "ready" && list.files.length === 0 ? <p>{tab === "git" ? "Git上の変更はありません（取得済み）。" : "観測した変更はありません。"}</p> : null}
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
        {tab === "backendReported" ? <div className="acts" style={{ marginTop: 10 }}><button className="btn-line" onClick={onRevert}>変更を戻す…</button></div> : null}
      </div>
    </>
  );
}

// ───────────────────────────── 変更を戻す ─────────────────────────────

export interface RevertProps {
  chat: Chat | undefined;
  live: boolean;
  cap: OpCapability | undefined;
  /** 実行の結果を親（ダイアログの外）へ伝える（トースト用。結果の本文はこのダイアログに残す）。 */
  onDone: (r: RevertResult) => void;
}

const BLOCK_LABEL: Record<string, string> = {
  notObserved: "観測していない", otherChatLater: "別の会話が後で変更", hashMismatch: "内容が観測直後と違う", hashUnknown: "観測直後の内容が未確認",
  missingNow: "ファイルがない", contextMismatch: "差分が当たらない", notText: "テキストではない", diffUnreadable: "差分を読めない",
  unsupported: "対象外", targetExists: "戻し先に別のファイル", pathUnresolved: "場所を特定できない", readFailed: "読み取れない",
};

export function RevertBody({ chat, live, cap, onDone }: RevertProps) {
  const [turns, setTurns] = useState<string[]>([]);
  const [from, setFrom] = useState("");
  const [plan, setPlan] = useState<RevertPlan | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [chosen, setChosen] = useState<Record<string, boolean>>({});
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<RevertResult | null>(null);
  const [nonce, setNonce] = useState(0);
  const chatKey = chat?.key;
  const chatId = chatKey?.id;

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    host.getChangeList(chatKey, { kind: "chat" }).then((l) => { if (alive) setTurns([...new Set(l.files.map((f) => f.turn).filter((t): t is string => !!t))]); }).catch(() => {});
    return () => { alive = false; };
  }, [live, chatId]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    setPlan(null); setErr(null); setConfirming(false); setResult(null);
    host.previewRevert(chatKey, from || null, null).then((p) => {
      if (!alive) return;
      setPlan(p);
      setChosen(Object.fromEntries(p.items.filter((i) => i.verdict.kind === "revertible").map((i) => [i.path, true])));
    }).catch((e) => { if (alive) setErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId, from, nonce]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const picked = plan ? plan.items.filter((i) => i.verdict.kind === "revertible" && chosen[i.path]).map((i) => i.path) : [];
  const run = async () => {
    if (!chatKey || !plan) return;
    setRunning(true); setErr(null);
    try {
      const r = await host.revertChanges(chatKey, plan.id, picked);
      setResult(r); setConfirming(false); onDone(r);
    } catch (e) { setErr(errText(e)); setConfirming(false); } finally { setRunning(false); }
  };
  return (
    <div className="content">
      <p><b>{chatName(chat)}</b> の、AgentDockが観測した変更を戻します。</p>
      <UnverifiedNote cap={cap} />
      <p className="small muted">戻せるのは、Codexが報告して AgentDock が観測した変更で、現在の内容が観測直後と一致するファイルだけです。コマンドの実行による変更、別の会話やエディタで後から変えられたファイルは戻せません（理由を表示して止めます）。戻す前の内容は、このチャット用の領域に控えとして保存します（自動では削除しません）。</p>
      {turns.length > 0 && !result ? (
        <div className="field"><span>戻す範囲</span>
          <select value={from} onChange={(e) => setFrom(e.target.value)} aria-label="戻す範囲">
            <option value="">このチャットで観測した変更すべて</option>
            {turns.map((t) => <option key={t} value={t}>turn {t} の変更（同じファイルの、より後の変更も一緒に戻ります）</option>)}
          </select></div>) : null}
      {err ? <p className="gbanner err" role="alert">{err}</p> : null}
      {!plan && !err ? <p>戻せるか確認しています…（ファイルは変更しません）</p> : null}
      {plan && !result ? (
        plan.items.length === 0 ? <p>戻す対象の変更はありません（観測した変更がない、または戻し済みです）。</p> : (
          <>
            <ul className="check-list" style={{ listStyle: "none", paddingLeft: 0 }}>
              {plan.items.map((i) => (
                <li key={i.path} style={{ marginBottom: 6 }}>
                  {i.verdict.kind === "revertible" ? (
                    <label><input type="checkbox" checked={!!chosen[i.path]} disabled={running || confirming} onChange={(e) => setChosen((c) => ({ ...c, [i.path]: e.target.checked }))} /> <span className="mono">{i.path}</span>
                      <span className="small muted"> — {i.verdict.summary}</span></label>
                  ) : (
                    <div><span className="mono">{i.path}</span> <span className="tag unv">戻せません: {i.verdict.kind === "blocked" ? (BLOCK_LABEL[i.verdict.code] ?? i.verdict.code) : "要確認"}</span>
                      <div className="small muted">{i.verdict.kind === "blocked" ? i.verdict.message : i.verdict.reasons.map((r) => r.message).join(" / ")}</div></div>
                  )}
                  {i.includesTurns.length > 0 ? <div className="small muted">一緒に戻る後続の変更: turn {i.includesTurns.join(", ")}</div> : null}
                </li>))}
            </ul>
            {confirming ? (
              <div className="gbanner warn" role="alertdialog" aria-label="戻す確認">
                <span className="grow">{picked.length} 件のファイルを書き換えます（戻す前の内容は控えに保存します）。よろしいですか？</span>
                <button className="btn-danger" disabled={running} onClick={() => void run()}>{running ? "戻しています…" : "戻す"}</button>
                <button className="btn-line" disabled={running} onClick={() => setConfirming(false)}>やめる</button>
              </div>
            ) : (
              <div className="acts"><button className="btn-main" disabled={picked.length === 0 || running} onClick={() => setConfirming(true)}>選んだ {picked.length} 件を戻す…</button>
                <button className="btn-line" onClick={() => setNonce((n) => n + 1)}>計画を作り直す</button></div>)}
          </>)) : null}
      {result ? (
        <div role="status">
          {result.failed.length > 0
            ? <p className="gbanner err">一部のみ戻しました（戻せたもの {result.reverted.length} 件、戻せなかったもの {result.failed.length} 件）。完了ではありません。</p>
            : <p className="gbanner warn">{result.reverted.length} 件のファイルを書き換えました。</p>}
          {result.reverted.length > 0 ? <><p className="small">戻したファイル:</p><ul className="check-list">{result.reverted.map((p) => <li key={p} className="mono">{p}</li>)}</ul></> : null}
          {result.failed.length > 0 ? <><p className="small">戻せなかったファイル:</p><ul className="check-list">{result.failed.map((f) => <li key={f.path}><span className="mono">{f.path}</span><div className="small muted">{f.reason}</div></li>)}</ul></> : null}
          {result.backupDir ? <p className="small muted">戻す前の内容の控え: <span className="mono">{result.backupDir}</span></p> : null}
          <div className="acts"><button className="btn-line" onClick={() => setNonce((n) => n + 1)}>もう一度確認する</button></div>
        </div>) : null}
    </div>
  );
}

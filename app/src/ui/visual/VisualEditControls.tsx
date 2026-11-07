import { useEffect, useRef, useState } from "react";
import type { ChatKey } from "../../ipc/types";
import { statusOf } from "./composerHint";
import { directRefusal, friendlyVisualError, visualDirectDiff, visualDirectRedo, visualDirectTurns, visualDirectUndo, type DirectDiff, type DirectTurn, type PreviewSession } from "./api";

const INTRO_KEY = "agentdock.visualDirectIntro";
const introSeen = (): boolean => { try { return window.localStorage.getItem(INTRO_KEY) === "1"; } catch { return true; } };
const rawMessageOf = (error: unknown): string => error instanceof Error ? error.message : typeof error === "string" ? error
  : error && typeof error === "object" && "message" in error && typeof (error as { message: unknown }).message === "string" ? (error as { message: string }).message : "";
const messageOf = (error: unknown): string => friendlyVisualError(rawMessageOf(error) || "不明なエラー");
const changeLabel = (kind: string) => kind === "add" ? "追加" : kind === "delete" ? "削除" : "変更";

/** 直接編集の状態バー。直前の依頼での変更件数と、［戻す］［やり直す］［差分］。実行中は戻す・やり直すを無効にする。 */
export function VisualEditControls({ preview, turns, onTurns, running, chat, onBeforeDiff, onAfterDiff }: {
  /** onTurns は、どのHTMLの一覧かを添えて呼ぶ（呼び出し側は、今表示中のHTMLと一致するときだけ反映する）。 */
  preview: PreviewSession | null; turns: DirectTurn[]; onTurns: (rootPath: string, htmlPath: string, turns: DirectTurn[]) => void;
  /** 編集に使う会話が実行中か（実行中は戻す・やり直すを出さない）。 */ running: boolean; chat: ChatKey | null;
  onBeforeDiff: () => Promise<void>; onAfterDiff: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [intro, setIntro] = useState(() => !introSeen());
  const [last, setLast] = useState<{ kind: "undo" | "redo"; turnId: string } | null>(null);
  const [confirm, setConfirm] = useState<{ kind: "undo" | "redo"; target: DirectTurn; paths: string[] } | null>(null);
  const [diffTurn, setDiffTurn] = useState<DirectTurn | null>(null);
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [diff, setDiff] = useState<DirectDiff | null>(null);
  const [diffBusy, setDiffBusy] = useState(false);
  const lock = useRef(false);
  const diffOpen = useRef(false);
  diffOpen.current = diffTurn !== null;
  const rootKey = preview ? preview.rootPath + "|" + preview.htmlPath : "";
  const closeDiff = () => { setDiffTurn(null); setDiffPath(null); setDiff(null); onAfterDiff(); };
  // 表示するHTMLが変わったら、状態を持ち越さない。差分を開いていたら、プレビューの退避も解除する。
  useEffect(() => {
    setError(null); setLast(null); setConfirm(null); setDiffTurn(null); setDiffPath(null); setDiff(null);
    if (diffOpen.current) onAfterDiff();
  }, [rootKey]);
  useEffect(() => () => { onAfterDiff(); }, []);
  useEffect(() => {
    if (!diffTurn && !confirm) return;
    const h = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); if (confirm) setConfirm(null); else closeDiff(); } };
    document.addEventListener("keydown", h, true);
    return () => document.removeEventListener("keydown", h, true);
  }, [!!diffTurn, !!confirm]);
  const hasTurns = turns.length > 0;
  const dismissIntro = () => { setIntro(false); try { window.localStorage.setItem(INTRO_KEY, "1"); } catch { /* 保存できなくても動く */ } };
  // 最初の変更が出たら、案内は役目を終えたとして閉じる。
  useEffect(() => { if (hasTurns && intro) dismissIntro(); }, [hasTurns]);
  if (!preview) return null;

  const undoTurn = [...turns].reverse().find((t) => t.canUndo) ?? null;
  const redoTurn = turns.find((t) => t.canRedo) ?? null;
  const { text: status, turn: shown } = statusOf(turns, running, last);

  const refresh = async () => {
    try { onTurns(preview.rootPath, preview.htmlPath, await visualDirectTurns(preview.rootPath, preview.htmlPath)); }
    catch { /* 取れなければ表示はそのまま */ }
  };
  const run = async (kind: "undo" | "redo", target: DirectTurn, force = false): Promise<void> => {
    if (lock.current || !chat) return;
    lock.current = true; setBusy(true); setError(null); setConfirm(null);
    const label = kind === "undo" ? "戻せませんでした: " : "やり直せませんでした: ";
    let failed = false;
    try {
      const next = await (kind === "undo" ? visualDirectUndo : visualDirectRedo)(preview.rootPath, preview.htmlPath, target.id, force, chat);
      onTurns(preview.rootPath, preview.htmlPath, next);
      setLast({ kind, turnId: target.id });
    } catch (e) {
      failed = true;
      const refusal = directRefusal(rawMessageOf(e));
      if (refusal?.kind === "conflict" && !force) setConfirm({ kind, target, paths: refusal.paths });
      else setError(label + messageOf(e));
    } finally { lock.current = false; setBusy(false); }
    // 失敗したときは、画面の一覧が古い可能性があるので必ず取り直す。
    if (failed) await refresh();
  };
  const readDiff = async (turn: DirectTurn, path: string) => {
    if (diffBusy) return;
    setDiffBusy(true); setError(null);
    try {
      if (!diffTurn) await onBeforeDiff();
      setDiffTurn(turn); setDiffPath(path); setDiff(null);
      setDiff(await visualDirectDiff(preview.rootPath, preview.htmlPath, turn.id, path));
    } catch (e) {
      setError("差分を読めませんでした: " + messageOf(e));
      if (!diffTurn) { setDiffTurn(null); setDiffPath(null); onAfterDiff(); }
    } finally { setDiffBusy(false); }
  };

  return <section className="vd-bar" aria-label="直接編集">
    <div className="vd-row" role="status">
      <span className="vd-text">{hasTurns ? status : intro ? "元のファイルを直接書き換えます。変更は［戻す］で取り消せます。" : status}</span>
      {hasTurns ? <span className="vd-acts">
        <button type="button" className="btn-line sm" disabled={busy || running || !chat || !undoTurn} title={running ? "会話の実行中は戻せません" : "直前の依頼での変更を戻します（編集範囲の外のファイルや新規ファイルは戻せません）"} onClick={() => undoTurn && void run("undo", undoTurn)}>戻す</button>
        <button type="button" className="btn-line sm" disabled={busy || running || !chat || !redoTurn} title={running ? "会話の実行中はやり直せません" : "戻した変更をやり直します"} onClick={() => redoTurn && void run("redo", redoTurn)}>やり直す</button>
        <button type="button" className="btn-line sm" disabled={busy || diffBusy || !shown || !shown.files.length} title="直前の依頼での変更を、変更前と並べて見ます" onClick={() => shown && shown.files.length && void readDiff(shown, shown.files[0].path)}>差分</button>
      </span> : !hasTurns && intro ? <button type="button" className="btn-line sm" onClick={dismissIntro}>OK</button> : null}
    </div>
    {error ? <p className="visual-error" role="alert">{error}</p> : null}
    {confirm ? <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) setConfirm(null); }}>
      <div className="dialog" role="alertdialog" aria-modal="true" aria-label={confirm.kind === "undo" ? "戻す前の確認" : "やり直す前の確認"}>
        <header><h3>{confirm.kind === "undo" ? "戻す前の確認" : "やり直す前の確認"}</h3></header>
        <div className="content">
          <p>{`元のファイルが、AIの編集後に別の内容へ変わっています。このまま${confirm.kind === "undo" ? "戻す" : "やり直す"}と、その変更が失われます。`}</p>
          {confirm.paths.length ? <ul>{confirm.paths.slice(0, 10).map((p) => <li key={p}>{p}</li>)}</ul> : null}
        </div>
        <footer><div className="foot-l" /><div className="foot-r">
          <button type="button" className="btn" autoFocus onClick={() => setConfirm(null)}>やめる</button>
          <button type="button" className="btn danger" onClick={() => void run(confirm.kind, confirm.target, true)}>{confirm.kind === "undo" ? "戻す" : "やり直す"}</button>
        </div></footer>
      </div>
    </div> : null}
    {diffTurn ? <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) closeDiff(); }}>
      <div className="dialog wide" role="dialog" aria-modal="true" aria-label="ファイル差分">
        <header><h3>{diffPath}</h3><button type="button" className="btn" autoFocus onClick={closeDiff}>閉じる</button></header>
        <div className="content">
          {diffTurn.files.length > 1 ? <div className="vd-files">{diffTurn.files.map((f) => <button key={f.path} type="button" className="btn-line sm" aria-pressed={f.path === diffPath} disabled={diffBusy} onClick={() => void readDiff(diffTurn, f.path)}>{f.path}（{changeLabel(f.change)}）</button>)}</div> : null}
          {diffBusy || !diff ? <p>{diffBusy ? "差分を読み込んでいます…" : "差分を表示できませんでした。"}</p> : <>
            <p>{diff.binary ? "バイナリファイルのため本文を表示できません。" : diff.truncated ? "本文は一部のみ表示しています。" : "変更前と変更後の内容"}</p>
            {!diff.binary ? <div className="visual-diff-columns"><section><h3>変更前</h3><pre>{diff.before ?? "（ファイルなし）"}</pre></section><section><h3>変更後</h3><pre>{diff.after ?? "（ファイルなし）"}</pre></section></div> : null}
          </>}
        </div>
      </div>
    </div> : null}
  </section>;
}

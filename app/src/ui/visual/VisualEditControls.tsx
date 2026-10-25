import { useEffect, useRef, useState } from "react";
import { adoptVisualEdit, createVisualEdit, diffVisualEdit, listVisualEdits, statusVisualEdit, undoVisualEdit, type PreviewSession, type VisualDiff, type VisualEditSession } from "./api";

const messageOf = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error && typeof (error as { message: unknown }).message === "string") {
    const e = error as { code?: unknown; message: string };
    return typeof e.code === "string" ? `${e.message} (${e.code})` : e.message;
  }
  return "不明なエラー";
};
const actionLabel = (kind: "adopt" | "undo") => kind === "adopt" ? "採用" : "復元";
const changeLabel = (kind: string | null) => kind === "add" ? "追加" : kind === "delete" ? "削除" : kind === "change" ? "変更" : "要確認";
const phaseLabel = (phase: string) => ({
  editable: "編集可能", preparedUnresolved: "送信の確認待ち", acceptanceUnknown: "受理不明",
  runningOrReconciling: "実行・停止を確認中", changes: "変更を確認", rejected: "要確認",
} as Record<string, string>)[phase] ?? "状態: " + phase;

export function VisualEditControls({ enabled, preview, session, onBind, onUpdate, onLeave, onBeforeDiff, onAfterDiff, onKnownEditChats, onNotice }: {
  enabled: boolean; preview: PreviewSession | null; session: VisualEditSession | null;
  onBind: (session: VisualEditSession) => Promise<void>; onUpdate: (session: VisualEditSession) => void; onLeave: () => Promise<void>;
  onBeforeDiff: () => Promise<void>; onAfterDiff: () => void; onKnownEditChats: (ids: string[]) => void; onNotice: (text: string) => void;
}) {
  const [sessions, setSessions] = useState<VisualEditSession[]>([]);
  const [listError, setListError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(true);
  const [busy, setBusy] = useState(false);
  const [pollEpoch, setPollEpoch] = useState(0);
  const [listBusy, setListBusy] = useState(false);
  const [diff, setDiff] = useState<VisualDiff | null>(null);
  const [diffBusy, setDiffBusy] = useState(false);
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [operation, setOperation] = useState<{ kind: "adopt" | "undo"; outcome: string; revision: number; id: string | null; message?: string } | null>(null);
  const writeLock = useRef(false);
  const generation = useRef(0);
  const currentId = useRef(session?.sessionId ?? null);
  const revision = useRef(session?.revision ?? -1);
  currentId.current = session?.sessionId ?? null;
  revision.current = session?.revision ?? -1;

  useEffect(() => () => { onAfterDiff(); }, []);
  useEffect(() => {
    if (diffPath !== null) { setDiffPath(null); setDiff(null); onAfterDiff(); }
  }, [session?.revision]);
  const loadList = async () => {
    if (listBusy) return;
    setListBusy(true); setListError(null);
    try { const found = await listVisualEdits(); setSessions(found); onKnownEditChats(found.map((item) => item.chat.id)); }
    catch (e) { setListError("編集セッション一覧を取得できませんでした: " + messageOf(e)); }
    finally { setListBusy(false); }
  };
  useEffect(() => { if (enabled) void loadList(); }, [enabled]);

  useEffect(() => {
    if (!enabled || !session) return;
    const id = session.sessionId;
    const token = ++generation.current;
    let timer: number | undefined;
    let cancelled = false;
    const poll = async () => {
      if (writeLock.current) { timer = window.setTimeout(() => void poll(), 1500); return; }
      try {
        const next = await statusVisualEdit(id);
        if (cancelled || generation.current !== token || currentId.current !== id || writeLock.current || next.revision < revision.current) return;
        setStale(false); setError(null); onUpdate(next);
      } catch (e) {
        if (!cancelled && generation.current === token && !writeLock.current) {
          setStale(true);
          setError("状態を確認できませんでした。採用と復元を止めています: " + messageOf(e));
        }
      } finally {
        if (!cancelled && generation.current === token) timer = window.setTimeout(() => void poll(), 1500);
      }
    };
    void poll();
    return () => { cancelled = true; generation.current++; if (timer) window.clearTimeout(timer); };
  }, [enabled, session?.sessionId, pollEpoch]);

  const bind = async (next: VisualEditSession) => {
    if (diffPath !== null) onAfterDiff();
    setBusy(true); setStale(true); setError(null); setDiff(null); setDiffPath(null); setOperation(null);
    try { await onBind(next); }
    catch (e) { setError("コピーのプレビューを開けませんでした: " + messageOf(e)); }
    finally { setBusy(false); }
  };
  const create = async () => {
    if (!preview || session || busy || writeLock.current) return;
    writeLock.current = true; setBusy(true); setError(null);
    try {
      const next = await createVisualEdit(preview.rootPath, preview.htmlPath);
      setSessions((all) => [next, ...all.filter((item) => item.sessionId !== next.sessionId)]);
      await bind(next);
      onNotice("編集用コピーと専用チャットを作成しました。依頼はまだ送信していません。");
    } catch (e) {
      setError("作業開始の結果を確認できませんでした: " + messageOf(e) + " 一覧を再確認してください。");
    } finally { writeLock.current = false; setBusy(false); }
  };
  const recheck = async () => {
    if (!session || busy || writeLock.current) return;
    const id = session.sessionId, epoch = generation.current;
    setBusy(true);
    try {
      const next = await statusVisualEdit(id);
      if (generation.current === epoch && !writeLock.current && currentId.current === id && next.revision >= revision.current) {
        setStale(false); setError(null); onUpdate(next);
      }
    } catch (e) {
      if (generation.current === epoch && currentId.current === id) {
        setStale(true); setError("状態を確認できませんでした: " + messageOf(e));
      }
    } finally { setBusy(false); }
  };
  const readDiff = async (path: string) => {
    if (!session || diffBusy) return;
    const expectedId = session.sessionId, expectedRevision = session.revision;
    setDiffBusy(true); setError(null);
    let opened = false;
    try {
      await onBeforeDiff();
      if (currentId.current !== expectedId || revision.current !== expectedRevision) { onAfterDiff(); return; }
      opened = true; setDiff(null); setDiffPath(path);
      const result = await diffVisualEdit(expectedId, path, expectedRevision);
      if (currentId.current === expectedId && revision.current === expectedRevision && result.path === path) setDiff(result);
    } catch (e) {
      if (currentId.current === expectedId) setError("差分を読めませんでした: " + messageOf(e));
      if (!opened) onAfterDiff();
    } finally { setDiffBusy(false); }
  };
  const write = async (kind: "adopt" | "undo") => {
    if (!session || stale || busy || writeLock.current || (kind === "adopt" ? !session.canAdopt : !session.canUndo)) return;
    const id = session.sessionId, expectedRevision = session.revision;
    const previousOperationId = session.lastOperation?.id ?? null;
    writeLock.current = true; generation.current++; setPollEpoch((n) => n + 1); setBusy(true); setStale(true); setError(null); setDiff(null); setDiffPath(null);
    try {
      const result = kind === "adopt" ? await adoptVisualEdit(id, expectedRevision) : await undoVisualEdit(id, expectedRevision);
      if (currentId.current !== id) return;
      onUpdate(result);
      const last = result.lastOperation;
      if (last?.kind === kind && last.id !== previousOperationId && last.outcome === "applied") {
        setOperation({ kind, outcome: "applied", revision: result.revision, id: last.id });
      } else {
        setOperation({ kind, outcome: last?.kind === kind && last.id !== previousOperationId ? last.outcome : "unknown", revision: result.revision, id: last?.id ?? null, message: last?.message ?? undefined });
        setError(actionLabel(kind) + "の完了を確認できません。状態と保留理由を再確認してください。");
      }
    } catch (e) {
      if (currentId.current === id) {
        setOperation({ kind, outcome: "unknown", revision: expectedRevision, id: null });
        setError(actionLabel(kind) + "の結果を確認できませんでした: " + messageOf(e) + " 自動で再実行しません。");
      }
    } finally { writeLock.current = false; setBusy(false); }
  };

  const visibleChanges = session?.changes.filter((item) => item.change !== null) ?? [];
  return <section className="visual-edit" aria-label="編集用コピー">
    <div className="visual-edit-heading">
      <strong>編集用コピー</strong>
      <button type="button" className="btn-line sm" disabled={listBusy} onClick={() => void loadList()}>一覧を更新</button>
    </div>
    {!session ? <>
      <p>指定ルート内のHTML・CSS・JS全体が編集候補です。コピー作成後に対象一覧を確認してから送信してください。</p>
      <button type="button" className="btn primary sm" disabled={!preview || busy} onClick={() => void create()}>編集用コピーで作業を始める</button>
      {preview ? <p className="visual-scope-line">元ルート: {preview.rootPath}<br />HTML: {preview.htmlPath}</p> : null}
    </> : <>
      <div className="visual-edit-heading"><span>{phaseLabel(session.phase)} · 版 {session.revision}</span><button type="button" className="btn-line sm" disabled={busy} onClick={() => void recheck()}>状態を再確認</button></div>
      <div className="visual-edit-heading"><button type="button" className="btn-line sm" disabled={busy} onClick={() => void onLeave().catch((e) => setError("元のHTMLを開けませんでした: " + messageOf(e)))}>元のHTMLを閲覧</button>{!preview ? <button type="button" className="btn-line sm" disabled={busy} onClick={() => void bind(session)}>コピーを再表示</button> : null}</div>
      <p className="visual-scope-line">元: {session.sourceRoot}<br />コピー: {session.draftRoot}<br />HTML: {session.htmlPath}</p>
      <details className="visual-scope"><summary>対象範囲 {session.scope.length}件</summary><ul>{session.scope.map((item) => <li key={item.path}><span>{item.access === "edit" ? "編集可" : "表示のみ"}</span> {item.path}</li>)}</ul></details>
      {session.blockers.length ? <ul className="visual-blockers">{session.blockers.map((item, index) => <li key={index}>{item}</li>)}</ul> : null}
      <div className="visual-edit-heading"><strong>変更 {visibleChanges.length}件</strong><span>{stale ? "状態未確認" : "状態確認済み"}</span></div>
      {visibleChanges.length ? <ul className="visual-changes">{visibleChanges.map((item) => <li key={item.path}><button type="button" className="btn-line sm" onClick={() => void readDiff(item.path)}>{changeLabel(item.change)} {item.path}</button>{item.conflict ? <small>競合: {item.conflict}</small> : null}</li>)}</ul> : <p>確認できた変更はありません。</p>}
      <div className="visual-edit-actions"><button type="button" className="btn sm" disabled={busy || stale || !session.canAdopt} onClick={() => void write("adopt")}>元フォルダへ採用</button><button type="button" className="btn sm" disabled={busy || stale || !session.canUndo} onClick={() => void write("undo")}>最新の未採用編集を戻す</button></div>
      {operation && operation.revision === session.revision && (operation.id === null || operation.id === session.lastOperation?.id) ? <p role="status">{operation.outcome === "applied" ? actionLabel(operation.kind) + "を確認しました。" : actionLabel(operation.kind) + "は " + operation.outcome + " です。"} {operation.message}</p> : null}
    </>}
    {sessions.length ? <details className="visual-sessions"><summary>保存された編集セッション {sessions.length}件</summary><ul>{sessions.map((item) => <li key={item.sessionId}><button type="button" className="btn-line sm" disabled={busy || item.sessionId === session?.sessionId} onClick={() => void bind(item)}>{item.htmlPath} · {phaseLabel(item.phase)}</button><small>{item.draftRoot}</small></li>)}</ul></details> : null}
    {listError ? <p className="visual-error" role="alert">{listError}</p> : null}
    {error ? <p className="visual-error" role="alert">{error}</p> : null}
    {diffPath ? <div className="visual-diff-scrim" role="presentation"><div className="visual-diff" role="dialog" aria-modal="true" aria-label="ファイル差分"><header><strong>{diffPath}</strong><button type="button" className="btn sm" onClick={() => { setDiff(null); setDiffPath(null); onAfterDiff(); }}>閉じる</button></header>{diffBusy ? <p>差分を読み込んでいます…</p> : diff ? <><p>{diff.binary ? "バイナリファイルのため本文を表示できません。" : diff.truncated ? "本文は一部のみ表示しています。" : "元とコピーの内容"}</p>{!diff.binary ? <div className="visual-diff-columns"><section><h3>元・直前</h3><pre>{diff.before ?? "（ファイルなし）"}</pre></section><section><h3>コピー・現在</h3><pre>{diff.after ?? "（ファイルなし）"}</pre></section></div> : null}</> : <p>差分を表示できませんでした。状態を再確認してください。</p>}</div></div> : null}
  </section>;
}

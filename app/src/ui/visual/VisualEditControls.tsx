import { useEffect, useRef, useState } from "react";
import type { ChatKey } from "../../ipc/types";
import { adoptVisualEdit, createVisualEdit, diffVisualEdit, discardVisualEdit, endVisualEdit, listVisualEdits, statusVisualEdit, undoVisualEdit, type PreviewSession, type VisualDiff, type VisualEditSession, type VisualStartMode } from "./api";

const FRIENDLY: Array<[string, string]> = [
  ["this chat is running, or its stop is not confirmed", "この会話が実行中か、停止を確認できません。作業が終わってから、もう一度お試しください。"],
  ["already used by an edit session", "この会話は、すでに別の編集で使われています。"],
  ["has not returned to its original folder yet", "前の編集の後始末が済んでいません。この会話で一度送信してから、もう一度お試しください。"],
  ["send or turn that is not settled", "この会話に、結果の確認が済んでいない送信があります。"],
  ["work folder is unknown", "この会話の作業フォルダを確認できません。"],
  ["could not be forked", "会話を分岐できませんでした。何も変更していません。"],
  ["end the edit first", "先に「編集を終える」を押してください。"],
  ["some files could not be deleted", "破棄しましたが、一部のファイルを削除できませんでした（使用中の可能性があります）。"],
  ["still running or unresolved", "編集がまだ実行中か、確認が済んでいません。止まって変更が確認できてから、もう一度お試しください。"],
  ["only Codex chats", "この会話では使えません（Codexの会話のみ対応）。"],
  ["source and storage overlap", "表示しているのは編集用コピーです。「HTMLを開く」で元のHTMLを開いてください。"],
];
const friendly = (message: string): string => FRIENDLY.find(([key]) => message.includes(key))?.[1] ?? message;
const messageOf = (error: unknown): string => {
  if (error instanceof Error) return friendly(error.message);
  if (typeof error === "string") return friendly(error);
  if (error && typeof error === "object" && "message" in error && typeof (error as { message: unknown }).message === "string") {
    const e = error as { code?: unknown; message: string };
    const known = friendly(e.message);
    return known !== e.message ? known : typeof e.code === "string" ? `${e.message} (${e.code})` : e.message;
  }
  return "不明なエラー";
};
const actionLabel = (kind: "adopt" | "undo") => kind === "adopt" ? "採用" : "復元";
const changeLabel = (kind: string | null) => kind === "add" ? "追加" : kind === "delete" ? "削除" : kind === "change" ? "変更" : "要確認";
const phaseLabel = (phase: string) => ({
  editable: "編集可能", preparedUnresolved: "送信の確認待ち", acceptanceUnknown: "受理不明",
  runningOrReconciling: "実行・停止を確認中", changes: "変更を確認", rejected: "要確認",
} as Record<string, string>)[phase] ?? "状態: " + phase;

export function VisualEditControls({ enabled, chatId, currentChat, onEnded, onCloseDraftPreview, onDiscarded, preview, session, onBind, onUpdate, onLeave, onBeforeDiff, onAfterDiff, onKnownEditChats, onNotice }: {
  enabled: boolean; chatId: string | null; currentChat: ChatKey | null; onEnded: (id: string) => Promise<void>; onCloseDraftPreview: (draftRoot: string) => Promise<void>; onDiscarded: (chatId: string, mode: "dedicated" | "current") => void; preview: PreviewSession | null; session: VisualEditSession | null;
  onBind: (session: VisualEditSession) => Promise<void>; onUpdate: (session: VisualEditSession) => void; onLeave: () => Promise<void>;
  onBeforeDiff: () => Promise<void>; onAfterDiff: () => void; onKnownEditChats: (ids: string[], dedicated: string[]) => void; onNotice: (text: string) => void;
}) {
  const [sessions, setSessions] = useState<VisualEditSession[]>([]);
  const [endedSessions, setEndedSessions] = useState<VisualEditSession[]>([]);
  const [discarding, setDiscarding] = useState<string | null>(null);
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
  const [startMode, setStartMode] = useState<VisualStartMode>("current");
  const autoBound = useRef<string | null>(null);
  const sessionChatId = useRef<string | null>(null);
  sessionChatId.current = session?.chat.id ?? null;
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
    try {
      const all = await listVisualEdits(); const found = all.filter((item) => !item.ended); setSessions(found); setEndedSessions(all.filter((item) => item.ended)); onKnownEditChats(found.map((item) => item.chat.id), found.filter((item) => item.mode === "dedicated").map((item) => item.chat.id));
      // 再起動後などで専用チャットを開いたときは、その編集コピーを自動で開き直す。
      const mine = chatId && !currentId.current ? found.find((item) => item.chat.id === chatId) : undefined;
      if (mine && autoBound.current !== mine.sessionId) { autoBound.current = mine.sessionId; void bind(mine); }
      // 別の会話を選んだとき、編集中の表示を引きずらない（その会話が自分の編集を持たなければ、元のHTMLの表示へ戻す）。
      else if (!mine && chatId && sessionChatId.current && sessionChatId.current !== chatId) void onLeave().catch(() => undefined);
    }
    catch (e) { setListError("編集セッション一覧を取得できませんでした: " + messageOf(e)); }
    finally { setListBusy(false); }
  };
  useEffect(() => { autoBound.current = null; }, [chatId]);
  useEffect(() => { if (enabled) void loadList(); }, [enabled, chatId]);

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
      const mode: VisualStartMode = currentChat ? startMode : "new";
      // 表示中が前の編集のコピーのままなら、その持ち主の編集から本当の元のフォルダを求める（コピーを元にして作らない）。
      let rootPath = preview.rootPath, htmlPath = preview.htmlPath;
      if (/[\\/]visual-editor[\\/]visual-[^\\/]+[\\/]draft/i.test(rootPath)) {
        const norm = (value: string) => value.replace(/\\/g, "/").replace(/\/$/, "").toLowerCase();
        const owner = (await listVisualEdits()).find((item) => norm(item.draftRoot) === norm(rootPath));
        if (!owner) throw new Error("source and storage overlap");
        rootPath = owner.sourceRoot; htmlPath = owner.sourceRoot + "/" + owner.htmlPath;
      }
      const next = await createVisualEdit(rootPath, htmlPath, null, currentChat, mode);
      setSessions((all) => [next, ...all.filter((item) => item.sessionId !== next.sessionId)]);
      await bind(next);
      onNotice(mode === "current" ? "この会話で編集を始めました。依頼はまだ送信していません。" : mode === "fork" ? "会話を分岐して、編集を始めました。依頼はまだ送信していません。" : "編集用コピーと専用チャットを作成しました。依頼はまだ送信していません。");
    } catch (e) {
      setError("編集を始められませんでした。" + messageOf(e));
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

  // 編集を破棄する。反映していなければ元に影響しない。反映済みは keepBackup で反映前のバックアップを残すか選ぶ。
  const discard = async (item: VisualEditSession, keepBackup: boolean) => {
    if (busy || writeLock.current) return;
    writeLock.current = true; setBusy(true); setError(null);
    try {
      if (session && session.sessionId === item.sessionId) await onLeave();
      await onCloseDraftPreview(item.draftRoot);
      await discardVisualEdit(item.sessionId, keepBackup);
      setDiscarding(null);
      onDiscarded(item.chat.id, item.mode);
      onNotice("編集を破棄しました。");
    } catch (e) {
      setError("破棄できませんでした。" + messageOf(e));
    } finally { writeLock.current = false; setBusy(false); void loadList(); }
  };
  // 既存の会話での編集を終える。以後この会話は、元の作業場所で動く。反映していない変更は元へ入らない（コピーは残る）。
  const finish = async () => {
    if (!session || busy || writeLock.current || session.mode !== "current") return;
    if (!window.confirm("編集を終えます。\n・元へ反映していない変更は、元のファイルに入りません（コピーは残ります）。\n・この会話の次の送信から、元の作業場所で動きます。\n終えますか？")) return;
    writeLock.current = true; setBusy(true); setError(null);
    try {
      const done = await endVisualEdit(session.sessionId);
      if (!done.ended) throw new Error("終了を確認できませんでした。");
      await onEnded(session.sessionId);
      onNotice("編集を終えました。この会話は元の作業場所に戻ります。");
    } catch (e) {
      setError("編集を終えられませんでした: " + messageOf(e));
    } finally { writeLock.current = false; setBusy(false); }
  };
  // 反映できないときの回復: 元の最新の内容から編集用コピーを作り直す（今のコピーは「保存された編集」に残る）。
  const restart = async () => {
    if (!session || busy || writeLock.current) return;
    writeLock.current = true; setBusy(true); setError(null);
    try {
      const next = await createVisualEdit(session.sourceRoot, session.sourceRoot + "/" + session.htmlPath);
      setSessions((all) => [next, ...all.filter((item) => item.sessionId !== next.sessionId)]);
      await bind(next);
      onNotice("元の最新の内容から、編集用コピーを作り直しました。依頼はまだ送信していません。");
    } catch (e) {
      setError("作り直せませんでした: " + messageOf(e));
    } finally { writeLock.current = false; setBusy(false); }
  };
  const visibleChanges = session?.changes.filter((item) => item.change !== null) ?? [];
  const conflicts = visibleChanges.filter((item) => item.conflict);
  const waiting = !!session && ["runningOrReconciling", "preparedUnresolved", "acceptanceUnknown", "rejected"].includes(session.phase);
  const cannotApply = !!session && !stale && !waiting && visibleChanges.length > 0 && (conflicts.length > 0 || !session.canAdopt);
  const status = !session ? null : cannotApply ? { tone: "warn", text: "この編集は元のファイルに反映できません", hint: conflicts.length ? "編集を始めたあとに、元のファイルが変更されたようです。" : "元のファイルが変更された、または表示専用のファイルが変更されたようです。" }
    : stale ? { tone: "wait", text: "状態を確認しています…", hint: "" }
    : session.phase === "runningOrReconciling" ? { tone: "wait", text: "AIが作業中、または終了を確認中です", hint: "終わると変更が表示されます。" }
    : session.phase === "preparedUnresolved" || session.phase === "acceptanceUnknown" ? { tone: "warn", text: "依頼が届いたか確認できていません", hint: "チャットの表示を確認してください。自動では再送しません。" }
    : session.phase === "rejected" ? { tone: "warn", text: "依頼が受け付けられませんでした", hint: "チャットの表示を確認してください。" }
    : visibleChanges.length ? { tone: "ok", text: `AIの編集が${visibleChanges.length}件あります`, hint: "プレビューで確認して、よければ元に反映します。" }
    : { tone: "ok", text: "編集できます", hint: "プレビューをドラッグして範囲を選び、下のチャットに依頼を書いて送信します。" };
  const savedList = (items: VisualEditSession[], label: string) => items.length ? <details className="visual-sessions"><summary>{label} {items.length}件</summary><ul>{items.map((item) => <li key={item.sessionId}>
    <div className="vs-saved">
      {item.ended ? <span>{item.htmlPath}（終了）</span> : <button type="button" className="btn-line sm" disabled={busy || item.sessionId === session?.sessionId} onClick={() => void bind(item)}>{item.htmlPath} · {phaseLabel(item.phase)}</button>}
      {item.adopted ? <small>反映済み</small> : null}
      <button type="button" className="btn-line sm" disabled={busy || (item.mode === "current" && !item.ended)} title={item.mode === "current" && !item.ended ? "先に「編集を終える」を押してください" : "この編集のコピーを削除します"} onClick={() => setDiscarding(discarding === item.sessionId ? null : item.sessionId)}>破棄</button>
    </div>
    {discarding === item.sessionId ? <div className="vs-confirm">
      {item.adopted ? <>
        <p>反映済みの編集です。反映前の元の内容は、バックアップとして残せます。</p>
        <button type="button" className="btn sm" disabled={busy} onClick={() => void discard(item, true)}>コピーだけ削除（バックアップは残す）</button>
        <button type="button" className="btn sm danger" disabled={busy} onClick={() => void discard(item, false)}>バックアップも含めて削除</button>
        <small>バックアップも削除すると、元のファイルを反映前の内容に戻せなくなります。</small>
      </> : <>
        <p>コピーを削除します。元のファイルは変わりません。</p>
        <button type="button" className="btn sm" disabled={busy} onClick={() => void discard(item, false)}>破棄する</button>
      </>}
      <button type="button" className="btn-line sm" disabled={busy} onClick={() => setDiscarding(null)}>やめる</button>
    </div> : null}
    <small>{item.draftRoot}</small></li>)}</ul></details> : null;
  const existingEdit = !session && chatId ? sessions.find((item) => item.chat.id === chatId && !item.ended) : undefined;
  return <section className="visual-edit" aria-label="編集用コピー">
    {!session ? <>
      {existingEdit ? <div className="vs-start">
        <p className="visual-lead">この会話には、編集の途中のコピーがあります。</p>
        <button type="button" className="btn primary" disabled={busy} onClick={() => void bind(existingEdit)}>編集を開く</button>
      </div> : <div className="vs-start">
        <p className="visual-lead">元のファイルは変更しません。コピーでAIに編集させ、確認してから元に反映できます。</p>
        {currentChat ? <div className="vm-group" role="radiogroup" aria-label="どの会話で編集しますか">
          {([["current", "今の会話で続ける", "これまでの内容と修正を、この会話が覚えます"], ["fork", "分岐して続ける", "今の会話は残し、コピーした会話で編集します"], ["new", "新しい会話で始める", "履歴は引き継ぎません"]] as const).map(([mode, title, note]) =>
            <label key={mode} className={"vm-opt" + (startMode === mode ? " on" : "")}><input type="radio" name="visual-start-mode" checked={startMode === mode} onChange={() => setStartMode(mode)} /><span><b>{title}</b><small>{note}</small></span></label>)}
        </div> : null}
        <div className="vs-actions">
          <button type="button" className="btn primary" disabled={!preview || busy} onClick={() => void create()}>{busy ? "作成しています…" : "編集を始める"}</button>
          {!preview ? <small className="visual-hint">先に「HTMLを開く」でページを表示してください。</small> : null}
        </div>
      </div>}
      <details className="visual-more"><summary>詳細</summary>
        {preview ? <p className="visual-scope-line">元ルート: {preview.rootPath}<br />HTML: {preview.htmlPath}</p> : null}
        <button type="button" className="btn-line sm" disabled={listBusy} onClick={() => void loadList()}>保存された編集を探す</button>
        {savedList(sessions, "保存された編集")}{savedList(endedSessions, "終了した編集")}
      </details>
    </> : <>
      <div className={"vs-row " + (status?.tone ?? "")} role="status">
        <span className="vs-dot" aria-hidden="true" />
        <span className="vs-text"><strong>{status?.text}</strong>{status?.hint ? <small>{status.hint}</small> : null}</span>
      </div>
      {cannotApply ? <div className="visual-recovery">
        <strong>どうすればいい？</strong>
        <p>元の変更とAIの編集は、自動では合わせられません。</p>
        <div className="vs-actions">
          <button type="button" className="btn primary" disabled={busy} onClick={() => void restart()}>元の最新の内容から、やり直す</button>
          <button type="button" className="btn" disabled={busy} onClick={() => void onLeave().catch((e) => setError("元のHTMLを開けませんでした: " + messageOf(e)))}>まず元のファイルを見る</button>
        </div>
        <small>元のファイルの今の内容で編集用コピーを作り直します（新しい専用チャットが作られます）。今のコピーのAI編集は引き継がれませんが、「詳細」の保存された編集に残ります。</small>
      </div> : <div className="vs-actions">
        <button type="button" className="btn primary" disabled={busy || stale || !session.canAdopt} onClick={() => void write("adopt")}>この内容を元に反映</button>
        <button type="button" className="btn" disabled={busy || stale || !session.canUndo} onClick={() => void write("undo")}>直前の編集を取り消す</button>
        {session.mode === "current" ? <button type="button" className="btn-line" disabled={busy} onClick={() => void finish()} title="元の作業場所に戻ります。反映していない変更は元に入りません">編集を終える</button> : null}
      </div>}
      {operation && operation.revision === session.revision && (operation.id === null || operation.id === session.lastOperation?.id)
        ? <p className="visual-hint" role="status">{operation.outcome === "applied" ? (operation.kind === "adopt" ? "元のファイルに反映しました。" : "直前のAI編集を取り消しました。") : actionLabel(operation.kind) + "の結果は「" + operation.outcome + "」です。"} {operation.message}</p> : null}
      <details className="visual-more"><summary>詳細{visibleChanges.length ? `（変更ファイル${visibleChanges.length}件）` : ""}</summary>
        {visibleChanges.length ? <ul className="visual-changes">{visibleChanges.map((item) => <li key={item.path}><button type="button" className="btn-line sm" onClick={() => void readDiff(item.path)}>{item.path} の差分</button><small>{changeLabel(item.change)}{item.conflict ? " · 元のファイルが変更されています" : ""}</small></li>)}</ul> : null}
        <div className="visual-edit-heading">
          <span>{phaseLabel(session.phase)} · 版 {session.revision} · {stale ? "状態未確認" : "状態確認済み"}</span>
          <button type="button" className="btn-line sm" disabled={busy} onClick={() => void recheck()}>状態を再確認</button>
        </div>
        <div className="visual-edit-heading">
          <button type="button" className="btn-line sm" disabled={busy} onClick={() => void onLeave().catch((e) => setError("元のHTMLを開けませんでした: " + messageOf(e)))}>元のHTMLを閲覧</button>
          {!preview ? <button type="button" className="btn-line sm" disabled={busy} onClick={() => void bind(session)}>コピーを再表示</button> : null}
          <button type="button" className="btn-line sm" disabled={listBusy} onClick={() => void loadList()}>一覧を更新</button>
        </div>
        <p className="visual-scope-line">元: {session.sourceRoot}<br />コピー: {session.draftRoot}<br />HTML: {session.htmlPath}</p>
        <details className="visual-scope"><summary>対象範囲 {session.scope.length}件</summary><ul>{session.scope.map((item) => <li key={item.path}><span>{item.access === "edit" ? "編集可" : "表示のみ"}</span> {item.path}</li>)}</ul></details>
        {session.blockers.length ? <ul className="visual-blockers">{session.blockers.map((item, index) => <li key={index}>{item}</li>)}</ul> : null}
        {savedList(sessions, "保存された編集")}{savedList(endedSessions, "終了した編集")}
      </details>
    </>}
    {listError ? <p className="visual-error" role="alert">{listError}</p> : null}
    {error ? <p className="visual-error" role="alert">{error}</p> : null}
    {diffPath ? <div className="visual-diff-scrim" role="presentation"><div className="visual-diff" role="dialog" aria-modal="true" aria-label="ファイル差分"><header><strong>{diffPath}</strong><button type="button" className="btn sm" onClick={() => { setDiff(null); setDiffPath(null); onAfterDiff(); }}>閉じる</button></header>{diffBusy ? <p>差分を読み込んでいます…</p> : diff ? <><p>{diff.binary ? "バイナリファイルのため本文を表示できません。" : diff.truncated ? "本文は一部のみ表示しています。" : "元とコピーの内容"}</p>{!diff.binary ? <div className="visual-diff-columns"><section><h3>元・直前</h3><pre>{diff.before ?? "（ファイルなし）"}</pre></section><section><h3>コピー・現在</h3><pre>{diff.after ?? "（ファイルなし）"}</pre></section></div> : null}</> : <p>差分を表示できませんでした。状態を再確認してください。</p>}</div></div> : null}
  </section>;
}

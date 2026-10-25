import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { actOnPreview, closePreview, instructionTemplates, onPreviewError, onPreviewSelection, openPreview, parentFolder, previewAvailable, selectionDraft, updatePreviewBounds, visualSendContext, type PreviewAction, type PreviewBounds, type PreviewElement, type PreviewSession, type VisualEditSession } from "./api";
import { VisualEditControls } from "./VisualEditControls";
import "./visual.css";

const messageOf = (error: unknown): string => error instanceof Error ? error.message
  : typeof error === "string" ? error : error && typeof error === "object" && "message" in error ? String(error.message) : "不明なエラー";

/** Native children render above DOM overlays. Include the existing menus, dialogs and floating panels. */
function overlayOpen(): boolean {
  return Array.from(document.querySelectorAll<HTMLElement>(".scrim,.dropdown,.pop,.sidepanel,.lightbox,.panel-scrim,[role=menu],[role=dialog],[role=alertdialog]"))
    .some((node) => node.getClientRects().length > 0 && getComputedStyle(node).visibility !== "hidden");
}

export function VisualWorkspace({ enabled, blocked, chatName, chatId, canInsert, onInsert, onContext, onNotice, onOpenEditChat, onLeaveEditChat, onKnownEditChats, children }: {
  enabled: boolean; blocked: boolean; chatName: string | null; chatId: string | null; canInsert: boolean;
  onOpenEditChat: (id: string) => void; onLeaveEditChat: () => void; onKnownEditChats: (ids: string[]) => void;
  onInsert: (text: string) => void; onContext: (text: string | null) => void; onNotice: (message: string) => void; children: ReactNode;
}) {
  const desktop = previewAvailable();
  const stage = useRef<HTMLDivElement>(null);
  const [session, setSession] = useState<PreviewSession | null>(null);
  const [edit, setEdit] = useState<VisualEditSession | null>(null);
  const editRef = useRef<VisualEditSession | null>(null);
  const editBinding = useRef(false);
  const reloadPending = useRef(false);
  const reviewOpen = useRef(false);
  const [selection, setSelection] = useState<PreviewElement[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [busy, setBusy] = useState(false);
  const [interaction, setInteraction] = useState(false);
  const [template, setTemplate] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [inserted, setInserted] = useState(false);
  const [closeUnconfirmed, setCloseUnconfirmed] = useState(false);
  const closeFailed = useRef(false);
  const current = useRef<PreviewSession | null>(null);
  const generation = useRef(0);
  const alive = useRef(true);
  const enabledRef = useRef(enabled);
  const blockedRef = useRef(blocked);
  const pickerOpen = useRef(false);
  const operationBusy = useRef(false);
  const serial = useRef<Promise<unknown>>(Promise.resolve());
  const lastBounds = useRef("");
  const notice = useRef(onNotice);
  const onContextRef = useRef(onContext);
  onContextRef.current = onContext;
  enabledRef.current = enabled;
  blockedRef.current = blocked;
  notice.current = onNotice;

  const enqueue = useCallback(<T,>(operation: () => Promise<T>): Promise<T> => {
    const next = serial.current.catch(() => undefined).then(operation);
    serial.current = next;
    return next;
  }, []);
  const bounds = useCallback((): PreviewBounds => {
    const rect = stage.current?.getBoundingClientRect();
    if (!rect) return { x: 0, y: 0, width: 1, height: 1 };
    return { x: Math.max(0, rect.x), y: Math.max(0, rect.y), width: Math.max(1, Math.min(rect.width, window.innerWidth - Math.max(0, rect.x))), height: Math.max(1, Math.min(rect.height, window.innerHeight - Math.max(0, rect.y))) };
  }, []);
  const visible = useCallback(() => enabledRef.current && !blockedRef.current && !pickerOpen.current
    && !reviewOpen.current && document.visibilityState !== "hidden" && !!stage.current && !overlayOpen(), []);

  const syncBounds = useCallback(() => {
    const active = current.current;
    if (!active || closeFailed.current) return Promise.resolve();
    return enqueue(async () => {
      if (current.current?.sessionId !== active.sessionId) return;
      const nextBounds = bounds();
      const nextVisible = visible();
      const signature = JSON.stringify([active.sessionId, nextBounds, nextVisible]);
      if (signature === lastBounds.current) return;
      try { await updatePreviewBounds(active.sessionId, nextBounds, nextVisible); lastBounds.current = signature; }
      catch (error) {
        lastBounds.current = "";
        if (alive.current) setError(`プレビューの位置・表示を更新できませんでした: ${messageOf(error)}`);
        // Stop using a native child whose position or visibility could not be confirmed.
        try { await closePreview(active.sessionId); }
        catch (closeError) {
          closeFailed.current = true;
          if (alive.current) {
            setCloseUnconfirmed(true);
            setError(`プレビューの停止を確認できません。閉じる操作を再試行してください: ${messageOf(closeError)}`);
          }
          notice.current(`プレビューを閉じたことを確認できませんでした: ${messageOf(closeError)}`);
          return; // Keep the handle so a user can retry closing this same native child.
        }
        if (current.current?.sessionId === active.sessionId) {
          current.current = null;
          if (alive.current) { setSession(null); setSelection([]); }
        }
      }
    });
  }, [bounds, enqueue, visible]);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      generation.current += 1;
      const active = current.current;
      current.current = null;
      if (active) void enqueue(() => closePreview(active.sessionId)).catch((error) => notice.current(`プレビューを閉じたことを確認できませんでした: ${messageOf(error)}`));
    };
  }, [enqueue]);

  useEffect(() => {
    if (enabled) return;
    generation.current += 1;
    const active = current.current;
    current.current = null;
    setSession(null); setSelection([]); setTruncated(false); setError(null);
    if (active) void enqueue(() => closePreview(active.sessionId)).catch((error) => notice.current(`プレビューを閉じたことを確認できませんでした: ${messageOf(error)}`));
  }, [enabled, enqueue]);

  useEffect(() => {
    if (!enabled || !desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onPreviewSelection((event) => {
      if (!alive.current || !enabledRef.current || current.current?.sessionId !== event.sessionId) return;
      setSelection(event.selection); setTruncated(event.truncated); setInserted(false);
    }).then((off) => { if (disposed) off(); else unlisten = off; })
      .catch((error) => { if (!disposed) setError(`選択情報を受信できませんでした: ${messageOf(error)}`); });
    return () => { disposed = true; unlisten?.(); };
  }, [enabled, desktop]);

  useEffect(() => {
    if (!enabled || !desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onPreviewError((event) => {
      if (!alive.current || !enabledRef.current || current.current?.sessionId !== event.sessionId) return;
      closeFailed.current = true;
      setCloseUnconfirmed(true);
      setError(`プレビューを継続できません。閉じる操作を再試行してください: ${event.error}`);
    }).then((off) => { if (disposed) off(); else unlisten = off; })
      .catch((error) => { if (!disposed) setError(`プレビューの状態通知を受信できませんでした: ${messageOf(error)}`); });
    return () => { disposed = true; unlisten?.(); };
  }, [enabled, desktop]);
  useLayoutEffect(() => { if (enabled) void syncBounds(); }, [enabled, blocked, session, syncBounds]);
  useEffect(() => {
    if (!enabled || !desktop) return;
    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => { frame = 0; void syncBounds(); });
    };
    const resize = new ResizeObserver(schedule);
    if (stage.current) resize.observe(stage.current);
    const mutations = new MutationObserver(schedule);
    mutations.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["class", "style", "hidden", "open"] });
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    document.addEventListener("visibilitychange", schedule);
    return () => {
      resize.disconnect(); mutations.disconnect(); cancelAnimationFrame(frame);
      window.removeEventListener("resize", schedule); window.removeEventListener("scroll", schedule, true); document.removeEventListener("visibilitychange", schedule);
    };
  }, [enabled, desktop, syncBounds]);

  const replacePreview = async (htmlPath: string, rootPath: string, token: number) => {
    const old = current.current;
    if (old) {
      try { await enqueue(() => closePreview(old.sessionId)); }
      catch (error) { closeFailed.current = true; if (alive.current) setCloseUnconfirmed(true); throw error; }
      if (current.current?.sessionId === old.sessionId) current.current = null;
      if (alive.current) { setSession(null); setSelection([]); setTruncated(false); }
    }
    if (!alive.current || generation.current !== token || !enabledRef.current) return;
    const opened = await enqueue(() => openPreview(rootPath, htmlPath, bounds()));
    if (!alive.current || generation.current !== token || !enabledRef.current) {
      await enqueue(() => closePreview(opened.sessionId));
      return;
    }
    lastBounds.current = "";
    current.current = opened;
    setSession(opened); setSelection([]); setTruncated(false); setInteraction(false); setInserted(false);
    await syncBounds();
  };

  const pick = async (folder: boolean) => {
    if (!desktop || editRef.current || operationBusy.current || closeFailed.current) return;
    operationBusy.current = true;
    pickerOpen.current = true;
    const token = ++generation.current;
    setBusy(true); setError(null);
    try {
      const hiding = current.current;
      await syncBounds(); // Native preview must be hidden before opening an OS dialog.
      if (!alive.current || generation.current !== token || !enabledRef.current || closeFailed.current
        || (hiding !== null && current.current?.sessionId !== hiding.sessionId)) return;
      const active = current.current;
      const selected = await openDialog(folder
        ? { directory: true, multiple: false, title: "表示用ファイルのルートを選択", defaultPath: active?.rootPath }
        : { multiple: false, title: "プレビューするHTMLを開く", filters: [{ name: "HTML", extensions: ["html", "htm"] }] });
      pickerOpen.current = false;
      if (!alive.current || generation.current !== token || !enabledRef.current) return;
      if (typeof selected === "string") {
        if (folder && active) await replacePreview(active.htmlPath, selected, token);
        else if (!folder) await replacePreview(selected, parentFolder(selected), token);
      }
    } catch (error) {
      if (alive.current && generation.current === token) setError(`HTMLプレビューを開けませんでした: ${messageOf(error)}`);
    } finally {
      pickerOpen.current = false;
      operationBusy.current = false;
      if (alive.current) { setBusy(false); void syncBounds(); }
    }
  };

  const action = async (action: PreviewAction) => {
    const active = current.current;
    if (!active || operationBusy.current || closeFailed.current) return;
    if (action === "reload" && editRef.current && !["editable", "changes"].includes(editRef.current.phase)) {
      setError("編集の停止と変更確認が済むまで再読み込みできません。"); return;
    }
    operationBusy.current = true; setBusy(true); setError(null);
    try {
      await enqueue(() => actOnPreview(active.sessionId, action));
      if (!alive.current || current.current?.sessionId !== active.sessionId) return;
      if (action === "select" || action === "interact") setInteraction(action === "interact");
      if (action === "reload") setInteraction(false);
      if (action === "clear" || action === "reload") { setSelection([]); setTruncated(false); setInserted(false); }
    } catch (error) { if (alive.current) setError(`プレビュー操作を確認できませんでした: ${messageOf(error)}`); }
    finally { operationBusy.current = false; if (alive.current) setBusy(false); }
  };

  const retryClose = async () => {
    const active = current.current;
    if (!active || operationBusy.current) return;
    operationBusy.current = true; setBusy(true);
    try {
      await enqueue(() => closePreview(active.sessionId));
      if (!alive.current || current.current?.sessionId !== active.sessionId) return;
      current.current = null; closeFailed.current = false;
      setSession(null); setSelection([]); setTruncated(false); setCloseUnconfirmed(false); setError(null);
    } catch (error) { if (alive.current) setError(`プレビューを閉じたことを確認できませんでした: ${messageOf(error)}`); }
    finally { operationBusy.current = false; if (alive.current) setBusy(false); }
  };

  const bindEdit = async (next: VisualEditSession) => {
    if (editBinding.current || operationBusy.current || closeFailed.current) throw new Error("プレビューの操作中です。");
    editBinding.current = true;
    const token = ++generation.current;
    editRef.current = next; setEdit(next); setError(null);
    onOpenEditChat(next.chat.id);
    try { await replacePreview(next.draftRoot + "/" + next.htmlPath, next.draftRoot, token); }
    finally { editBinding.current = false; }
  };
  const beforeDiff = async () => {
    reviewOpen.current = true;
    const active = current.current;
    try {
      await syncBounds();
      if (active && (closeFailed.current || current.current?.sessionId !== active.sessionId)) throw new Error("プレビューの退避を確認できませんでした。");
    } catch (failure) {
      reviewOpen.current = false; void syncBounds(); throw failure;
    }
  };
  const afterDiff = () => { reviewOpen.current = false; void syncBounds(); };
  const leaveEdit = async () => {
    const active = editRef.current;
    if (!active || editBinding.current || operationBusy.current || closeFailed.current) throw new Error("プレビューの操作中です。");
    editBinding.current = true;
    try {
      await replacePreview(active.sourceRoot + "/" + active.htmlPath, active.sourceRoot, ++generation.current);
      editRef.current = null; setEdit(null); onLeaveEditChat();
    } finally { editBinding.current = false; }
  };
  const updateEdit = (next: VisualEditSession) => {
    if (editRef.current?.sessionId !== next.sessionId || next.revision < editRef.current.revision) return;
    const oldRevision = editRef.current.revision;
    editRef.current = next; setEdit(next);
    if (oldRevision !== next.revision) {
      setSelection([]); setTruncated(false); setInserted(false);
      reloadPending.current = true;
      if (current.current && !editBinding.current && !operationBusy.current && ["editable", "changes"].includes(next.phase)) { reloadPending.current = false; void action("reload"); }
    }
  };
  useEffect(() => {
    if (!enabled || !edit || current.current || editBinding.current || closeFailed.current) return;
    const token = ++generation.current;
    void replacePreview(edit.draftRoot + "/" + edit.htmlPath, edit.draftRoot, token)
      .catch((failure) => { if (alive.current) setError("編集用コピーのプレビューを開けませんでした: " + messageOf(failure)); });
  }, [enabled, edit?.sessionId]);
  useEffect(() => {
    if (enabled && !busy && reloadPending.current && current.current && edit && ["editable", "changes"].includes(edit.phase)) {
      reloadPending.current = false; void action("reload");
    }
  }, [enabled, busy, edit?.revision, edit?.phase]);
  // 編集チャットへの送信に、選択範囲と編集方針を自動で添える（入力欄は書き換えない）。
  const contextText = enabled && edit && chatId === edit.chat.id ? visualSendContext(edit.htmlPath, selection, edit.scope) : null;
  useEffect(() => { onContextRef.current(contextText); }, [contextText]);
  useEffect(() => () => onContextRef.current(null), []);
  if (!enabled) return <>{children}</>;
  const disabled = !session || busy || closeUnconfirmed;
  const reloadAllowed = !edit || edit.phase === "editable" || edit.phase === "changes";
  return (
    <div className="visual-workspace">
      <section className="visual-preview" aria-label="HTMLプレビュー">
        <header className="visual-toolbar">
          <button type="button" className="btn primary sm" disabled={!desktop || busy || closeUnconfirmed || !!edit} onClick={() => void pick(false)}>HTMLを開く</button>
          <button type="button" className="btn sm" disabled={disabled || !reloadAllowed} onClick={() => void action("reload")}>再読み込み</button>
          <span className="visual-tools" role="group" aria-label="ページの操作モード">
            <button type="button" className="btn sm" aria-pressed={!interaction} disabled={disabled} title="ページ上をドラッグして範囲を選びます（クリックで1つ、Ctrl＋クリックで追加）" onClick={() => void action("select")}>範囲選択</button>
            <button type="button" className="btn sm" aria-pressed={interaction} disabled={disabled} title="ページのリンクやボタンを実際に操作します" onClick={() => void action("interact")}>ページ操作</button>
          </span>
          <button type="button" className="btn sm" disabled={disabled || !selection.length} title="選択を、それを含む一つ外側の要素に広げます" onClick={() => void action("parent")}>外側へ広げる</button>
          <button type="button" className="btn sm" disabled={disabled || !selection.length} onClick={() => void action("clear")}>選択を解除</button>
        </header>
        <div className="visual-file">
          <div><span>HTML</span><strong title={session?.htmlPath}>{session?.htmlPath ?? "未選択"}</strong></div>
          <div><span>表示ルート</span><span className="visual-path" title={session?.rootPath}>{session?.rootPath ?? "HTMLの親フォルダを使用"}</span>
            <button type="button" className="btn subtle sm" disabled={disabled || !!edit} onClick={() => void pick(true)}>変更…</button></div>
        </div>
        <div className="visual-stage" ref={stage} aria-label="専用プレビューの表示領域">
          {!session ? <div className="visual-empty"><strong>HTMLを表示して、要素を選択</strong><p>{desktop ? "HTMLを開くと、ここにページを表示します。" : "デスクトップ版でHTMLを開いてください。"}</p><p>画像・CSSなどは表示ルートから読み込みます。<br />../assets が必要なら、表示ルートを親フォルダへ変更できます。</p></div>
            : <div className="visual-native-note">{blocked || overlayOpen() ? "メニュー・ダイアログの操作中はプレビューを非表示にします。" : "専用プレビュー領域"}</div>}
        </div>
        <footer className="visual-status" role="status">{busy ? "操作を確認しています…" : session ? interaction ? "ページ操作: リンクやボタンを操作できます（範囲選択に戻すには「範囲選択」）" : "範囲選択: ドラッグで範囲を選択 · クリックで1つ · Ctrl＋クリックで追加 · Escで解除" : "HTMLを開いて開始"}</footer>
      </section>
      <section className="visual-chat" aria-label="参照先のチャット">
        <div className="visual-selection">
          <VisualEditControls enabled={enabled && desktop} chatId={chatId} preview={session} session={edit} onBind={bindEdit} onUpdate={updateEdit} onLeave={leaveEdit} onBeforeDiff={beforeDiff} onAfterDiff={afterDiff} onKnownEditChats={onKnownEditChats} onNotice={onNotice} />
          <div className="visual-selection-line" role="status">
            {selection.length
              ? <><strong>選択中 {selection.length}件{truncated ? "（一部のみ）" : ""}</strong><span title={selection.map((item) => item.selector).join("\n")}>{selection[0].text.replace(/\s+/g, " ").trim().slice(0, 24) || selection[0].selector || selection[0].tag}{selection.length > 1 ? ` ほか${selection.length - 1}件` : ""}</span>
                {edit && chatId === edit.chat.id ? <small>送信すると自動で添付されます</small> : null}</>
              : <span>{edit ? "ページをドラッグして範囲を選ぶと、依頼に添付されます（選ばなくても送れます）" : "プレビューで要素を選べます"}</span>}
          </div>
          <details className="visual-more visual-fine"><summary>細かく指示する</summary>
            <div className="visual-target">入力先: {edit ? chatId === edit.chat.id ? chatName ?? edit.chat.id : "専用チャットへ切り替えてください" : chatName ?? "チャットを選択してください"}</div>
            {selection.length ? <div className="visual-selected-list">{selection.map((item, index) => <div key={`${index}:${item.selector}`}><strong title={item.selector}>{item.selector || item.tag}</strong>{item.text ? <span>{item.text}</span> : null}</div>)}</div> : <p className="visual-selection-empty">要素を選ぶと、ここに詳細が出ます。</p>}
            <div className="visual-templates" role="group" aria-label="指示のひな形">{instructionTemplates.map((item, index) => <button key={item.name} type="button" className="btn sm" aria-pressed={template === index} onClick={() => { setTemplate(index); setInserted(false); }}>{item.name}</button>)}</div>
            <button type="button" className="btn-line sm visual-insert" disabled={!session || !selection.length || truncated || !canInsert || busy || closeUnconfirmed || (!!edit && chatId !== edit.chat.id)} onClick={() => {
              if (!session || !selection.length || truncated || !canInsert || closeFailed.current || (edit && chatId !== edit.chat.id)) return;
              onInsert(selectionDraft(session, selection, instructionTemplates[template].text)); setInserted(true);
            }}>セレクタ付きの指示を入力欄に書き出す</button>
            <p className="visual-draft-note" role="status">{inserted ? "入力欄へ追加しました。内容を編集してから送信できます。" : "通常は不要です。要素を厳密に指定したいときだけ使います。"}</p>
          </details>
          {error ? <p className="visual-error" role="alert">{error}</p> : null}
          {closeUnconfirmed ? <button type="button" className="btn-line sm" disabled={busy} onClick={() => void retryClose()}>閉じるを再試行</button> : null}
        </div>
        <div className="visual-chat-body">{children}</div>
      </section>
    </div>
  );
}

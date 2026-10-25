import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { actOnPreview, closePreview, instructionTemplates, onPreviewError, onPreviewSelection, openPreview, parentFolder, previewAvailable, selectionDraft, updatePreviewBounds, type PreviewAction, type PreviewBounds, type PreviewElement, type PreviewSession } from "./api";
import "./visual.css";

const messageOf = (error: unknown): string => error instanceof Error ? error.message
  : typeof error === "string" ? error : error && typeof error === "object" && "message" in error ? String(error.message) : "不明なエラー";

/** Native children render above DOM overlays. Include the existing menus, dialogs and floating panels. */
function overlayOpen(): boolean {
  return Array.from(document.querySelectorAll<HTMLElement>(".scrim,.dropdown,.pop,.sidepanel,.lightbox,.panel-scrim,[role=menu],[role=dialog],[role=alertdialog]"))
    .some((node) => node.getClientRects().length > 0 && getComputedStyle(node).visibility !== "hidden");
}

export function VisualWorkspace({ enabled, blocked, chatName, canInsert, onInsert, onNotice, children }: {
  enabled: boolean; blocked: boolean; chatName: string | null; canInsert: boolean;
  onInsert: (text: string) => void; onNotice: (message: string) => void; children: ReactNode;
}) {
  const desktop = previewAvailable();
  const stage = useRef<HTMLDivElement>(null);
  const [session, setSession] = useState<PreviewSession | null>(null);
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
    && document.visibilityState !== "hidden" && !!stage.current && !overlayOpen(), []);

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
    if (!desktop || operationBusy.current || closeFailed.current) return;
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

  if (!enabled) return <>{children}</>;
  const disabled = !session || busy || closeUnconfirmed;
  return (
    <div className="visual-workspace">
      <section className="visual-preview" aria-label="HTMLプレビュー">
        <header className="visual-toolbar">
          <button type="button" className="btn primary sm" disabled={!desktop || busy || closeUnconfirmed} onClick={() => void pick(false)}>HTMLを開く</button>
          <button type="button" className="btn sm" disabled={disabled} onClick={() => void action("reload")}>再読み込み</button>
          <span className="visual-tools" role="group" aria-label="ページの操作モード">
            <button type="button" className="btn sm" aria-pressed={!interaction} disabled={disabled} onClick={() => void action("select")}>選択</button>
            <button type="button" className="btn sm" aria-pressed={interaction} disabled={disabled} onClick={() => void action("interact")}>操作</button>
          </span>
          <button type="button" className="btn sm" disabled={disabled || !selection.length} onClick={() => void action("parent")}>親要素</button>
          <button type="button" className="btn sm" disabled={disabled || !selection.length} onClick={() => void action("clear")}>解除</button>
        </header>
        <div className="visual-file">
          <div><span>HTML</span><strong title={session?.htmlPath}>{session?.htmlPath ?? "未選択"}</strong></div>
          <div><span>表示ルート</span><span className="visual-path" title={session?.rootPath}>{session?.rootPath ?? "HTMLの親フォルダを使用"}</span>
            <button type="button" className="btn subtle sm" disabled={disabled} onClick={() => void pick(true)}>変更…</button></div>
        </div>
        <div className="visual-stage" ref={stage} aria-label="専用プレビューの表示領域">
          {!session ? <div className="visual-empty"><strong>HTMLを表示して、要素を選択</strong><p>{desktop ? "HTMLを開くと、ここにページを表示します。" : "デスクトップ版でHTMLを開いてください。"}</p><p>画像・CSSなどは表示ルートから読み込みます。<br />../assets が必要なら、表示ルートを親フォルダへ変更できます。</p></div>
            : <div className="visual-native-note">{blocked || overlayOpen() ? "メニュー・ダイアログの操作中はプレビューを非表示にします。" : "専用プレビュー領域"}</div>}
        </div>
        <footer className="visual-status" role="status">{busy ? "操作を確認しています…" : session ? interaction ? "操作モード: リンクやボタンを操作できます" : "選択モード: クリックで選択 · Ctrl＋クリックで複数選択" : "HTMLを開いて開始"}</footer>
      </section>
      <section className="visual-chat" aria-label="参照先のチャット">
        <div className="visual-selection">
          <div className="visual-selection-title"><strong>選択情報</strong><span>{selection.length}件{truncated ? "（一部のみ）" : ""}</span></div>
          <div className="visual-target">入力先: {chatName ?? "チャットを選択してください"}</div>
          {selection.length ? <div className="visual-selected-list">{selection.map((item, index) => <div key={`${index}:${item.selector}`}><strong title={item.selector}>{item.selector || item.tag}</strong>{item.text ? <span>{item.text}</span> : null}</div>)}</div>
            : <p className="visual-selection-empty">プレビューで要素をクリックしてください。</p>}
          <div className="visual-templates" role="group" aria-label="指示のひな形">{instructionTemplates.map((item, index) => <button key={item.name} type="button" className="btn sm" aria-pressed={template === index} onClick={() => { setTemplate(index); setInserted(false); }}>{item.name}</button>)}</div>
          <button type="button" className="btn-line sm visual-insert" disabled={!session || !selection.length || truncated || !canInsert || busy || closeUnconfirmed} onClick={() => {
            if (!session || !selection.length || truncated || !canInsert || closeFailed.current) return;
            onInsert(selectionDraft(session, selection, instructionTemplates[template].text)); setInserted(true);
          }}>選択情報を入力欄へ</button>
          <p className="visual-draft-note" role="status">{inserted ? "入力欄へ追加しました。内容を編集してから送信できます。" : truncated ? "選択情報が多いため一部のみ表示しています。選択を減らしてから入力欄へ追加してください。" : "指示のひな形と要素情報を追加します。送信はチャットの入力欄から行います。"}</p>
          <p className="visual-draft-note">送信は通常のチャットとして扱われます。変更の採用・復元はまだ利用できません。</p>
          {error ? <p className="visual-error" role="alert">{error}</p> : null}
          {closeUnconfirmed ? <button type="button" className="btn-line sm" disabled={busy} onClick={() => void retryClose()}>閉じるを再試行</button> : null}
        </div>
        <div className="visual-chat-body">{children}</div>
      </section>
    </div>
  );
}

// Visual Editor の Markdown 画面（遅延読み込み）。開いている間だけ動く: 文書を読み、ファイルの変化を見て、編集欄とプレビューを描く。
// 保存・外部変更の検出・AI実行中の読み取り専用はここが持ち、依頼の送信・ターンの戻す/やり直しは Visual Editor 側（既存の仕組み）が持つ。
import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject, type ReactNode } from "react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Icon } from "../../Icon";
import { askConfirm } from "../../appConfirm";
import { useDialogFocus } from "../../useDialogFocus";
import { directRefusal, errorText, isMarkdownPath, parentFolder, visualMarkdownRead, visualMarkdownSave, visualMarkdownStamp, type MarkdownDoc, type PreviewElement } from "../api";
import { dropDraft, stashDraft, takeDraft } from "./draftStash";
import { registerMarkdownGuard } from "./guard";
import { lineCol, lineOfOffset, offsetOfLine } from "./lineMap";
import { MarkdownPreview, type PreviewHandle } from "./MarkdownPreview";
import { renderMarkdown } from "./render";
import { sanitizeMarkdownHtml } from "./sanitize";

const POLL_MS = 1000;
const PREVIEW_DELAY_MS = 200;
type View = "edit" | "split" | "preview";
const VIEWS: { id: View; label: string; hint: string }[] = [
  { id: "edit", label: "編集", hint: "編集欄だけを表示" },
  { id: "split", label: "並べる", hint: "編集欄とプレビューを並べて表示" },
  { id: "preview", label: "表示", hint: "プレビューだけを表示" },
];
const VIEW_KEY = "agentdock.markdownView";
const readView = (): View => { try { const v = window.localStorage.getItem(VIEW_KEY); return v === "edit" || v === "preview" ? v : "split"; } catch { return "split"; } };

/** Visual Editor 側（依頼の送信・画面の切り替え）から編集欄を確かめるための窓口。 */
export interface MarkdownHandle {
  isDirty: () => boolean;
  /** 未保存があれば「保存してから依頼する／やめる」を確かめて保存する。続けてよければ true（保存しないなら false＝送らない）。 */
  ensureSaved: () => Promise<boolean>;
}

export interface MarkdownPaneProps {
  rootPath: string; path: string;
  /** 編集に使う会話が実行中（AIが編集している間）。編集欄は読み取り専用になる。 */ aiBusy: boolean;
  /** 依頼を送っている最中（受理が決まるまで編集を止める）。 */ sending: boolean;
  selection: PreviewElement[]; onSelect: (items: PreviewElement[]) => void;
  /** 別名で保存したあと、そのファイルへ切り替える。 */ onOpenPath: (path: string) => void;
  handleRef: MutableRefObject<MarkdownHandle | null>;
  /** 上の操作列の左端・右端に置く、ファイルを開く・場所の表示（持ち主は Visual Editor 側）。 */ barStart?: ReactNode; barEnd?: ReactNode;
}

type Prompt = "leave" | "send";

/** 編集位置（文字の位置）の見た目の高さ。折り返しを含む行の位置合わせに使う（編集欄と同じ幅・書体の見えない複製で測る）。 */
function caretTop(ta: HTMLTextAreaElement, offset: number): number {
  const cs = getComputedStyle(ta);
  const mirror = document.createElement("div");
  const s = mirror.style;
  s.position = "absolute"; s.visibility = "hidden"; s.top = "0"; s.left = "-9999px";
  s.boxSizing = "border-box"; s.width = ta.clientWidth + "px";
  s.padding = cs.padding; s.border = "0"; s.font = cs.font; s.letterSpacing = cs.letterSpacing; s.tabSize = cs.tabSize;
  s.whiteSpace = "pre-wrap"; s.overflowWrap = "break-word";
  mirror.textContent = ta.value.slice(0, offset);
  const mark = document.createElement("span");
  mark.textContent = "​";
  mirror.appendChild(mark);
  document.body.appendChild(mirror);
  const top = mark.offsetTop;
  mirror.remove();
  return top;
}

export default function MarkdownPane({ rootPath, path, aiBusy, sending, selection, onSelect, onOpenPath, handleRef, barStart, barEnd }: MarkdownPaneProps) {
  const draftKey = rootPath + "|" + path;
  const [base, setBase] = useState<MarkdownDoc | null>(null);
  const [text, setText] = useState("");
  const [saved, setSaved] = useState("");
  const [shown, setShown] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [external, setExternal] = useState<"changed" | "missing" | null>(null);
  const [saving, setSaving] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [view, setViewState] = useState<View>(readView);
  const [epoch, setEpoch] = useState(0);
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [caret, setCaret] = useState({ line: 1, col: 1 });
  const [followLine, setFollowLine] = useState<number | null>(null);

  const textRef = useRef(""); textRef.current = text;
  const savedRef = useRef(""); savedRef.current = saved;
  const baseRef = useRef<MarkdownDoc | null>(null); baseRef.current = base;
  const stampRef = useRef<string | null>(null);
  const aliveRef = useRef(true);
  const loadingRef = useRef(false);
  const savingRef = useRef(false);
  const gen = useRef(0);
  const aiRef = useRef(false); aiRef.current = aiBusy;
  const onSelectRef = useRef(onSelect); onSelectRef.current = onSelect;
  const hasSelection = useRef(false); hasSelection.current = selection.length > 0;
  const answer = useRef<((ok: boolean) => void) | null>(null);
  const ta = useRef<HTMLTextAreaElement>(null);
  const pane = useRef<HTMLDivElement>(null);
  const preview = useRef<PreviewHandle>(null);
  const dirty = text !== saved;
  const dirtyRef = useRef(false); dirtyRef.current = dirty;

  const setView = (next: View) => { setViewState(next); try { window.localStorage.setItem(VIEW_KEY, next); } catch { /* 保存できなくても動く */ } };

  /** 読み込み。first＝最初（退避した編集があれば戻す）、auto＝ファイルの変化を検出して（未保存があれば置き換えない）、user＝利用者が読み直しを選んだ。 */
  const load = useCallback(async (mode: "first" | "auto" | "user") => {
    if (loadingRef.current) return;
    loadingRef.current = true;
    try {
      const next = await visualMarkdownRead(rootPath, path);
      if (!aliveRef.current) return;
      if (mode === "auto" && dirtyRef.current) { setExternal("changed"); return; }
      stampRef.current = next.stamp;
      let initial = next.text;
      if (mode === "first") {
        const draft = takeDraft(draftKey, next.stamp);
        if (draft !== null && draft !== next.text) { initial = draft; setNote("保存していない編集を復元しました（前回の続きです）"); }
      } else setNote(mode === "auto" ? "ファイルが変わったため、読み直しました" : null);
      if (textRef.current !== initial && hasSelection.current) onSelectRef.current([]); // 行がずれるので選択は外す
      setBase(next); setSaved(next.text); setText(initial); setShown(initial);
      setEpoch((n) => n + 1); setLoadError(null); setExternal(null); setSaveError(null);
    } catch (failure) {
      if (!aliveRef.current) return;
      const message = errorText(failure);
      setLoadError(message);
      if (mode !== "first" && /見つかりません/.test(message)) setExternal("missing");
    } finally { loadingRef.current = false; }
  }, [rootPath, path, draftKey]);

  useEffect(() => { aliveRef.current = true; return () => { aliveRef.current = false; }; }, []);
  useEffect(() => { stampRef.current = null; setBase(null); setText(""); setSaved(""); setShown(""); setLoadError(null); setSaveError(null); setExternal(null); setNote(null); void load("first"); }, [load]);

  // 閉じるとき、未保存の編集は一時退避する（同じファイルを開き直し、ファイルが変わっていなければ戻せる）。
  useEffect(() => () => { if (dirtyRef.current && stampRef.current) stashDraft(draftKey, textRef.current, stampRef.current); }, [draftKey]);
  // 未保存のままウィンドウごと閉じる・再読み込みするときの標準の確認。
  useEffect(() => {
    const h = (event: BeforeUnloadEvent) => { if (dirtyRef.current) { event.preventDefault(); event.returnValue = ""; } };
    window.addEventListener("beforeunload", h);
    return () => window.removeEventListener("beforeunload", h);
  }, []);

  // 開いている間だけ、ファイルの変化を見る（見えているときだけ）。自分の保存の途中や直後の結果は見ない。
  useEffect(() => {
    let live = true, timer = 0;
    const tick = async () => {
      if (document.visibilityState === "visible" && stampRef.current !== null && !loadingRef.current && !savingRef.current) {
        const g = gen.current;
        try {
          const now = await visualMarkdownStamp(rootPath, path);
          if (live && g === gen.current && !savingRef.current && now !== stampRef.current) {
            if (dirtyRef.current) setExternal("changed"); else await load("auto");
          }
        } catch (failure) { if (live && g === gen.current && /見つかりません/.test(errorText(failure))) setExternal("missing"); }
      }
      if (live) timer = window.setTimeout(() => void tick(), POLL_MS);
    };
    timer = window.setTimeout(() => void tick(), POLL_MS);
    return () => { live = false; window.clearTimeout(timer); };
  }, [rootPath, path, load]);

  // AIの作業が終わったら、読み直して編集内容を確かめる（未保存があれば置き換えず、食い違いを知らせる）。
  const wasBusy = useRef(aiBusy);
  useEffect(() => { if (wasBusy.current && !aiBusy) void load("auto"); wasBusy.current = aiBusy; }, [aiBusy, load]);

  // プレビューは入力から少し遅らせて描く。
  useEffect(() => {
    const timer = window.setTimeout(() => setShown(text), PREVIEW_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [text]);
  const rendered = useMemo(() => view === "edit" ? null : renderMarkdown(shown), [shown, view]);
  const html = useMemo(() => rendered ? sanitizeMarkdownHtml(rendered.html) : "", [rendered]);

  const save = useCallback(async (force = false): Promise<boolean> => {
    const b = baseRef.current;
    if (!b || savingRef.current || aiRef.current) return false;
    savingRef.current = true; gen.current++; setSaving(true); setSaveError(null);
    const sent = textRef.current;
    try {
      const result = await visualMarkdownSave({ rootPath, path, text: sent, bom: b.bom, eol: b.eol, baseStamp: stampRef.current, force });
      stampRef.current = result.stamp;
      setSaved(sent); setShown(sent); setExternal(null); setLoadError(null); setNote("保存しました");
      setBase({ ...b, stamp: result.stamp, mixed: false }); setEpoch((n) => n + 1);
      dropDraft(draftKey);
      return true;
    } catch (failure) {
      const message = errorText(failure);
      if (directRefusal(message)?.kind === "conflict") setExternal("changed"); else setSaveError("保存できませんでした: " + message);
      return false;
    } finally { savingRef.current = false; gen.current++; setSaving(false); }
  }, [rootPath, path, draftKey]);

  /** 別名で保存して、そのファイルへ切り替える（元のファイルには手を付けない）。 */
  const saveAs = async () => {
    const b = baseRef.current;
    if (!b || savingRef.current || aiRef.current) return;
    let target: string | null;
    try { target = await saveDialog({ title: "別名で保存", defaultPath: path, filters: [{ name: "Markdown", extensions: ["md", "markdown"] }] }); }
    catch (failure) { setSaveError("保存先を選べませんでした: " + errorText(failure)); return; }
    if (!target) return;
    if (!isMarkdownPath(target)) { setSaveError("ファイル名の末尾は .md または .markdown にしてください。"); return; }
    savingRef.current = true; gen.current++; setSaving(true); setSaveError(null);
    const sent = textRef.current;
    try {
      await visualMarkdownSave({ rootPath: parentFolder(target), path: target, text: sent, bom: b.bom, eol: b.eol, baseStamp: null, force: true });
      dropDraft(draftKey);
      setSaved(sent); savedRef.current = sent; dirtyRef.current = false; // 編集内容は新しいファイルへ移った（元のファイルは変えていない）
      onOpenPath(target);
    } catch (failure) { setSaveError("別名で保存できませんでした: " + errorText(failure)); }
    finally { savingRef.current = false; gen.current++; setSaving(false); }
  };

  const reloadByUser = async () => {
    if (dirtyRef.current && !(await askConfirm({ title: "読み直しますか", body: "保存していない編集は失われます。", okLabel: "読み直す", danger: true }))) return;
    await load("user");
  };

  // 画面の外（依頼の送信・別ファイルを開く・Visual Editor を閉じる）から確かめられるようにする。
  const ask = useCallback((kind: Prompt): Promise<boolean> => new Promise((resolve) => {
    answer.current?.(false);
    answer.current = resolve;
    setPrompt(kind);
  }), []);
  const settle = (ok: boolean) => { const done = answer.current; answer.current = null; setPrompt(null); done?.(ok); };
  useEffect(() => {
    handleRef.current = { isDirty: () => dirtyRef.current, ensureSaved: () => dirtyRef.current ? ask("send") : Promise.resolve(true) };
    const off = registerMarkdownGuard({ dirty: () => dirtyRef.current, leave: () => ask("leave") });
    return () => { off(); handleRef.current = null; answer.current?.(false); answer.current = null; };
  }, [ask, handleRef]);

  // 位置の連動: プレビューのブロックを押すと編集欄の該当行へ、編集欄の位置にプレビューが追従する。
  const syncCaret = () => {
    const el = ta.current;
    if (!el) return;
    const pos = lineCol(textRef.current, el.selectionStart);
    setCaret((prev) => prev.line === pos.line && prev.col === pos.col ? prev : pos);
    setFollowLine((prev) => prev === pos.line ? prev : pos.line);
  };
  useEffect(() => {
    if (view === "split" && followLine && document.activeElement === ta.current) preview.current?.scrollToLine(followLine);
  }, [followLine, view]);
  const revealLine = (line: number) => {
    setFollowLine(line);
    const el = ta.current;
    if (!el || view === "preview") return;
    const offset = offsetOfLine(textRef.current, line);
    el.setSelectionRange(offset, offset);
    el.scrollTop = Math.max(0, caretTop(el, offset) - el.clientHeight * 0.3);
    setCaret(lineCol(textRef.current, offset));
  };

  const onEditorKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return;
    const ctrl = event.ctrlKey || event.metaKey;
    if (ctrl && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "s") { event.preventDefault(); event.stopPropagation(); void save(); return; }
    // Esc: 編集欄から出る（Tab を文字として使うため、キーボードだけでも他の操作へ移れるようにする）
    if (event.key === "Escape") { event.preventDefault(); pane.current?.focus(); }
  };

  const locked = aiBusy || sending;
  const name = path.split(/[\\/]/).pop() ?? path;
  const status: { text: string; tone: "" | "busy" | "bad" | "warn" } =
    !base && loadError ? { text: "読み込めません", tone: "bad" }
    : aiBusy ? { text: "AIが編集中", tone: "busy" }
    : sending ? { text: "依頼を送信中", tone: "busy" }
    : saving ? { text: "保存中…", tone: "busy" }
    : external === "missing" ? { text: "ファイルが見つかりません", tone: "bad" }
    : external === "changed" ? { text: "外部で変更あり", tone: "bad" }
    : !base ? { text: "読み込み中…", tone: "" }
    : dirty ? { text: "未保存の変更あり", tone: "warn" }
    : { text: "保存済み", tone: "" };
  const encoding = base ? `UTF-8${base.bom ? "（BOMあり）" : ""}・改行 ${base.eol === "crlf" ? "CRLF" : "LF"}${base.mixed ? "（混在。保存すると統一）" : ""}` : "";

  return <div className="md-pane" ref={pane} tabIndex={-1}>
    <div className="md-bar">
      {barStart}
      <strong className="md-name" title={path}>{name}</strong>
      <span className={"md-state" + (status.tone ? " " + status.tone : "")} role="status">{status.text}</span>
      <span className="md-bar-end">
        <span className="vp-seg" role="group" aria-label="表示の切り替え">
          {VIEWS.map((v) => <button key={v.id} type="button" className="btn sm" aria-pressed={view === v.id} title={v.hint} onClick={() => setView(v.id)}>{v.label}</button>)}
        </span>
        <button type="button" className="btn primary sm" disabled={!base || !dirty || saving || locked} title={locked ? "AIの作業中は保存できません" : dirty ? "保存（Ctrl+S）" : "保存済みです"} onClick={() => void save()}>保存</button>
        <button type="button" className="btn subtle sm vp-ico" aria-label="読み直す" title="ファイルを読み直す" disabled={locked || saving} onClick={() => void reloadByUser()}><Icon name="refresh" /></button>
        {barEnd}
      </span>
    </div>
    {external && base ? <div className="md-alert" role="alert">
      <p>{external === "missing" ? "ファイルが見つかりません。移動・削除された可能性があります。" : "このファイルが、読み込んだあとに外部で変更されました。"}{dirty ? " 編集中の内容と食い違います。どうしますか。" : ""}</p>
      <div className="md-alert-acts">
        {external === "changed" ? <button type="button" className="btn sm" disabled={locked || saving} onClick={() => void load("user")}>読み直す（編集は破棄）</button> : null}
        <button type="button" className="btn sm" disabled={locked || saving} onClick={() => void save(true)}>{external === "missing" ? "この内容で保存し直す" : "自分の内容で上書き"}</button>
        <button type="button" className="btn sm" disabled={locked || saving} onClick={() => void saveAs()}>別名で保存…</button>
      </div>
    </div> : null}
    {loadError && base && !external ? <p className="visual-error md-msg" role="alert">{loadError}</p> : null}
    {saveError ? <p className="visual-error md-msg" role="alert">{saveError}</p> : null}
    {note && !saveError ? <p className="md-msg md-note" role="status">{note}</p> : null}
    <div className={"md-body md-view-" + view}>
      {!base ? <div className="visual-empty"><strong>{loadError ? "文書を開けませんでした" : "読み込んでいます…"}</strong>{loadError ? <p>{loadError}</p> : null}</div> : <>
        {view !== "preview" ? <div className="md-edit">
          {locked ? <p className="md-lock" role="status">{aiBusy ? "AIが編集中です。終わるまで編集できません（終わると自動で読み直します）。" : "依頼を送信しています。結果が決まるまで編集できません。"}</p> : null}
          <textarea ref={ta} className="md-editor" data-md-editor="" value={text} readOnly={locked} spellCheck={false} aria-label={"Markdownの編集欄: " + name}
            onChange={(event) => { setText(event.target.value); setNote(null); if (hasSelection.current) onSelectRef.current([]); }}
            onKeyDown={onEditorKeyDown} onSelect={syncCaret} />
        </div> : null}
        {view !== "edit" && rendered ? <div className="md-prev"><MarkdownPreview ref={preview} html={html} text={shown} rootPath={rootPath} path={path} epoch={epoch}
          selection={selection} onSelect={onSelect} onBlockPick={revealLine} currentLine={view === "split" ? followLine : null} /></div> : null}
      </>}
    </div>
    {base && view !== "preview" ? <div className="md-foot"><span>行 {caret.line}（全{lineOfOffset(text, text.length)}行）・列 {caret.col}</span><span title="読み込み時に判定した文字コードと改行。保存でも同じ形で書きます">{encoding}</span></div> : null}
    {prompt ? <SavePrompt kind={prompt} saving={saving} onSave={async () => settle(await save())} onDiscard={() => settle(true)} onCancel={() => settle(false)} /> : null}
  </div>;
}

/** 未保存の確認。leave＝離れる前（保存して続ける・保存しないで続ける・やめる）、send＝依頼の前（保存して依頼する・やめる）。 */
function SavePrompt({ kind, saving, onSave, onDiscard, onCancel }: { kind: Prompt; saving: boolean; onSave: () => void; onDiscard: () => void; onCancel: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDialogFocus(ref);
  const title = kind === "send" ? "依頼の前に保存します" : "保存していない編集があります";
  return <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onCancel(); }}
    onKeyDown={(e) => { if (e.key === "Escape" && !e.nativeEvent.isComposing) { e.preventDefault(); e.stopPropagation(); onCancel(); } }}>
    <div ref={ref} tabIndex={-1} className="dialog" role="alertdialog" aria-modal="true" aria-label={title}>
      <header><h3>{title}</h3></header>
      <div className="content"><p>{kind === "send" ? "AIはファイルに保存された内容を読んで編集します。保存してから依頼を送ります（保存しない場合は、依頼を送りません）。" : "このまま離れると、編集中の内容は一時的にだけ残ります。保存してから続けますか。"}</p></div>
      <footer><div className="foot-l" /><div className="foot-r">
        <button type="button" className="btn" onClick={onCancel}>{kind === "send" ? "送らない" : "やめる"}</button>
        {kind === "leave" ? <button type="button" className="btn danger" onClick={onDiscard}>保存しないで続ける</button> : null}
        <button type="button" className="btn primary" disabled={saving} onClick={onSave}>{kind === "send" ? "保存して依頼する" : "保存して続ける"}</button>
      </div></footer>
    </div>
  </div>;
}

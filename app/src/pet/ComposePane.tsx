// 返信・推敲のパネル（ペット窓の中。ショートカットから開いても、ペットから開いても同じ）。DESIGN_COMPOSE。
// 取り込んだ文章・結果はホストのメモリ上だけ。ここは表示と操作だけで、自動貼り付け・自動送信・自動再送はしない（出力はコピーまで）。
// 見た目はペットのカード（pc-*）と同じ柔らかい系統。主ボタン（作成／コピー）は右下に固定し、長い文章は本文の中でスクロールする。
import { useEffect, useRef, useState } from "react";
import type { ComposeMode, ComposeView } from "../ipc/types";
import * as host from "../ipc/client";
import { canAdjust, canCopy, canCreate, charCount, MODE_LABEL, originLabel, primaryAction, QUICK_ADJUSTS, statusLine } from "./composeLogic";

const MODES: ComposeMode[] = ["reply", "polish"];
const COPIED_MS = 2200;

export function ComposePane({ view, onView, onClose }: { view: ComposeView; onView: (v: ComposeView) => void; onClose: () => void }) {
  const [mode, setMode] = useState<ComposeMode>(view.mode);
  const [source, setSource] = useState(view.source);
  const [instruction, setInstruction] = useState("");
  const [adjust, setAdjust] = useState("");
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sourceRef = useRef<HTMLTextAreaElement>(null);
  const instructionRef = useRef<HTMLTextAreaElement>(null);
  const resultRef = useRef<HTMLDivElement>(null);

  // 新しく取り込んだとき（ショートカット・取り込み直し）は、入力欄を作り直す。
  useEffect(() => {
    setMode(view.mode); setSource(view.source); setInstruction(""); setAdjust(""); setExpanded(false); setCopied(false); setError(null);
    (view.source.trim() === "" ? sourceRef : instructionRef).current?.focus();
  }, [view.capture]); // eslint-disable-line react-hooks/exhaustive-deps
  // 結果が出たら、結果の先頭から読めるようにする。
  useEffect(() => {
    if (view.state.kind === "done") resultRef.current?.scrollIntoView({ block: "nearest" });
  }, [view.state.kind, view.round]);
  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(false), COPIED_MS);
    return () => window.clearTimeout(t);
  }, [copied]);

  const generating = view.state.kind === "generating";
  const status = statusLine(view, source, error);
  const primary = primaryAction(view);
  const createOk = canCreate(view, source, busy);
  const copyOk = canCopy(view, busy);
  const adjustOk = canAdjust(view, adjust, busy);

  const run = async (f: () => Promise<ComposeView>) => {
    setBusy(true);
    setError(null);
    try { onView(await f()); } catch (e) { setError(host.asIpcError(e).message); } finally { setBusy(false); }
  };
  const create = () => { if (createOk) void run(() => host.composeStart(mode, source, instruction)); };
  const doAdjust = (text: string) => { if (canAdjust(view, text, busy)) { setAdjust(""); void run(() => host.composeAdjust(text)); } };
  const copy = async () => {
    if (!copyOk) return;
    setError(null);
    try { await host.composeCopy(); setCopied(true); } catch (e) { setError(host.asIpcError(e).message); }
  };
  const reimport = () => { void run(() => host.composeOpen(mode)); };
  const main = () => { if (primary === "copy") void copy(); else create(); };

  // Ctrl+Enter: 再調整の欄なら再調整、それ以外は作成（結果があれば作り直す）。変換中の Enter は無視する。
  const onKey = (e: React.KeyboardEvent, inAdjust = false) => {
    if (e.key !== "Enter" || !e.ctrlKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    if (inAdjust) doAdjust(adjust); else create();
  };

  const showResult = !!view.result;
  return (
    <div className="pc-compose" role="group" aria-label="返信・推敲">
      <div className="pc-head">
        <span className="pc-title">返信・推敲</span>
        <div className="pc-seg" role="radiogroup" aria-label="使い方">
          {MODES.map((m) => (
            <button key={m} type="button" role="radio" aria-checked={mode === m} className={`pc-pill ${mode === m ? "picked" : "quiet"}`} disabled={generating} onClick={() => setMode(m)}>{MODE_LABEL[m]}</button>
          ))}
        </div>
      </div>
      <div className={`pc-status ${status.tone}`} role="status" aria-live="polite">{status.text}</div>

      <div className="pc-compose-body">
        <section className="pc-block" aria-label="取り込んだ文章">
          <div className="pc-block-head">
            <span className="pc-label">{mode === "reply" ? "相手の文章" : "推敲する文章"}<span className="pc-badge">{originLabel(view.origin)}</span></span>
            <span className="pc-hint">{charCount(source)}</span>
          </div>
          <textarea ref={sourceRef} className="pc-input pc-text" rows={expanded ? 9 : 3} value={source} disabled={generating} aria-label="取り込んだ文章"
            placeholder="ここに貼り付け（Ctrl+V）" onChange={(e) => setSource(e.target.value)} onKeyDown={onKey} />
          <div className="pc-block-foot">
            <button type="button" className="pc-link" onClick={() => setExpanded((x) => !x)} aria-expanded={expanded}>{expanded ? "たたむ" : "全文を広げる"}</button>
            <button type="button" className="pc-link" disabled={busy || generating} onClick={reimport} title="クリップボードの文字を取り込み直します（新しい会話になります）">取り込み直す</button>
          </div>
        </section>

        <section className="pc-block" aria-label="追加の指示">
          <label className="pc-label" htmlFor="pc-instruction">追加の指示（なくても作れます）</label>
          <textarea id="pc-instruction" ref={instructionRef} className="pc-input pc-text" rows={2} value={instruction} disabled={generating}
            placeholder={mode === "reply" ? "例: 来週なら大丈夫だと伝える。丁寧めに" : "例: 敬語をそろえる。短くしすぎない"} onChange={(e) => setInstruction(e.target.value)} onKeyDown={onKey} />
        </section>

        {showResult ? (
          <section className="pc-block" aria-label="結果">
            <div className="pc-block-head"><span className="pc-label">結果{generating ? "（前の結果）" : ""}</span><span className="pc-hint">{charCount(view.result ?? "")}</span></div>
            <div ref={resultRef} className={`pc-result${generating ? " stale" : ""}`} tabIndex={0} aria-label="結果の文章">{view.result}</div>
            <div className="pc-opts" role="group" aria-label="再調整">
              {QUICK_ADJUSTS.map((q) => (
                <button key={q.label} type="button" className="pc-pill quiet" disabled={!canAdjust(view, q.instruction, busy)} onClick={() => doAdjust(q.instruction)}>{q.label}</button>
              ))}
            </div>
            <div className="pc-adjust">
              <input className="pc-input" type="text" value={adjust} disabled={!view.canAdjust || generating} aria-label="再調整の指示" placeholder="自由に指示（例: 結論を先に）"
                onChange={(e) => setAdjust(e.target.value)} onKeyDown={(e) => onKey(e, true)} />
              <button type="button" className="pc-pill quiet" disabled={!adjustOk} onClick={() => doAdjust(adjust)}>調整</button>
            </div>
            {!view.canAdjust && !generating ? <div className="pc-hint">この会話には続けて送れません。「作り直す」で新しく作成できます</div> : null}
          </section>
        ) : null}
      </div>

      <div className="pc-actions pc-compose-foot">
        <span className={`pc-copied${copied ? " on" : ""}`} role="status" aria-live="polite">{copied ? "コピーしました" : ""}</span>
        <button type="button" className="pc-pill quiet" onClick={onClose}>{generating ? "中止して閉じる" : "閉じる"}</button>
        {primary === "copy" ? (
          <>
            <button type="button" className="pc-pill quiet" disabled={!createOk} onClick={create} title="取り込んだ文章から、新しい会話で作り直します（Ctrl+Enter）">作り直す</button>
            <button type="button" className="pc-pill primary" disabled={!copyOk} onClick={main}>コピー</button>
          </>
        ) : (
          <button type="button" className="pc-pill primary" disabled={!createOk} onClick={main} title="Ctrl+Enter でも作成できます">{generating ? "作成中…" : "作成（Ctrl+Enter）"}</button>
        )}
      </div>
    </div>
  );
}

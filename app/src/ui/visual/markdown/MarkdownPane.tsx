// Visual Editor の Markdown 画面（遅延読み込み）。開いている間だけ動く: 文書を読み、ファイルの変化を見て、プレビューを描く。
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Icon } from "../../Icon";
import { errorText, visualMarkdownRead, visualMarkdownStamp, type MarkdownDoc, type PreviewElement } from "../api";
import { MarkdownPreview } from "./MarkdownPreview";
import { renderMarkdown } from "./render";
import { sanitizeMarkdownHtml } from "./sanitize";

const POLL_MS = 1000;

export interface MarkdownPaneProps {
  rootPath: string; path: string;
  /** 編集に使う会話が実行中（AIが編集している間）。 */ aiBusy: boolean;
  selection: PreviewElement[]; onSelect: (items: PreviewElement[]) => void;
  /** 上の操作列の左端・右端に置く、ファイルを開く・場所の表示（持ち主は Visual Editor 側）。 */ barStart?: ReactNode; barEnd?: ReactNode;
}

export default function MarkdownPane({ rootPath, path, aiBusy, selection, onSelect, barStart, barEnd }: MarkdownPaneProps) {
  const [doc, setDoc] = useState<MarkdownDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [epoch, setEpoch] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const stamp = useRef<string | null>(null);
  const alive = useRef(true);
  const loading = useRef(false);
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const textRef = useRef<string | null>(null);
  const hasSelection = useRef(false);
  hasSelection.current = selection.length > 0;

  const load = useCallback(async (quiet: boolean) => {
    if (loading.current) return;
    loading.current = true;
    try {
      const next = await visualMarkdownRead(rootPath, path);
      if (!alive.current) return;
      stamp.current = next.stamp;
      if (textRef.current !== next.text && hasSelection.current) onSelectRef.current([]); // 行がずれるので選択は外す
      textRef.current = next.text;
      setDoc(next);
      setEpoch((n) => n + 1);
      setError(null);
      setNote(quiet ? "ファイルが変わったため、読み直しました" : null);
    } catch (failure) {
      if (alive.current) setError(errorText(failure));
    } finally { loading.current = false; }
  }, [rootPath, path]);

  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { stamp.current = null; textRef.current = null; setDoc(null); setError(null); setNote(null); void load(false); }, [load]);

  // 開いている間だけ、ファイルの変化を見る（見えているときだけ）。変わったら読み直す。
  useEffect(() => {
    let live = true, timer = 0;
    const tick = async () => {
      if (document.visibilityState === "visible" && stamp.current !== null && !loading.current) {
        try {
          const now = await visualMarkdownStamp(rootPath, path);
          if (live && now !== stamp.current) await load(true);
        } catch (failure) { if (live && /見つかりません/.test(errorText(failure))) setError(errorText(failure)); }
      }
      if (live) timer = window.setTimeout(() => void tick(), POLL_MS);
    };
    timer = window.setTimeout(() => void tick(), POLL_MS);
    return () => { live = false; window.clearTimeout(timer); };
  }, [rootPath, path, load]);

  // AIの作業が終わったら、読み直して編集内容を確かめる。
  const wasBusy = useRef(aiBusy);
  useEffect(() => { if (wasBusy.current && !aiBusy) void load(true); wasBusy.current = aiBusy; }, [aiBusy, load]);

  const rendered = useMemo(() => doc ? renderMarkdown(doc.text) : null, [doc]);
  const html = useMemo(() => rendered ? sanitizeMarkdownHtml(rendered.html) : "", [rendered]);
  const name = path.split(/[\\/]/).pop() ?? path;
  const status = error ? "読み込めません" : aiBusy ? "AIが編集中" : !doc ? "読み込み中…" : "表示中";

  return <div className="md-pane">
    <div className="md-bar">
      {barStart}
      <strong className="md-name" title={path}>{name}</strong>
      <span className={"md-state" + (error ? " bad" : aiBusy ? " busy" : "")} role="status">{status}</span>
      <span className="md-bar-end"><button type="button" className="btn subtle sm vp-ico" aria-label="読み直す" title="ファイルを読み直す" disabled={aiBusy} onClick={() => void load(false)}><Icon name="refresh" /></button>{barEnd}</span>
    </div>
    {error ? <p className="visual-error md-msg" role="alert">{error}</p> : note ? <p className="md-msg md-note" role="status">{note}</p> : null}
    <div className="md-body">
      {doc && rendered ? <div className="md-prev"><MarkdownPreview html={html} text={doc.text} rootPath={rootPath} path={path} epoch={epoch} selection={selection} onSelect={onSelect} /></div>
        : <div className="visual-empty"><strong>{error ? "文書を開けませんでした" : "読み込んでいます…"}</strong></div>}
    </div>
  </div>;
}

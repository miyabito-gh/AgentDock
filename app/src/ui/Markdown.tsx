// 応答の Markdown 整形表示（DESIGN_P5 §3）。生HTMLは文字として出し、リンク・画像は確認経由／読み込まない。
import { memo, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { components } from "./markdown/parts";
import { exceedsFormatLimit, remarkHtmlAsText, STREAM_THROTTLE_MS } from "./markdown/plugins";

export { LinkContext, CopyButton, COPY_LABEL, useCopy } from "./markdown/parts";

const REMARK_PLUGINS = [remarkGfm, remarkHtmlAsText];

/** 逐次表示中は最短120ms間隔で更新する。終端になったら即座に最終文字列。 */
function useThrottledText(text: string, streaming: boolean): string {
  const [shown, setShown] = useState(text);
  const last = useRef(0);
  useEffect(() => {
    if (!streaming) return;
    const wait = Math.max(0, last.current + STREAM_THROTTLE_MS - Date.now());
    const id = setTimeout(() => { last.current = Date.now(); setShown(text); }, wait);
    return () => clearTimeout(id);
  }, [text, streaming]);
  return streaming ? shown : text;
}

export const Markdown = memo(function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const shown = useThrottledText(text, streaming);
  const [forced, setForced] = useState(false);
  if (exceedsFormatLimit(shown, streaming) && !forced) {
    return (
      <div className="md">
        <div className="md-note">長いため整形せずに表示しています <button type="button" className="md-mini" onClick={() => setForced(true)}>整形して表示</button></div>
        <div className="md-plain">{shown}</div>
      </div>
    );
  }
  return <div className="md"><ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={components}>{shown}</ReactMarkdown></div>;
});

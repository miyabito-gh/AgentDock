// Markdown整形表示の部品（DESIGN_P5 §3）。node のテストから読めるよう、JSX を使わず createElement で書く。
import { Children, createContext, createElement as h, isValidElement, useContext, useEffect, useRef, useState, type ReactElement, type ReactNode } from "react";
import type { Components } from "react-markdown";
import { CODE_COLLAPSED_VIEW_LINES, CODE_COLLAPSE_LINES, countLines, linkPolicy, shortUrl } from "./plugins.ts";

/** リンクを開く要求（確認ダイアログへ渡す）。既定は何もしない（App が差し込む）。 */
export interface LinkActions { openLink: (url: string, text: string) => void }
export const LinkContext = createContext<LinkActions>({ openLink: () => {} });

/** クリップボードへ書く。Tauri のプラグイン、なければブラウザの API。失敗は false。 */
export async function copyText(text: string): Promise<boolean> {
  try {
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    await writeText(text);
    return true;
  } catch {
    try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
  }
}

const textOf = (n: ReactNode): string =>
  Children.toArray(n).map((c) => (typeof c === "string" || typeof c === "number" ? String(c) : isValidElement(c) ? textOf((c.props as { children?: ReactNode }).children) : "")).join("");

/** コピー結果の表示（2秒）。結果は aria-live で読み上げる。 */
export function useCopy(): { state: "idle" | "ok" | "ng"; copy: (text: string) => void } {
  const [state, setState] = useState<"idle" | "ok" | "ng">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  const copy = (text: string) => {
    void copyText(text).then((ok) => {
      setState(ok ? "ok" : "ng");
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setState("idle"), 2000);
    });
  };
  return { state, copy };
}

export const COPY_LABEL = { idle: "コピー", ok: "コピーしました", ng: "コピーできませんでした" } as const;

export function CopyButton({ text, className, label }: { text: string; className?: string; label?: string }) {
  const { state, copy } = useCopy();
  return h("button", { type: "button", className, onClick: () => copy(text), "aria-live": "polite" }, state === "idle" ? (label ?? COPY_LABEL.idle) : COPY_LABEL[state]);
}

/** リンク。http・https だけ押せる（確認ダイアログ経由）。それ以外は押せない文字。href は付けず、アプリ内遷移できない形にする。 */
export function ExtLink({ href, children }: { href?: string; children?: ReactNode }) {
  const { openLink } = useContext(LinkContext);
  if (linkPolicy(href) !== "open" || !href) {
    return h("span", { className: "md-link off", title: "このリンクは開きません（http・https以外）" }, children);
  }
  const go = () => openLink(href, textOf(children));
  return h("a", {
    role: "link", tabIndex: 0, className: "md-link", title: href,
    onClick: (e: { preventDefault: () => void }) => { e.preventDefault(); go(); },
    onAuxClick: (e: { preventDefault: () => void }) => e.preventDefault(),
    onKeyDown: (e: { key: string; preventDefault: () => void }) => { if (e.key === "Enter") { e.preventDefault(); go(); } },
  }, children, h("span", { "aria-hidden": "true", className: "md-ext" }, " ↗"));
}

/** 画像は読み込まない。代替文字とURLを文字で出す。 */
export function ImageStub({ src, alt }: { src?: string; alt?: string }) {
  const { openLink } = useContext(LinkContext);
  const url = typeof src === "string" ? src : "";
  return h("span", { className: "md-img" },
    `[画像: ${alt || "（代替文字なし）"}]`,
    url ? h("span", { className: "md-img-url mono", title: url.startsWith("data:") ? undefined : url }, ` ${shortUrl(url)}`) : null,
    linkPolicy(url) === "open" ? h("button", { type: "button", className: "md-mini", onClick: () => openLink(url, alt ?? "") }, "ブラウザで開く") : null);
}

/** コードブロック。言語名・コピー・折り返し切替。41行以上は高さを制限する（本文はDOMに全部ある）。 */
export function CodeBlock({ code, lang }: { code: string; lang: string | null }) {
  const [wrap, setWrap] = useState(false);
  const [all, setAll] = useState(false);
  const lines = countLines(code);
  const long = lines > CODE_COLLAPSE_LINES;
  const limited = long && !all;
  return h("div", { className: "md-code" },
    h("div", { className: "md-code-bar" },
      h("span", { className: "md-lang" }, lang ?? ""),
      h("span", { className: "md-code-acts" },
        h("button", { type: "button", className: "md-mini", "aria-pressed": wrap, onClick: () => setWrap(!wrap) }, "折り返す"),
        h(CopyButton, { text: code, className: "md-mini" }))),
    h("pre", { className: wrap ? "wrap" : undefined, style: limited ? { maxHeight: `${CODE_COLLAPSED_VIEW_LINES * 1.5}em`, overflow: "hidden" } : undefined, tabIndex: 0 },
      h("code", null, code)),
    long ? h("button", { type: "button", className: "md-mini md-more", "aria-expanded": all, onClick: () => setAll(!all) }, all ? "折りたたむ" : `すべて表示（${lines}行）`) : null);
}

/** react-markdown の部品の差し替え表。 */
export const components: Components = {
  a: ({ href, children }) => h(ExtLink, { href }, children),
  img: ({ src, alt }) => h(ImageStub, { src: typeof src === "string" ? src : undefined, alt }),
  pre: ({ children }) => {
    const only = Children.toArray(children)[0];
    if (isValidElement(only)) {
      const p = (only as ReactElement<{ className?: string; children?: ReactNode }>).props;
      const lang = /language-(\S+)/.exec(p.className ?? "")?.[1] ?? null;
      return h(CodeBlock, { code: textOf(p.children).replace(/\n$/, ""), lang });
    }
    return h("pre", null, children);
  },
  table: ({ children }) => h("div", { className: "md-table" }, h("table", null, children)),
  input: ({ checked, type }) => type === "checkbox"
    ? h("span", { className: "md-task" }, h("input", { type: "checkbox", disabled: true, checked: !!checked, readOnly: true }), h("span", { className: "sr" }, checked ? "済み" : "未"))
    : null,
};

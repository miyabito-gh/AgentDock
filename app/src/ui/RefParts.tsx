// 会話内のチャット参照・ファイルパスの表示部品（項目5・24・30）。検出は markdown/refs.ts。
// ボタン・メニューにするのは「一覧にあるチャット」と「実在を確認できたパス」だけ。それ以外は元の見た目のまま。
import { Children, createContext, isValidElement, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { PathInfo } from "../ipc/types";
import { ExtLink } from "./markdown/parts";
import { linkPolicy } from "./markdown/plugins";
import { baseNameOf, pathCandidate, pathFromHref, pathLinkView, type PathCandidate, type PathLinkView } from "./markdown/refs";

export type PathAction = "attach" | "copyAbs" |"open" | "reveal";

export interface RefActions {
  /** 一覧にあるチャットの表示名。一覧になければ undefined。 */
  chatTitle: (id: string) => string | undefined;
  /** そのチャットを選ぶ（一覧から選ぶのと同じ。再開・送信はしない）。 */
  openChat: (id: string) => void;
  /** 一覧を取り直す（読取りのみ）。 */
  refreshList: () => Promise<void>;
  /** 確認済みのパス情報（まだ・確認中なら undefined）。 */
  pathInfo: (path: string) => PathInfo | undefined;
  /** パスの実在確認を依頼する（まとめて・間引いて問い合わせる）。 */
  requestPath: (path: string) => void;
  /** `strict`＝会話中のリンクから。作業フォルダの外・実行形式は確認でも開かない（ホストも同じ条件で止める）。 */
  pathAct: (act: PathAction, path: string, info: PathInfo, strict?: boolean) => void;
  canAttach: boolean;
  /** このチャットの作業フォルダ（リンクの内外判定で、作業フォルダ自身を内にするため）。不明なら null。 */
  cwd?: string | null;
}
export const RefContext = createContext<RefActions | null>(null);

const textOf = (n: ReactNode): string =>
  Children.toArray(n).map((c) => (typeof c === "string" || typeof c === "number" ? String(c) : isValidElement(c) ? textOf((c.props as { children?: ReactNode }).children) : "")).join("");

const clip = (s: string, n = 24): string => (s.length > n ? `${s.slice(0, n)}…` : s);

function ThreadRef({ id, directive, code, children }: { id: string; directive: boolean; code: boolean; children?: ReactNode }) {
  const ctx = useContext(RefContext);
  const [state, setState] = useState<"idle" | "busy" | "tried">("idle");
  const title = ctx?.chatTitle(id);
  if (ctx && title !== undefined) {
    return (
      <button type="button" className="md-ref" title={`${title}\n${id}`} onClick={() => ctx.openChat(id)}>
        {title ? `チャット「${clip(title)}」を開く` : "このチャットを開く"}
      </button>
    );
  }
  if (ctx && directive) {
    const refresh = () => {
      setState("busy");
      void ctx.refreshList().finally(() => setState("tried"));
    };
    return (
      <span className="md-ref off" title={id}>
        このチャットは一覧にありません{state === "tried" ? "（更新後も見つかりません）" : ""}{" "}
        <button type="button" className="md-mini" disabled={state === "busy"} onClick={refresh}>{state === "busy" ? "更新中…" : "一覧を更新"}</button>
      </span>
    );
  }
  return code ? <code>{id}</code> : <>{children}</>;
}

/** パスのメニュー。段落（p）の外＝body直下に出し、スクロール領域・表の枠で切れない。位置はアンカーの座標から決め、画面端で反転する。
 *  Esc・外側クリック・スクロール・リサイズで閉じ、Esc／選択のあとはアンカーへフォーカスを戻す。 */
function PathMenu({ label, title, trigger, children }: { label: string; title: string; trigger: ReactNode; children: (close: () => void) => ReactNode }) {
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const id = useId();
  const open = pos !== null;
  const closeQuiet = useCallback(() => setPos(null), []);
  const close = useCallback(() => { setPos(null); btn.current?.focus(); }, []);
  const show = () => {
    const r = btn.current?.getBoundingClientRect();
    if (r) setPos({ top: r.bottom + 4, left: r.left });
  };
  // 画面からはみ出すときは、上へ反転・左へ寄せる。
  useLayoutEffect(() => {
    if (!pos || !panel.current || !btn.current) return;
    const r = btn.current.getBoundingClientRect();
    const p = panel.current.getBoundingClientRect();
    let { top, left } = pos;
    if (top + p.height > window.innerHeight - 4) top = Math.max(4, r.top - 4 - p.height);
    if (left + p.width > window.innerWidth - 4) left = Math.max(4, window.innerWidth - 4 - p.width);
    if (top !== pos.top || left !== pos.left) setPos({ top, left });
  }, [pos]);
  useEffect(() => {
    if (!open) return;
    panel.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); e.preventDefault(); close(); } };
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!panel.current?.contains(t) && !btn.current?.contains(t)) closeQuiet();
    };
    const onScroll = (e: Event) => { if (!panel.current?.contains(e.target as Node)) closeQuiet(); };
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", closeQuiet);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", closeQuiet);
    };
  }, [open, close, closeQuiet]);
  const onPanelKey = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = Array.from(panel.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length].focus();
  };
  return (
    <>
      <button ref={btn} type="button" className="md-path" aria-expanded={open} aria-haspopup="menu" aria-controls={open ? id : undefined} aria-label={label} title={title}
        onClick={() => (open ? closeQuiet() : show())} onContextMenu={(e) => { e.preventDefault(); show(); }}>{trigger}</button>
      {pos ? createPortal(
        <div id={id} ref={panel} role="menu" aria-label={label} tabIndex={-1} className="pop" style={{ position: "fixed", top: pos.top, left: pos.left, right: "auto" }} onKeyDown={onPanelKey}>
          {children(close)}
        </div>, document.body) : null}
    </>
  );
}

/** 会話中のリンク用。作業フォルダ内のファイルだけ「開く」「保存場所を表示」を出し、外・実行形式は理由を示す。 */
function LinkPathMenu({ path, info, view, label }: { path: string; info: PathInfo; view: Extract<PathLinkView, { kind: "menu" }>; label: ReactNode }) {
  const ctx = useContext(RefContext);
  if (!ctx) return <>{label}</>;
  return (
    <PathMenu label={`ファイル操作: ${baseNameOf(path)}`} title={`${info.absolute}（クリックまたは右クリックで操作）`} trigger={label}>
      {(close) => {
        const act = (a: PathAction) => () => { close(); ctx.pathAct(a, path, info, true); };
        return (
          <>
            <div className="muted" style={{ padding: "4px 10px", maxWidth: 360, overflowWrap: "anywhere" }}>{info.absolute}</div>
            {view.reason ? <div className="muted" role="note" style={{ padding: "0 10px 4px", maxWidth: 360 }}>{view.reason}</div> : null}
            {view.open ? <button role="menuitem" onClick={act("open")}><span>{info.kind === "folder" ? "フォルダを開く（既定のアプリ）" : "開く（既定のアプリ）"}</span></button> : null}
            {view.reveal ? <button role="menuitem" onClick={act("reveal")}><span>保存場所を表示</span></button> : null}
            <button role="menuitem" onClick={act("copyAbs")}><span>パスをコピー</span></button>
          </>
        );
      }}
    </PathMenu>
  );
}

function PathRef({ cand, plain, label, strict = false }: { cand: PathCandidate | null; plain: ReactNode; label: ReactNode; strict?: boolean }) {
  const ctx = useContext(RefContext);
  const wrap = useRef<HTMLSpanElement>(null);
  const path = cand?.path;
  const request = ctx?.requestPath;
  // 画面に見えたものだけ、実在確認を依頼する（見えないものは問い合わせない）。
  useEffect(() => {
    if (!request || !path || !wrap.current) return;
    if (typeof IntersectionObserver === "undefined") { request(path); return; }
    const io = new IntersectionObserver((es) => { if (es.some((e) => e.isIntersecting)) { request(path); io.disconnect(); } });
    io.observe(wrap.current);
    return () => io.disconnect();
  }, [request, path]);
  const info = ctx && path ? ctx.pathInfo(path) : undefined;
  const ok = ctx && path && info && (info.kind === "file" || info.kind === "folder") && info.absolute ? info : null;
  if (strict && ctx && path && info) {
    const view = pathLinkView(info, ctx.cwd ? ctx.pathInfo(ctx.cwd)?.absolute : null);
    if (view?.kind === "missing") return <span ref={wrap} className="md-pathwrap"><span className="md-link off" title={`見つかりません: ${path}`}>{label}（見つかりません）</span></span>;
    if (view?.kind === "unconfirmed") return <span ref={wrap} className="md-pathwrap"><span className="md-link off" title={`確認できません: ${path}`}>{label}（存在を確認できません）</span></span>;
    if (view?.kind === "menu") return <span ref={wrap} className="md-pathwrap"><LinkPathMenu path={path} info={info} view={view} label={label} /></span>;
  }
  return (
    <span ref={wrap} className="md-pathwrap">
      {ok && ctx && path ? (
        <PathMenu label={`ファイル操作: ${baseNameOf(path)}`} title={`${ok.absolute}（クリックまたは右クリックで操作）`} trigger={label}>
          {(close) => {
            const act = (a: PathAction) => () => { close(); ctx.pathAct(a, path, ok); };
            return (
              <>
                {ok.kind === "file" ? <button role="menuitem" disabled={!ctx.canAttach} title={ctx.canAttach ? "入力欄の添付に追加します（コピーして保持）" : "チャットを選ぶと使えます"} onClick={act("attach")}><span>コンテキストとして添付</span></button> : null}
                <button role="menuitem" onClick={act("copyAbs")}><span>絶対パスをコピー</span></button>
                <hr />
                <button role="menuitem" onClick={act("open")}><span>{ok.kind === "folder" ? "フォルダを開く" : "開く"}</span></button>
                <button role="menuitem" onClick={act("reveal")}><span>エクスプローラーで表示</span></button>
              </>
            );
          }}
        </PathMenu>
      ) : plain}
    </span>
  );
}

/** react-markdown の `span` の差し替え。参照の印（data-ref）が付いたものだけ部品にし、他はそのまま。 */
export function RefSpan(props: Record<string, unknown> & { children?: ReactNode }) {
  const { node: _node, children, ...rest } = props;
  const kind = rest["data-ref"];
  if (kind === "thread") {
    return <ThreadRef id={String(rest["data-id"] ?? "")} directive={rest["data-directive"] === "1"} code={rest["data-code"] === "1"}>{children}</ThreadRef>;
  }
  if (kind === "path") {
    const code = <code>{children}</code>;
    return <PathRef cand={{ path: String(rest["data-path"] ?? ""), line: null, col: null }} plain={code} label={code} />;
  }
  return <span {...(rest as Record<string, never>)}>{children}</span>;
}

/** react-markdown の `a` の差し替え。http・https は従来どおり確認付きリンク。それ以外でパスと読めるものは、実在を確認できたときだけメニューにする。 */
export function RefLink({ href, children }: { href?: string; children?: ReactNode }) {
  const plain = <ExtLink href={href}>{children}</ExtLink>;
  if (linkPolicy(href) === "open") return plain;
  return <PathRef strict cand={pathFromHref(href) ?? pathCandidate(textOf(children))} plain={plain} label={<span className="md-link">{children}</span>} />;
}

/** 成果物欄など、パス文字列をそのまま出す場所用。 */
export function PathText({ path }: { path: string }) {
  const code = <>{path}</>;
  return <PathRef cand={pathCandidate(path)} plain={code} label={code} />;
}

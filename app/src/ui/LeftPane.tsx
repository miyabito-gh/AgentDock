import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Chat, HostSnapshot } from "../ipc/types";
import { Icon } from "./Icon";
import { TENTATIVE_HINT, TENTATIVE_STYLE, chatAgents, chatName, chatRequests, chatStatusText, chatTitle, sortChats } from "./derive";
import { STATE, keyStr, knownValue } from "./format";
import { PENDING_TEXT, archiveTag } from "./ManageDialogs";
import { ROW_HEIGHT, WINDOW_THRESHOLD, scrollTopToReveal, windowRange } from "./listWindow";

/** 一覧の状態文字（chatStatusText）から状態チップの色クラスを引く。対応が無い文言（履歴なし等）は色なしの控えめな文字。 */
function statusClass(text: string): string {
  const head = text.split("（")[0];
  for (const s of Object.values(STATE)) if (s.t === head) return s.c;
  if (text.startsWith("対応待ち")) return "wait";
  if (text.startsWith("作業中")) return text.includes("失敗") ? "fail" : "run";
  if (text.includes("状態不明")) return "unk";
  return "";
}

export function LeftPane({ snap, sel, onSelect, onAct, onAcknowledge, listStatus }: {
  snap: HostSnapshot; sel: string | null; onSelect: (id: string) => void; onAct: (a: string) => void; onAcknowledge: (c: Chat) => void;
  /** 直近の「更新」の結果（成功・失敗とも次の更新まで残す）。 */
  listStatus?: { ok: boolean; text: string } | null;
}) {
  const [query, setQuery] = useState("");
  const [inclArch, setInclArch] = useState(false);
  const [showArch, setShowArch] = useState(false);
  const q = query.trim();
  const localOf = (c: Chat) => snap.chatLocals.find((l) => l.chat.id === c.key.id);
  // アプリ側のアーカイブ（Codex へ未反映でも一覧から隠す、M43）と Codex 側のアーカイブのどちらかなら、アーカイブ扱い。
  const archived = (c: Chat) => localOf(c)?.visibility.kind === "archived" || knownValue(c.archived) === true;
  // 外部作成の会話を「一覧から外した」ものは、元の履歴を消さずにアプリの一覧からだけ隠す。
  const removed = (c: Chat) => localOf(c)?.visibility.kind === "removedFromList";
  const visible = sortChats(snap.chats).filter((c) => !removed(c)).filter((c) =>
    (showArch ? archived(c) : !archived(c) || (q !== "" && inclArch)) && (!q || chatName(c).includes(q)));
  const pinned = visible.filter((c) => c.pinned && !showArch);
  const rest = visible.filter((c) => !c.pinned || showArch);

  // 200件を超えたら固定行高の窓表示（見える範囲＋少しだけ描く）。スクロール位置は一覧の先頭からの相対で持つ。
  const scrollerRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState({ rel: 0, view: 600 });
  const windowed = rest.length > WINDOW_THRESHOLD;
  const measure = useCallback(() => {
    const sc = scrollerRef.current, ls = listRef.current;
    if (!sc || !ls) return;
    const offset = ls.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
    const next = { rel: sc.scrollTop - offset, view: sc.clientHeight };
    setMetrics((m) => (m.rel === next.rel && m.view === next.view ? m : next));
  }, []);
  useLayoutEffect(() => { if (windowed) measure(); }, [windowed, rest.length, showArch, pinned.length, measure]);
  useEffect(() => {
    const sc = scrollerRef.current;
    if (!windowed || !sc || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(sc);
    return () => ro.disconnect();
  }, [windowed, measure]);
  const range = windowed ? windowRange(rest.length, metrics.rel, metrics.view) : { start: 0, end: rest.length };
  // 矢印キーの移動で窓の外へ出る行は、スクロールしてから描画後にフォーカスする。
  const focusIdx = useRef<number | null>(null);
  useEffect(() => {
    if (focusIdx.current === null) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-idx="${focusIdx.current}"]`);
    if (el) { el.focus(); focusIdx.current = null; }
  });
  const onListKey = (e: React.KeyboardEvent) => {
    if (!windowed || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    const cur = (e.target as HTMLElement).closest<HTMLElement>("[data-idx]");
    if (!cur) return;
    e.preventDefault();
    const idx = Math.min(rest.length - 1, Math.max(0, Number(cur.dataset.idx) + (e.key === "ArrowDown" ? 1 : -1)));
    const sc = scrollerRef.current, ls = listRef.current;
    if (sc && ls) {
      const offset = ls.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
      sc.scrollTop = offset + scrollTopToReveal(idx, sc.scrollTop - offset, sc.clientHeight);
    }
    focusIdx.current = idx;
    measure();
    const el = ls?.querySelector<HTMLElement>(`[data-idx="${idx}"]`);
    if (el) { el.focus(); focusIdx.current = null; }
  };
  const restRows = () => (windowed ? (
    <div ref={listRef} onKeyDown={onListKey} style={{ height: rest.length * ROW_HEIGHT, position: "relative" }}>
      <div style={{ position: "absolute", top: range.start * ROW_HEIGHT, left: 0, right: 0 }}>
        {rest.slice(range.start, range.end).map((c, i) => row(c, range.start + i))}
      </div>
    </div>
  ) : <div ref={listRef}>{rest.map((c) => row(c))}</div>);

  const row = (c: Chat, idx?: number) => {
    const k = keyStr(c.key);
    // 印はホストが状態から導く（通知設定に関係なく付く）。ホストの値がなければ（モック）画面側の状態から出す。
    const marks = snap.chatLocals.find((l) => l.chat.id === c.key.id)?.marks;
    const hasReq = marks?.awaitingAnswer || chatRequests(snap, c).length > 0;
    const unconfirmedFail = marks ? marks.unacknowledgedFailure : chatAgents(snap, c).some((a) => a.status.state === "failed");
    return (
      <button key={k} className={`row-chat ${k === sel ? "sel" : ""}${idx !== undefined ? " fixed" : ""}`} onClick={() => onSelect(c.key.id)} aria-current={k === sel}
        {...(idx !== undefined ? { style: { height: ROW_HEIGHT, minHeight: ROW_HEIGHT }, "data-idx": idx, "aria-posinset": idx + 1, "aria-setsize": rest.length } : {})}>
        <span className="nm" style={chatTitle(c).confirmed ? undefined : TENTATIVE_STYLE} title={chatTitle(c).confirmed ? undefined : TENTATIVE_HINT}>{chatName(c)}</span>
        <span className="marks">
          {localOf(c)?.deletePending ? <span className="tag unv" title={PENDING_TEXT(localOf(c)!.deletePending!.reason)}>削除保留</span> : null}
          {hasReq ? <span className="mark wait" title="承認・質問待ち">待</span> : null}
          {unconfirmedFail ? (
            <span className="mark fail" role="button" tabIndex={0} title="未確認の失敗があります。クリックで「確認済み」にします（再実行ではありません）"
              onClick={(e) => { e.stopPropagation(); onAcknowledge(c); }}
              onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); onAcknowledge(c); } }}>失</span>
          ) : null}
        </span>
        <span className="sub">
          {c.kind === "general" ? "一般" : <Icon name="folder" />}
          <span className={`st ${statusClass(chatStatusText(snap, c))}`} title={chatStatusText(snap, c)}><i aria-hidden="true" /><span>{chatStatusText(snap, c)}</span></span>
          {archived(c) ?<span className="tag" title={archiveTag(localOf(c)).title || undefined}>{archiveTag(localOf(c)).text}</span> : null}
        </span>
      </button>
    );
  };

  return (
    <>
      <div className="left-top">
        <div className="lt-row">
          <button className="btn-line grow" onClick={() => onAct("newChat")}><Icon name="plus" />新しいチャット</button>
          <button className="btn icon" title={listStatus?.ok ? `チャット一覧を Codex から取り直します（読み取りのみ）。直近の結果: ${listStatus.text}` : "チャット一覧を Codex から取り直します（読み取りのみ）"} aria-label="一覧を更新" onClick={() => onAct("refreshList")}><Icon name="refresh" /></button>
        </div>
        <input type="search" placeholder="名前で検索" aria-label="チャットを名前で検索" value={query} onChange={(e) => setQuery(e.target.value)} />
        {q ? <label className="small"><input type="checkbox" checked={inclArch} onChange={(e) => setInclArch(e.target.checked)} />アーカイブも含める</label> : null}
        {listStatus && !listStatus.ok ? <div className="small lt-fail" role="status">{listStatus.text}</div> : null}
      </div>
      <div className="left-scroll" ref={scrollerRef} onScroll={windowed ? measure : undefined}>
        {showArch ? (
          <>
            <div className="list-h"><span>アーカイブ</span><button className="small" style={{ height: 20 }} onClick={() => setShowArch(false)}>戻る</button></div>
            {rest.length ? restRows() : <div className="empty-note">アーカイブしたチャットはありません</div>}
          </>
        ) : (
          <>
            {pinned.length ? <><div className="list-h"><span>ピン留め</span></div>{pinned.map((c) => row(c))}</> : null}
            <div className="list-h"><span>最近使った順</span></div>
            {rest.length ? restRows() : <div className="empty-note">{q ? "一致するチャットはありません" : "まだチャットがありません"}</div>}
          </>
        )}
      </div>
      <div className="left-foot">
        <button className="small" style={{ height: 22 }} onClick={() => setShowArch(true)}><Icon name="archive" />アーカイブ（{snap.chats.filter((c) => !removed(c) && archived(c)).length}）</button>
        <span className="only-narrow-l"><button className="small" style={{ height: 22 }} onClick={() => onAct("toggleLeft")}>閉じる</button></span>
      </div>
    </>
  );
}

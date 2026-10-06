import { useState } from "react";
import type { Chat, HostSnapshot } from "../ipc/types";
import { Icon } from "./Icon";
import { TENTATIVE_HINT, TENTATIVE_STYLE, chatAgents, chatName, chatRequests, chatStatusText, chatTitle } from "./derive";
import { keyStr, knownValue } from "./format";
import { archiveTag } from "./ManageDialogs";

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
  const visible = snap.chats.filter((c) => !removed(c)).filter((c) =>
    (showArch ? archived(c) : !archived(c) || (q !== "" && inclArch)) && (!q || chatName(c).includes(q)));
  const pinned = visible.filter((c) => c.pinned && !showArch);
  const rest = visible.filter((c) => !c.pinned || showArch);

  const row = (c: Chat) => {
    const k = keyStr(c.key);
    // 印はホストが状態から導く（通知設定に関係なく付く）。ホストの値がなければ（モック）画面側の状態から出す。
    const marks = snap.chatLocals.find((l) => l.chat.id === c.key.id)?.marks;
    const hasReq = marks?.awaitingAnswer || chatRequests(snap, c).length > 0;
    const unconfirmedFail = marks ? marks.unacknowledgedFailure : chatAgents(snap, c).some((a) => a.status.state === "failed");
    return (
      <button key={k} className={`row-chat ${k === sel ? "sel" : ""}`} onClick={() => onSelect(c.key.id)} aria-current={k === sel}>
        <span className="nm" style={chatTitle(c).confirmed ? undefined : TENTATIVE_STYLE} title={chatTitle(c).confirmed ? undefined : TENTATIVE_HINT}>{chatName(c)}</span>
        <span className="marks">
          {hasReq ? <span className="mark wait" title="承認・質問待ち">待</span> : null}
          {unconfirmedFail ? (
            <span className="mark fail" role="button" tabIndex={0} title="未確認の失敗があります。クリックで「確認済み」にします（再実行ではありません）"
              onClick={(e) => { e.stopPropagation(); onAcknowledge(c); }}
              onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); onAcknowledge(c); } }}>失</span>
          ) : null}
        </span>
        <span className="sub">
          {c.kind === "general" ? "一般" : <Icon name="folder" />}
          <span>{chatStatusText(snap, c)}</span>
          {archived(c) ? <span className="tag" title={archiveTag(localOf(c)).title || undefined}>{archiveTag(localOf(c)).text}</span> : null}
          {localOf(c)?.deletePending ? <span className="tag unv" title="停止を確認できていない、または一部の削除に失敗したため、削除を保留しています。会話・添付・成果物は残っています。">削除保留</span> : null}
        </span>
      </button>
    );
  };

  return (
    <>
      <div className="left-top">
        <div style={{ display: "flex", gap: 4 }}>
          <button className="btn-line grow" onClick={() => onAct("newChat")}><Icon name="plus" />新しいチャット</button>
          <button className="btn-line" title="チャット一覧を Codex から取り直します（読み取りのみ）" aria-label="一覧を更新" onClick={() => onAct("refreshList")}>更新</button>
        </div>
        {listStatus ? <div className="small" role="status" style={{ color: listStatus.ok ? "var(--ink3)" : "var(--fail)" }}>{listStatus.text}</div> : null}
        <input type="search" placeholder="名前で検索" aria-label="チャットを名前で検索" value={query} onChange={(e) => setQuery(e.target.value)} />
        <label className="small"><input type="checkbox" checked={inclArch} onChange={(e) => setInclArch(e.target.checked)} />アーカイブも含める</label>
      </div>
      <div className="left-scroll">
        {showArch ? (
          <>
            <div className="list-h"><span>アーカイブ</span><button className="small" style={{ height: 20 }} onClick={() => setShowArch(false)}>戻る</button></div>
            {rest.length ? rest.map(row) : <div className="empty-note">アーカイブしたチャットはありません</div>}
          </>
        ) : (
          <>
            {pinned.length ? <><div className="list-h"><span>ピン留め</span></div>{pinned.map(row)}</> : null}
            <div className="list-h"><span>最近使った順</span></div>
            {rest.length ? rest.map(row) : <div className="empty-note">{q ? "一致するチャットはありません" : "まだチャットがありません"}</div>}
          </>
        )}
      </div>
      <div className="left-foot">
        <button className="small" style={{ height: 22 }} onClick={() => setShowArch(true)}><Icon name="archive" />アーカイブ（{snap.chats.filter((c) => !removed(c) && archived(c)).length}）</button>
        <span className="only-narrow"><button className="small" style={{ height: 22 }} onClick={() => onAct("toggleLeft")}>閉じる</button></span>
      </div>
    </>
  );
}

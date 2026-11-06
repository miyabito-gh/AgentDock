import { useEffect, useMemo, useRef, useState } from "react";
import type { Chat, ChatLocalView, HostSnapshot, PendingRequest, RequestAnswer, TurnRecord } from "../ipc/types";
import { Icon } from "./Icon";
import { Messages, sendLockReason, type FileActions, type FindState } from "./CenterPane";
import { chatAgents, chatRequests, chatTitle, isRunning, rootView, sortChats } from "./derive";
import { chatGapRequests } from "./org";
import { showKnown, keyStr, knownValue } from "./format";
import { resendCtxOf } from "./ResendDialog";
import { firstSendLockReason, type PendingSend } from "./pendingSend";
import { histView, type HistLoad } from "./historyLoad";
import type { DecisionItem } from "./decisions";
import type { SentLocal } from "./chips";
import type { EnterMode } from "./sendKeys";

const NO_FIND: FindState = { open: false, query: "", setQuery: () => undefined, tick: 0, partial: false, close: () => undefined };
const NO_THREADS: never[] = [];
const RECENT_MAX = 8;

/** 右ペインに出すチャットの候補（待ち→作業中→最近の順。中央のチャットは除く）。 */
export function peekCandidates(snap: HostSnapshot, centerId: string | null, excludeIds?: ReadonlySet<string>): { wait: Chat[]; run: Chat[]; recent: Chat[] } {
  const hidden = (c: Chat) => {
    if (excludeIds?.has(c.key.id)) return true; // 専用の編集会話は左の一覧と同じく出さない
    const l = snap.chatLocals.find((x) => x.chat.id === c.key.id);
    return l?.visibility.kind === "archived" || knownValue(c.archived) === true || !!l?.org || !!l?.deletePending;
  };
  const all = sortChats(snap.chats.filter((c) => c.key.id !== centerId && !hidden(c)));
  const wait = all.filter((c) => chatRequests(snap, c).length > 0);
  const waitIds = new Set(wait.map((c) => c.key.id));
  const run = all.filter((c) => !waitIds.has(c.key.id) && isRunning(snap, c));
  const used = new Set([...waitIds, ...run.map((c) => c.key.id)]);
  return { wait, run, recent: all.filter((c) => !used.has(c.key.id)).slice(0, RECENT_MAX) };
}

/** 右ペインの切替（ドック／チャット）。待ちがあるときは印を付けるだけで、自動では切り替えない。 */
export function RightTabs({ view, onView, waiting }: { view: "dock" | "chat"; onView: (v: "dock" | "chat") => void; waiting: boolean }) {
  return (
    <div className="rp-tabs">
      <span className="seg" role="group" aria-label="右ペインの表示">
        <button aria-pressed={view === "dock"} onClick={() => onView("dock")}>ドック</button>
        <button aria-pressed={view === "chat"} onClick={() => onView("chat")}>チャット{waiting && view !== "chat" ? <i className="rp-dot" aria-label="対応待ちがあります" /> : null}</button>
      </span>
    </div>
  );
}

export function MiniChat({ snap, chat, centerId, excludeIds, turns, history, onReload, local, pending, decisions, sends, files, draft, setDraft, enterMode, connected, onSend, onRespond, onPick, onOpenCenter }: {
  snap: HostSnapshot;
  chat: Chat | null;
  /** 中央に出ているチャット（同じ会話を2か所で開かない）。 */
  centerId: string | null;
  /** 候補に出さない会話（専用の編集会話）。 */
  excludeIds?: ReadonlySet<string>;
  turns: TurnRecord[];
  history: HistLoad | undefined;
  onReload: () => void;
  local: ChatLocalView | undefined;
  pending: PendingSend[];
  decisions: DecisionItem[];
  sends: SentLocal[];
  files: FileActions;
  draft: string;
  setDraft: (v: string) => void;
  enterMode: EnterMode;
  connected: boolean;
  /** 中央の送信と同じ経路。受け付けたら true。 */
  onSend: (chat: Chat, text: string) => Promise<boolean>;
  onRespond: (r: PendingRequest, a: RequestAnswer) => void;
  onPick: (id: string | null) => void;
  onOpenCenter: (id: string) => void;
}) {
  const [menu, setMenu] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (!menu) return;
    const out = (e: MouseEvent) => { if (!wrap.current?.contains(e.target as Node)) setMenu(false); };
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape") setMenu(false); };
    document.addEventListener("mousedown", out);
    document.addEventListener("keydown", esc);
    return () => { document.removeEventListener("mousedown", out); document.removeEventListener("keydown", esc); };
  }, [menu]);
  const cand = useMemo(() => peekCandidates(snap, centerId, excludeIds), [snap, centerId, excludeIds]);
  const same = !!chat && chat.key.id === centerId;
  const shown = chat && !same ? chat : null;

  const title = shown ? chatTitle(shown) : null;
  const lock = shown ? (sendLockReason(snap, shown, local) ?? firstSendLockReason(pending)) : null;
  const running = shown ? isRunning(snap, shown) : false;
  const hv = histView(history, turns.length > 0);
  const nameOf = (a: { id: string }): string => {
    const v = shown ? chatAgents(snap, shown).find((x) => x.agent.key.id === a.id) : undefined;
    return v ? showKnown(v.agent.displayName) : a.id;
  };
  const send = async () => {
    if (!shown || lock || !connected || !draft.trim()) return;
    if (await onSend(shown, draft)) setDraft("");
  };
  const row = (c: Chat, tag?: string) => (
    <button key={c.key.id} type="button" role="menuitem" className="rp-item" onClick={() => { setMenu(false); onPick(c.key.id); }}>
      <span className="nm">{chatTitle(c).text}</span>{tag ? <span className="mark pend">{tag}</span> : null}
    </button>
  );
  const group = (label: string, list: Chat[], tag?: string) => (list.length ? <div className="rp-grp" key={label}><div className="rp-h">{label}</div>{list.map((c) => row(c, tag))}</div> : null);
  const empty = !cand.wait.length && !cand.run.length && !cand.recent.length;

  return (
    <div className="rp-chat">
      <div className="rp-head" ref={wrap}>
        <button type="button" className="rp-pick" aria-haspopup="menu" aria-expanded={menu} title="右に表示するチャットを選ぶ" onClick={() => setMenu((v) => !v)}>
          <span className="nm" style={title && !title.confirmed ? { color: "var(--ink3)" } : undefined}>{title ? title.text : "チャットを選ぶ"}</span>
          <span aria-hidden="true">▾</span>
        </button>
        {shown ? <button type="button" className="rp-ico" aria-label="中央で開く" title="中央で開く（中央のチャットは入れ替わります）" onClick={() => onOpenCenter(shown.key.id)}><Icon name="expand" /></button> : null}
        {shown ? <button type="button" className="rp-ico" aria-label="右の表示を外す" title="右の表示を外す" onClick={() => onPick(null)}><Icon name="x" /></button> : null}
        {menu ? (
          <div className="rp-menu dropdown" role="menu">
            {group("対応待ち", cand.wait, "待")}
            {group("作業中", cand.run)}
            {group("最近", cand.recent)}
            {empty ? <div className="empty-note">ほかのチャットがありません</div> : null}
          </div>
        ) : null}
      </div>
      {same ? (
        <div className="rp-empty">中央で開いているチャットです。別のチャットを選んでください。</div>
      ) : !shown ? (
        <div className="rp-empty">{empty ? "ほかのチャットがありません" : "上のメニューか、一覧の「⋯ → 右に表示」で選びます。"}</div>
      ) : (
        <>
          {hv.kind === "failed" ? <div className="rp-empty">{hv.text}<button type="button" className="small" onClick={onReload}>再取得</button></div> : hv.kind === "loading" ? <div className="rp-empty">読み込み中…</div> : null}
          <Messages key={shown.key.id} mini orgBound={false} headKey={shown.key} threads={local?.conversation?.threads ?? NO_THREADS} chatId={shown.key.id} agentName={(a) => nameOf(a)}
            decisions={decisions} turns={turns} reqs={chatRequests(snap, shown)} gaps={chatGapRequests(snap, shown.key)} running={running} onRespond={onRespond} onAct={() => undefined} find={NO_FIND}
            locals={snap.attachments.filter((a) => keyStr(a.chat) === keyStr(shown.key))} files={files} sends={sends}
            pending={pending} onPendingRestore={() => undefined} onPendingDismiss={() => undefined}
            resend={{ ctx: resendCtxOf(snap, shown, snap.sources.some((x) => x.connection.kind === "connected"), turns), rootAgentId: rootView(snap, shown)?.agent.key.id }} />
          <div className="rp-compose">
            <textarea ref={input} rows={2} value={draft} placeholder={lock ?? (running ? "作業中です。送ると完了後の依頼になります" : "返信を入力")} aria-label="返信"
              disabled={!!lock} onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.nativeEvent.isComposing || e.key !== "Enter") return;
                const mod = e.ctrlKey || e.metaKey;
                if (enterMode === "ctrl" ? e.ctrlKey : !e.shiftKey && !mod) { e.preventDefault(); void send(); }
              }} />
            <button type="button" className="primary send" aria-label="送信" title={lock ?? "送信"} disabled={!!lock || !connected || !draft.trim()} onClick={() => void send()}><Icon name="send" /></button>
          </div>
        </>
      )}
    </div>
  );
}

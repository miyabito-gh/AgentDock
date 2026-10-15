import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentKey, AgentView, Chat, ChatKey, HistoryOutcome, HostSnapshot, MonitorScope, PendingRequest, RequestAnswer } from "../ipc/types";
import { RequestCard, chatChipClass } from "./CenterPane";
import { cardDensity, chatSignature, mayRedirect, orderByRecency, pickRecentDescendants, selectTasks, stableMembership, stableOrder, trackRecency, type Recency } from "./dockTasks";
import { Icon } from "./Icon";
import { Popover } from "./Popover";
import type { Known } from "../ipc/types";
import { chatAgents, chatDisplay, chatRequests, chatStatusText, descendantStates, historyOutcomeOf, chatName, isDoneLike, isRunning, isWorkingForDock, previewTitle, rootView } from "./derive";
import { CHAT_DISPLAY_TEXT, HISTORY_OUTCOME_TEXT, historyBreakdown, historyBreakdownText, parkGroup, rawStateText, sortByDockRank, unknownCount, unknownNote, unknownSignature } from "./chatState";
import { FRESH, SOURCE_LABEL, STATE, hms, keyStr, showKnown } from "./format";
import { TitleBar } from "./Chrome";
import { ModelBadge } from "./ModelBadge";
import { SendStatusBar } from "./SendStatusBar";
import type { StatusInput } from "./sendStatus";

function waitText(a: AgentView): string | null {
  const w = a.status.wait;
  if (!w) return null;
  switch (w.reason.kind) {
    case "approval": return "承認待ち";
    case "userInput": return "質問への回答待ち";
    case "child": return "子の完了待ち";
    case "other": return `待機中（${w.reason.raw}）`;
  }
}

/** 活動の表示。値が本当に無ければ「活動なし」、取得していなければ「未確認」（欠損の語は出さない）。 */
function activityText(k: Known<string>): string {
  if (k.kind === "value") return k.value;
  return k.kind === "notFetched" ? "未確認" : k.kind === "unsupported" ? "このAIでは非対応" : "活動なし";
}

/** 状態不明カードの「確認済みにして隠す」の署名（新しい活動・turnが届けば変わる）。 */
const sigOf = (v: AgentView): string => unknownSignature(v.status.raw.label, v.agent.latestTurn, v.currentActivity ? v.currentActivity.evidence.observedAt : null);

function Berth({ a, depth, orphan, mini, confirmed, onConfirmFail, rootName, childNote, onDismiss, outcome, outcomeAt }: {
  outcome?: HistoryOutcome | null; outcomeAt?: number; rootName?: string; childNote?: string; a: AgentView; depth: number; orphan: boolean; mini: boolean; confirmed: boolean; onConfirmFail: (key: string) => void;
  onDismiss?: () => void;
}) {
  // 状態不明でも履歴で終端を確認できたものは、別ラベルで出す(状態は書き換えない。live未確認)
  const m = outcome ? { c: outcome === "failed" ? STATE.failed.c : outcome === "interrupted" ? STATE.interrupted.c : STATE.done.c, t: HISTORY_OUTCOME_TEXT[outcome] } : STATE[a.status.state];
  const rel = orphan ? "親不明" : ["メイン", "子", "孫", "ひ孫"][depth] ?? `${depth}階層下`;
  const notLive = a.freshness !== "live";
  const act = waitText(a) ?? (a.currentActivity ? activityText(a.currentActivity.summary) : a.status.state === "unknown" ? rawStateText(a.status.raw.label) : "活動なし");
  const failUnconfirmed = a.status.state === "failed" && !confirmed;
  return (
    <div className={`card berth ${m.c} ${notLive ? "stale" : ""}`}>
      <div className="c1">
        <span className="nm" title={a.agent.parent.kind === "root" ? undefined : "Codexが付けた呼び名です（役割ではありません）"}>{a.agent.parent.kind === "root" ? (rootName ?? showKnown(a.agent.displayName)) : showKnown(a.agent.displayName)}</span>
        <span className="rel" title={a.agent.parent.kind === "root" ? "役割: メイン（会話の本体）" : undefined}>{rel}</span>
        {a.agent.parent.kind === "root" ? null : <ModelBadge model={a.agent.model} effort={a.agent.effort} spawn={a.agent.spawnSpec} compact={mini} />}
        <span className={`st ${m.c}${outcome ? " hist" : ""}`}><i aria-hidden="true" />{m.t}</span>
      </div>
      {childNote || (a.agent.parent.kind !== "root" && a.agent.agentPath.kind === "value") || (!mini && a.agent.parent.kind !== "root") ? (
        <div className="c2">
          {childNote ? <span className="kvp" title="親のみ表示のため、子孫を含めたチャットの状態を示します">{childNote}</span> : null}
          {a.agent.parent.kind !== "root" && a.agent.agentPath.kind === "value" ? <span className="path mono" title="Codexが返したエージェントの経路（呼び名とは別）">{a.agent.agentPath.value}</span> : null}
          {mini || a.agent.parent.kind === "root" ? null : (
            <>
              <span className="kvp" title={a.agent.role.kind === "missing" ? "Codexが返していません" : undefined}>役割: {a.agent.role.kind === "missing" ? "未提供" : showKnown(a.agent.role)}</span>
              <span className="kvp">担当: {showKnown(a.agent.assignment)}</span>
            </>
          )}
        </div>
      ) : null}
      <div className="c3" title={act}>{act}</div>
      {mini && !outcome ? null : (
        <div className="c4">
          {mini ? null : (
            <>
              <span>{hms(a.status.evidence.observedAt)}</span>
              <span>{SOURCE_LABEL[a.status.evidence.source]}</span>
            </>
          )}
          {outcome ? <span>live未確認・取得{outcomeAt !== undefined ? hms(outcomeAt) : "時刻不明"}（履歴の読取りで確認）</span> : null}
          {notLive && !mini ? (
            a.agent.parent.kind !== "root" && a.freshness === "historyOnly"
              ? <span className="stale-t" title="状態は走査で更新（約3秒間隔）。子エージェントはライブ通知ではなく、走査（約3秒間隔の読み取り）で状態を更新しています">走査で更新</span>
              : <span className="stale-t">{FRESH[a.freshness].t}</span>
          ) : null}
          {onDismiss && !mini ? (
            <button className="btn sm" aria-label="確認済みにして隠す" title="この画面の表示だけを隠します。Codexやホストの状態・判定は変わりません。新しい活動が届くと再表示します" onClick={onDismiss}><Icon name="check" />隠す</button>
          ) : null}
          {failUnconfirmed && !mini ? (
            <button className="btn sm" aria-label="失敗を確認済みにする" title="失敗を確認済みにする" onClick={() => onConfirmFail(keyStr(a.agent.key))}><Icon name="check" />確認済み</button>
          ) : null}
        </div>
      )}
    </div>
  );
}

/** 親のみ表示のときに、子孫込みの状態を親カードへ添える（子孫の状態が親と同じ扱いなら添えない）。 */
function parentNote(snap: HostSnapshot, c: Chat): string | undefined {
  const d = chatDisplay(snap, c);
  const n = unknownCount(descendantStates(snap, c));
  const base = d === "root" ? undefined : CHAT_DISPLAY_TEXT[d];
  return n > 0 ? [base, unknownNote(n)].filter(Boolean).join("／") : base;
}

function RootBlock({ snap, c, views, showHead, selected, mini, parentOnly, confirmed, dismissed, onDismiss, onOpen, onConfirmFail, onRecheck }: {
  snap: HostSnapshot; c: Chat; views: AgentView[]; showHead: boolean; selected: boolean; mini: boolean; parentOnly: boolean;
  confirmed: Set<string>; dismissed: Map<string, string>; onDismiss: (items: AgentView[]) => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void;
  onRecheck?: (chat: ChatKey, agents: AgentKey[]) => Promise<void>;
}) {
  const [rechecking, setRechecking] = useState(false);
  const [openUnknown, setOpenUnknown] = useState(false);
  const [openHistory, setOpenHistory] = useState(false);
  const inList = new Set(views.map((v) => keyStr(v.agent.key)));
  const st = (v: AgentView) => v.status.state;
  const childrenOf = (k: string) => views.filter((v) => v.agent.parent.kind === "explicit" && keyStr(v.agent.parent.parent) === k);
  const outcomeOf = (v: AgentView) => historyOutcomeOf(snap, v);
  const atOf = (v: AgentView) => snap.historyConfirmations.find((h) => h.agent.id === v.agent.key.id)?.confirmedAt;
  const subtreeAllUnknown = (v: AgentView): boolean => st(v) === "unknown" && childrenOf(keyStr(v.agent.key)).every(subtreeAllUnknown);
  // 状態不明の子孫（自分以下も全て不明）は末尾の折りたたみへ。親（メイン）は対象外。確認済みにして隠したものは、新しい活動が届くまで出さない。
  const parked = (v: AgentView) => v.agent.parent.kind !== "root" && subtreeAllUnknown(v);
  const isDismissed = (v: AgentView) => dismissed.get(keyStr(v.agent.key)) === sigOf(v);
  const groupOf = (v: AgentView) => (parked(v) ? parkGroup(st(v), outcomeOf(v)) : "tree");
  const parkedAll = views.filter((v) => groupOf(v) === "unknown");
  const parkedShown = parkedAll.filter((v) => !isDismissed(v));
  const parkedHidden = parkedAll.length - parkedShown.length;
  const histAll = views.filter((v) => groupOf(v) === "history");
  const histShown = histAll.filter((v) => !isDismissed(v));
  const histHidden = histAll.length - histShown.length;
  const histB = historyBreakdown(histShown.map((v) => outcomeOf(v)!));
  const kids = (k: string) => sortByDockRank(childrenOf(k).filter((v) => groupOf(v) === "tree"), st, outcomeOf);
  const roots = sortByDockRank(views.filter((v) => (v.agent.parent.kind === "root" || (v.agent.parent.kind === "explicit" && !inList.has(keyStr(v.agent.parent.parent)))) && groupOf(v) === "tree"), st, outcomeOf);
  const orphans = sortByDockRank(views.filter((v) => v.agent.parent.kind === "unknown" && groupOf(v) === "tree"), st, outcomeOf);
  const depthOf = (v: AgentView): number => {
    let d = 0; let p = v.agent.parent;
    while (p.kind === "explicit") {
      d++;
      const pk = keyStr(p.parent);
      const next = chatAgents(snap, c).find((x) => keyStr(x.agent.key) === pk);
      if (!next) break;
      p = next.agent.parent;
    }
    return d;
  };
  const node = (v: AgentView) => {
    const k = keyStr(v.agent.key);
    const ch = parentOnly ? [] : kids(k);
    return (
      <li key={k}>
        <Berth a={v} outcome={outcomeOf(v)} outcomeAt={atOf(v)} depth={depthOf(v)} orphan={false} mini={mini} confirmed={confirmed.has(k)} onConfirmFail={onConfirmFail} childNote={parentOnly && v.agent.parent.kind === "root" ? parentNote(snap, c) : undefined} rootName={v.agent.parent.kind === "root" ? (v.agent.displayName.kind === "value" ? v.agent.displayName.value : previewTitle(c) ?? "メイン") : undefined} />
        {ch.length ? <ul className="tree">{ch.map(node)}</ul> : null}
      </li>
    );
  };
  return (
    <div>
      {showHead ? (
        <div className="root-h">
          <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{chatName(c)}</span>
          {!selected ? <button onClick={() => onOpen(c.key.id)}>開く</button> : null}
        </div>
      ) : null}
      <ul className="tree">{roots.map(node)}</ul>
      {histAll.length ? (
        <div style={{ margin: "6px 0 0" }}>
          <div className="grp-h">
            <button className={`btn-line small ${histB.attention ? "attn" : ""}`} aria-expanded={openHistory} title="履歴の読取りだけで終端を確認した子です(live確認ではありません)" onClick={() => setOpenHistory(!openHistory)}>{openHistory ? "▼" : "▶"} 履歴のみで確認した子 {histShown.length}件（{historyBreakdownText(histB)}）{histHidden ? `／確認済み${histHidden}件は非表示` : ""}</button>
            {openHistory && histShown.length && !mini ? <button className="btn-line small" title="この画面の表示だけを隠します。ホストの状態・判定は変わりません" onClick={() => onDismiss(histShown)}>まとめて確認済みにして隠す</button> : null}
          </div>
          {openHistory ? (
            <ul className="tree">{histShown.map((v) => (
              <li key={keyStr(v.agent.key)}>
                <Berth a={v} outcome={outcomeOf(v)} outcomeAt={atOf(v)} depth={depthOf(v)} orphan={v.agent.parent.kind === "unknown"} mini={mini} confirmed={false} onConfirmFail={onConfirmFail} onDismiss={() => onDismiss([v])} />
              </li>
            ))}</ul>
          ) : null}
        </div>
      ) : null}
      {parkedAll.length ? (
        <div style={{ margin: "6px 0 0" }}>
          <div className="grp-h">
            <button className="btn-line small" aria-expanded={openUnknown} onClick={() => setOpenUnknown(!openUnknown)}>{openUnknown ? "▼" : "▶"} 状態不明の子 {parkedShown.length}件{parkedHidden ? `（確認済み${parkedHidden}件は非表示）` : ""}</button>
            {openUnknown && parkedShown.length && !mini ? <button className="btn-line small" title="この画面の表示だけを隠します。ホストの状態・判定は変わりません" onClick={() => onDismiss(parkedShown)}>まとめて確認済みにして隠す</button> : null}
            {parkedShown.length && !mini && onRecheck ? (
              <button className="btn-line small" disabled={rechecking} aria-busy={rechecking} title="保存履歴を読むだけです。再開・停止はしません。確認できなければ状態不明のままです"
                onClick={() => { setRechecking(true); void onRecheck(c.key, parkedShown.map((v) => v.agent.key)).finally(() => setRechecking(false)); }}>{rechecking ? "確認中…" : "履歴で再確認"}</button>
            ) : null}
          </div>
          {openUnknown ? (
            <ul className="tree">{parkedShown.map((v) => (
              <li key={keyStr(v.agent.key)}>
                <Berth a={v} depth={depthOf(v)} orphan={v.agent.parent.kind === "unknown"} mini={mini} confirmed={false} onConfirmFail={onConfirmFail} onDismiss={() => onDismiss([v])} />
              </li>
            ))}</ul>
          ) : null}
        </div>
      ) : null}
      {orphans.length ? (
        <>
          <div className="small muted" style={{ margin: "6px 0 0" }}>直接の親が分からないエージェント</div>
          <ul className="tree">{orphans.map((v) => (
            <li key={keyStr(v.agent.key)}>
              <Berth a={v} outcome={outcomeOf(v)} outcomeAt={atOf(v)} depth={0} orphan mini={mini} confirmed={false} onConfirmFail={onConfirmFail} />
            </li>
          ))}</ul>
        </>
      ) : null}
    </div>
  );
}

/** ドックのカードから呼ぶ操作（本窓のみ。監視窓には渡さない）。送信・回答は中央と同じ経路で、押した1件だけに作用する。 */
export interface DockActions {
  draftOf: (c: Chat) => string;
  onDraft: (c: Chat, text: string) => void;
  /** 実行中なら「完了後に送る」登録、そうでなければ新しいturn。結果の案内は呼び出し側が出す。 */
  send: (c: Chat, text: string) => Promise<boolean>;
  /** 送れない理由（無ければ null）。 */
  block: (c: Chat) => string | null;
  /** 直近の送信の結果（1行）。 */
  note: (c: Chat) => string | null;
  /** 段階表示の入力（送信した・受理待ち・応答待ち・キュー）。中央の入力欄の表示と同じ規則。 */
  status: (c: Chat) => StatusInput;
  respond: (r: PendingRequest, a: RequestAnswer) => void;
}
/** 通知を押したチャット（tick が変わるたびに1回）。 */
export interface DockFocus { id: string; tick: number }

type DockView = "task" | "sel" | "all";
const cid = (c: Chat): string => `${c.key.backend}:${c.key.id}`;

/** いまの入力状態（フォーカスが文字入力欄にあり、変換中か文字がある間は、スクロール・展開・並べ替えをしない）。 */
function inputState(composing: boolean): { focusInInput: boolean; composing: boolean; hasText: boolean } {
  const el = document.activeElement;
  const isText = el instanceof HTMLTextAreaElement || (el instanceof HTMLInputElement && ["text", "search", ""].includes(el.type));
  return { focusInInput: isText, composing, hasText: isText && el.value.length > 0 };
}

/** 入力中か（真の間は並べ替えを保留する）。画面のどの入力欄でも同じ判定。 */
function useTyping(): { typing: boolean; now: () => boolean } {
  const composing = useRef(false);
  const [typing, setTyping] = useState(false);
  const ref = useRef(false);
  useEffect(() => {
    const recompute = () => { const t = !mayRedirect(inputState(composing.current)); ref.current = t; setTyping(t); };
    const later = () => window.setTimeout(recompute, 0); // フォーカス移動の後の activeElement を見る
    const start = () => { composing.current = true; recompute(); };
    const end = () => { composing.current = false; later(); };
    document.addEventListener("focusin", later, true);
    document.addEventListener("focusout", later, true);
    document.addEventListener("input", recompute, true);
    document.addEventListener("compositionstart", start, true);
    document.addEventListener("compositionend", end, true);
    return () => {
      document.removeEventListener("focusin", later, true);
      document.removeEventListener("focusout", later, true);
      document.removeEventListener("input", recompute, true);
      document.removeEventListener("compositionstart", start, true);
      document.removeEventListener("compositionend", end, true);
    };
  }, []);
  return { typing, now: () => !mayRedirect(inputState(composing.current)) };
}

function depthIn(snap: HostSnapshot, c: Chat, v: AgentView): number {
  let d = 0; let p = v.agent.parent;
  while (p.kind === "explicit") {
    d++;
    const pk = keyStr(p.parent);
    const next = chatAgents(snap, c).find((x) => keyStr(x.agent.key) === pk);
    if (!next) break;
    p = next.agent.parent;
  }
  return d;
}

/** タスクカード（1チャット）。状態は書き換えず、親の状態・活動を既存の文言のまま見せる。 */
function TaskCard({ snap, c, band, line, open, marked, mini, at, confirmed, actions, onToggle, onClose, onOpen, onConfirmFail }: {
  snap: HostSnapshot; c: Chat; band: boolean; line: boolean; open: boolean; marked: boolean; mini: boolean; at: number | undefined; confirmed: Set<string>;
  actions?: DockActions; onToggle: () => void; onClose?: () => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void;
}) {
  const [allKids, setAllKids] = useState(false);
  const root = rootView(snap, c);
  const working = isWorkingForDock(snap, c);
  const reqs = chatRequests(snap, c);
  const body = !line || open;
  const kids = pickRecentDescendants(snap, c, allKids ? Infinity : undefined);
  const act = root?.currentActivity ? activityText(root.currentActivity.summary) : root ? "活動なし" : "未確認";
  const nameOf = (a: AgentKey): string => { const v = chatAgents(snap, c).find((x) => keyStr(x.agent.key) === keyStr(a)); return v ? showKnown(v.agent.displayName) : a.id; };
  const notLive = !!root && root.freshness !== "live";
  const canWrite = !!actions && !mini;
  const draft = canWrite ? actions.draftOf(c) : "";
  const block = canWrite ? actions.block(c) : null;
  const note = canWrite ? actions.note(c) : null;
  const run = isRunning(snap, c);
  const [busy, setBusy] = useState(false);
  const ta = useRef<HTMLTextAreaElement>(null);
  // 指示欄は1〜3行（約66px）まで内容に合わせて伸びる（表示だけ）
  useLayoutEffect(() => {
    const el = ta.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 66)}px`;
  }, [draft, body]);
  const send = async () => {
    if (!actions || busy || block || !draft.trim()) return;
    setBusy(true);
    try { if (await actions.send(c, draft)) actions.onDraft(c, ""); } finally { setBusy(false); }
  };
  const chip = chatChipClass(snap, c);
  const stateText = chatStatusText(snap, c);
  return (
    <section className={`tcard ${chip} ${band ? "band" : ""} ${working ? "working" : "ended"} ${line ? "line" : ""}`} data-chat={cid(c)} aria-label={`${chatName(c)}のカード`}>
      <div className="tc-h">
        {line ? (
          <button className="tc-line" aria-expanded={open} title={`${stateText}。押すとこのカードだけ展開します`} onClick={onToggle}>
            <span className={`st ${chip}`}><i aria-hidden="true" />{stateText}</span>
            <span className="tc-name">{chatName(c)}</span>
            {marked ? <span className="tc-mark" role="img" aria-label="新しい動きがあります">●</span> : null}
            {at ? <span className="tc-at">{hms(at)}</span> : null}
          </button>
        ) : (
          <>
            <span className="tc-name">{chatName(c)}</span>
            {marked ? <span className="tc-mark" role="img" aria-label="新しい動きがあります">●</span> : null}
            <span className={`st ${chip}`}><i aria-hidden="true" />{stateText}</span>
            {at ? <span className="tc-at">{hms(at)}</span> : null}
          </>
        )}
        <button className="small" onClick={() => onOpen(c.key.id)} title="中央でこのチャットを開きます">開く</button>
        {onClose ? <button className="small" aria-label={`${chatName(c)}のカードを閉じる`} title="この画面から外します（新しい活動があれば再表示します。会話・作業には影響しません）" onClick={onClose}><Icon name="x" /></button> : null}
      </div>
      {reqs.length ? (
        <div className="tc-reqs">
          {reqs.map((r) => (mini || !actions
            ? <div key={`${r.key.source}/${r.key.requestId}`} className="tc-req-mini"><Icon name="alert" />{r.kind.kind === "userInput" ? "質問" : "承認"}の回答待ち: {r.detail.summary}（通常画面で回答します）</div>
            : <RequestCard key={`${r.key.source}/${r.key.requestId}`} r={r} onRespond={(q, a) => actions.respond(q, a)} agentName={nameOf} />))}
        </div>
      ) : null}
      {body ? (
        <>
          {notLive ? <div className="tc-fresh">{FRESH[root.freshness].t}</div> : null}
          <div className={`tc-act ${working ? "" : "clamp"}`} title={act}>{act}</div>
          {working ? (
            <div className="tc-kids">
              {kids.shown.length ? (
                <ul className="tree">{kids.shown.map((v) => (
                  <li key={keyStr(v.agent.key)}>
                    <Berth a={v} outcome={historyOutcomeOf(snap, v)} outcomeAt={snap.historyConfirmations.find((h) => h.agent.id === v.agent.key.id)?.confirmedAt} depth={depthIn(snap, c, v)} orphan={v.agent.parent.kind === "unknown"} mini confirmed={confirmed.has(keyStr(v.agent.key))} onConfirmFail={onConfirmFail} />
                  </li>
                ))}</ul>
              ) : null}
              {kids.rest > 0 ? <button className="btn-line small" onClick={() => setAllKids(true)}>ほか{kids.rest}件を表示</button> : null}
              {allKids && kids.shown.length > 3 ? <button className="btn-line small" onClick={() => setAllKids(false)}>直近3件だけにする</button> : null}
            </div>
          ) : null}
          {canWrite ? (
            <div className="tc-send">
              <textarea ref={ta} rows={1} aria-label={`${chatName(c)}への次の指示`} value={draft} disabled={!!block}
                placeholder={run ? "完了後に送る次の依頼（Ctrl＋Enter）" : "次の指示（Ctrl＋Enter で送信）"}
                title={block ?? "下書きは中央の入力欄と共有しています"}
                onChange={(e) => actions.onDraft(c, e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey && !e.nativeEvent.isComposing) { e.preventDefault(); void send(); } }} />
              <button className="btn-main small" disabled={!!block || busy || !draft.trim()} title={block ?? (run ? "完了後に送る依頼として登録します" : "送信")} onClick={() => void send()}>{run ? "完了後に送る" : "送信"}</button>
              {busy && run ? <div className="tc-note" role="status"><span className="spinner" aria-hidden="true" /> 受け付けました（キューへ登録しています）</div> : null}
              <SendStatusBar compact input={actions.status(c)} />
              {block ? <div className="tc-note">{block}</div> : null}
              {note ? <div className="tc-note" role="status">{note}</div> : null}
            </div>
          ) : null}
        </>
      ) : null}
    </section>
  );
}

export function DockPane({ snap, scope, selId, mini, confirmed, onScope, onOpen, onConfirmFail, onAct, onRecheck, actions, focusReq }: {
  snap: HostSnapshot; scope: MonitorScope; selId: string | null; mini?: boolean; confirmed: Set<string>;
  /** 状態不明の子の「履歴で再確認」（本窓のみ。保存履歴を読むだけ）。結果の案内は呼び出し側が出す。 */
  onRecheck?: (chat: ChatKey, agents: AgentKey[]) => Promise<void>;
  /** カード内の指示欄・回答（本窓のみ。監視窓では渡さず、回答ボタンと指示欄を出さない）。 */
  actions?: DockActions; focusReq?: DockFocus | null;
  onScope: (s: MonitorScope) => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void; onAct: (a: string) => void;
}) {
  // 表示（タスク｜選択中｜全）と「親のみ」はウィンドウ内の記憶（ホストへは保存しない）。表示範囲の変更だけで送信先・中断・承認を変えない。
  const [view, setView] = useState<DockView>("task");
  const [parentOnly, setParentOnly] = useState(false);
  // 最近動かした順（UI側で求める。署名が変わったチャットだけ更新）
  const recRef = useRef<Recency>(new Map());
  const rec = useMemo(() => { const n = trackRecency(recRef.current, snap, Date.now()); recRef.current = n; return n; }, [snap]);
  const { typing, now: typingNow } = useTyping();
  // タスクカードの「閉じた」（閉じた時の署名。変われば再表示）・1行表示の展開・印（●）
  const [closed, setClosed] = useState<Map<string, string>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [marks, setMarks] = useState<Set<string>>(new Set());
  const taskOrder = useRef<string[]>([]);
  const bandOrder = useRef<string[]>([]);
  const allOrder = useRef<string[]>([]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [dim, setDim] = useState({ h: 9999, w: 9999 });
  useLayoutEffect(() => {
    const el = scrollRef.current?.parentElement;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => setDim((d) => (d.h === el.clientHeight && d.w === el.clientWidth ? d : { h: el.clientHeight, w: el.clientWidth }));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const line = cardDensity({ dockH: dim.h, dockW: dim.w }) === "line";
  // 状態不明の子の「確認済みにして隠す」: 画面内の記憶（ホスト・永続化なし）。署名が変われば再表示。
  const [dismissed, setDismissed] = useState<Map<string, string>>(new Map());
  const onDismiss = (items: AgentView[]) => setDismissed((m) => { const n = new Map(m); items.forEach((v) => n.set(keyStr(v.agent.key), sigOf(v))); return n; });
  const isSel = view === "sel";
  const isTask = view === "task";
  const showFinished = scope.kind === "allChats" && scope.showFinished;
  // 「終了済みも表示」は「全」だけで選べる。オンの項目はボタンの文字に出して、隠れた状態を見せる。
  const showFinishedOpt = view === "all";
  const onItems = [parentOnly ? "親のみ" : null, showFinishedOpt && showFinished ? "終了済み" : null].filter((s): s is string => s !== null);
  const viewLabel = onItems.length ? `表示: ${onItems.join("・")}` : "表示 ▾";
  const viewTitle = onItems.length ? `表示の設定（${onItems.join("・")}がオン）` : "表示の設定（親のみ表示・終了済みも表示）";
  // 「選択中」「全」: 全の並びは最近動かした順（入力中は並べ替えを保留）
  const allIds = stableOrder(allOrder.current, orderByRecency(snap.chats, rec).map(cid), typing);
  const allById = new Map(snap.chats.map((c) => [cid(c), c] as const));
  const chats = isSel ? snap.chats.filter((c) => c.key.id === selId) : allIds.flatMap((id) => allById.get(id) ?? []);
  const visibleOf = (c: Chat) => chatAgents(snap, c).filter((v) => {
    if (parentOnly) {
      if (v.agent.parent.kind !== "root") return false;
      if (isSel || isWorkingForDock(snap, c)) return true; // 子孫が作業中の親は、親自身が完了でも出す
    } else if (isSel) return true; // 選択中: 完了済みも常に表示
    const st = v.status.state;
    if (st === "failed") return confirmed.has(keyStr(v.agent.key)) ? showFinished : true;
    if (isDoneLike(st) || st === "idle") return showFinished;
    return true;
  });
  const blocks = isTask ? [] : chats.map((c) => ({ c, views: visibleOf(c) })).filter((b) => isSel || b.views.length);
  // 「タスク」: 帯（対応待ち）＋最近動かした順の最大5件＋ほか（1行）。完了しても消さない（閉じる・アーカイブ・削除だけ）。
  const sel = useMemo(
    () => (isTask ? selectTasks(snap, rec, closed, (c) => isWorkingForDock(snap, c)) : { attention: [], tasks: [], overflow: [] }),
    [isTask, snap, rec, closed],
  );
  // 入力中は、並びだけでなく所属（帯・タスク列）も保つ（カードを作り直さない＝フォーカス・変換を保つ）。
  const { band: bandIds, task: taskIds } = stableMembership(bandOrder.current, taskOrder.current, sel.attention.map(cid), sel.tasks.map(cid), typing, new Set(allById.keys()));
  useEffect(() => { bandOrder.current = bandIds; taskOrder.current = taskIds; allOrder.current = allIds; });
  const inTask = new Set([...bandIds, ...taskIds]);
  const bandChats = bandIds.flatMap((id) => allById.get(id) ?? []);
  const taskChats = taskIds.flatMap((id) => allById.get(id) ?? []);
  const overflow = [...sel.tasks, ...sel.overflow].filter((c) => !inTask.has(cid(c)));
  // 入力中に新しい対応待ちが来たら、位置は動かさず印だけ付ける
  const seenBand = useRef<Set<string>>(new Set());
  useEffect(() => {
    const fresh = sel.attention.map(cid).filter((id) => !seenBand.current.has(id));
    seenBand.current = new Set(sel.attention.map(cid));
    if (fresh.length && typingNow()) setMarks((m) => { const n = new Set(m); fresh.forEach((id) => n.add(id)); return n; });
  }, [sel.attention]); // eslint-disable-line react-hooks/exhaustive-deps
  // 通知を押したとき: 入力中でなければそのカードへスクロールして展開。入力中なら印だけ。
  useEffect(() => {
    if (!focusReq || !isTask) return;
    const id = `codex:${focusReq.id}`;
    if (!inTask.has(id)) return;
    if (typingNow()) { setMarks((m) => new Set(m).add(id)); return; }
    setExpanded((s) => new Set(s).add(id));
    window.requestAnimationFrame(() => scrollRef.current?.querySelector(`[data-chat="${id}"]`)?.scrollIntoView({ block: "nearest" }));
  }, [focusReq?.tick]); // eslint-disable-line react-hooks/exhaustive-deps
  const toggleOpen = (id: string) => {
    setExpanded((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
    setMarks((m) => { if (!m.has(id)) return m; const n = new Set(m); n.delete(id); return n; });
  };
  const closeCard = (c: Chat) => setClosed((m) => new Map(m).set(cid(c), rec.get(cid(c))?.sig ?? chatSignature(snap, c)));
  const card = (c: Chat, band: boolean) => (
    <TaskCard key={cid(c)} snap={snap} c={c} band={band} line={line} open={expanded.has(cid(c))} marked={marks.has(cid(c))} mini={!!mini} at={rec.get(cid(c))?.at}
      confirmed={confirmed} actions={mini ? undefined : actions} onToggle={() => toggleOpen(cid(c))} onClose={band ? undefined : () => closeCard(c)} onOpen={onOpen} onConfirmFail={onConfirmFail} />
  );
  return (
    <>
      <div className="dock-top">
        <span className="seg" role="group" aria-label="表示範囲">
          <button aria-pressed={isTask} aria-label="タスク" title="最近動かした順のタスク（完了しても残ります）" onClick={() => setView("task")}>タスク</button>
          <button aria-pressed={isSel} aria-label="選択中のチャット" title="選択中のチャット（完了したエージェントも表示しています）" onClick={() => { setView("sel"); onScope({ kind: "selectedChat", chat: selId ? { backend: "codex", id: selId } : null }); }}>選択中</button>
          <button aria-pressed={view === "all"} aria-label="全チャット" title="全チャット" onClick={() => { setView("all"); onScope({ kind: "allChats", showFinished }); }}>全</button>
        </span>
        {isTask ? null : (
          <Popover trigger={viewLabel} title={viewTitle} triggerClassName="btn subtle sm dock-view">
            <label className="small"><input type="checkbox" checked={parentOnly} onChange={(e) => setParentOnly(e.target.checked)} />親のみ表示（子・孫を隠す）</label>
            {showFinishedOpt ? (
              <label className="small"><input type="checkbox" checked={showFinished} onChange={(e) => onScope({ kind: "allChats", showFinished: e.target.checked })} />終了済みも表示（完了・停止・確認済みの失敗）</label>
            ) : null}
          </Popover>
        )}
        {mini ? null : (
          <Popover trigger="?" label="説明" title="状態は Codex から取得した情報です。取得できない項目は「未確認」と表示します。" triggerClassName="help" textual align="right">
            状態は Codex から取得した情報です。取得できない項目は「未確認」と表示します。
          </Popover>
        )}
        {mini ? null : <button className="only-narrow-r small" onClick={() => onAct("toggleRight")}>閉じる</button>}
      </div>
      <div className="dock-scroll" ref={scrollRef} style={{ "--dock-h": `${dim.h}px` } as React.CSSProperties}>
        {isTask ? (
          <>
            {bandChats.length ? (
              <div className="dock-band" role="region" aria-label="対応待ち">
                <div className="dock-band-h">対応待ち {bandChats.length}件</div>
                {bandChats.map((c) => card(c, true))}
              </div>
            ) : null}
            {taskChats.map((c) => card(c, false))}
            {overflow.length ? (
              <div className="dock-more">
                <div className="dock-more-h">ほかのタスク {overflow.length}件</div>
                {overflow.map((c) => {
                  const chip = chatChipClass(snap, c);
                  const at = rec.get(cid(c))?.at;
                  return (
                    <button key={cid(c)} className="tc-line tc-over" title="中央でこのチャットを開きます" onClick={() => onOpen(c.key.id)}>
                      <span className={`st ${chip}`}><i aria-hidden="true" />{chatStatusText(snap, c)}</span>
                      <span className="tc-name">{chatName(c)}</span>
                      {at ? <span className="tc-at">{hms(at)}</span> : null}
                    </button>
                  );
                })}
              </div>
            ) : null}
            {!bandChats.length && !taskChats.length && !overflow.length ? <div className="empty-note">{snap.chats.length ? "表示するタスクはありません（閉じたものは新しい活動で戻ります）" : "チャットを始めると、ここにタスクが表示されます"}</div> : null}
          </>
        ) : blocks.length ? blocks.map(({ c, views }) => (
          <RootBlock key={keyStr(c.key)} snap={snap} c={c} views={views} showHead={!isSel || !!mini} selected={c.key.id === selId} mini={!!mini} parentOnly={parentOnly}
            confirmed={confirmed} dismissed={dismissed} onDismiss={onDismiss} onOpen={onOpen} onConfirmFail={onConfirmFail} onRecheck={onRecheck} />
        )) : <div className="empty-note">{snap.chats.length ? "作業中・対応待ちのエージェントはありません" : "チャットを始めると、ここにエージェントが表示されます"}</div>}
      </div>
    </>
  );
}

export function MiniWindow(props: Parameters<typeof DockPane>[0] & { top: boolean }) {
  return (
    <div className="mini" role="dialog" aria-label="コンパクト監視窓">
      <TitleBar title="AgentDock 監視" mini top={props.top} onAct={props.onAct} />
      <DockPane {...props} mini />
    </div>
  );
}

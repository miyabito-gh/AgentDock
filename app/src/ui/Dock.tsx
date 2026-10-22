import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentKey, AgentView, Chat, ChatKey, HistoryOutcome, HostSnapshot, MonitorScope, PendingRequest, RequestAnswer } from "../ipc/types";
import { RequestCard, chatChipClass } from "./CenterPane";
import { DESCENDANT_LIMIT, branchOrder, cardDensity, chatSignature, descendantSummary, descendantSummaryText, mayRedirect, orderWorkingFirst, selectTasks, stableMembership, stableOrder, trackRecency, type Recency } from "./dockTasks";
import { Icon } from "./Icon";
import { OrgBadge } from "./OrgPane";
import { RequestGapLine } from "./RequestGap";
import { chatGapRequests } from "./org";
import { Popover } from "./Popover";
import type { Known } from "../ipc/types";
import { chatAgents, chatDisplay, chatRequests, chatStatusText, descendantStates, historyOutcomeOf, chatName, isDoneLike, isUnknownForDock, isWorkingForDock, previewTitle, rootView } from "./derive";
import { CHAT_DISPLAY_TEXT, isActiveState, HISTORY_OUTCOME_TEXT, historyBreakdown, historyBreakdownText, parkGroup, rawStateText, sortByDockRank, unknownCount, unknownNote, unknownSignature } from "./chatState";
import { FRESH, SOURCE_LABEL, STATE, hms, keyStr, showKnown } from "./format";
import { TitleBar } from "./Chrome";
import { ModelBadge } from "./ModelBadge";
import { useThrottled } from "./useThrottled";

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

/** 活動の行は1行固定（高さが伸び縮みしない）。長い文は先頭だけ出し、全文は title で読む。 */
const ACT_MAX = 60;
function shortAct(s: string): string {
  const one = s.replace(/\s+/g, " ").trim();
  return one.length > ACT_MAX ? `${one.slice(0, ACT_MAX)}…` : one;
}

/** 状態不明カードの「確認済みにして隠す」の署名（新しい活動・turnが届けば変わる）。 */
const sigOf = (v: AgentView): string => unknownSignature(v.status.raw.label, v.agent.latestTurn, v.currentActivity ? v.currentActivity.evidence.observedAt : null);

function Berth({ a, depth, orphan, mini, confirmed, onConfirmFail, rootName, childNote, onDismiss, outcome, outcomeAt, readOnly }: {
  /** 監視窓（見るだけ）。詳細を広げても確認済み・隠すのボタンは出さない。 */ readOnly?: boolean;
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
        <ModelBadge model={a.agent.model} effort={a.agent.effort} spawn={a.agent.spawnSpec} root={a.agent.parent.kind === "root"} />
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
      <div className="c3" title={act}>{shortAct(act)}</div>
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
          {onDismiss && !mini && !readOnly ? (
            <button className="btn sm" aria-label="確認済みにして隠す" title="この画面の表示だけを隠します。Codexやホストの状態・判定は変わりません。新しい活動が届くと再表示します" onClick={onDismiss}><Icon name="check" />隠す</button>
          ) : null}
          {failUnconfirmed && !mini && !readOnly ? (
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

/** カード内の子ツリー表示（compact）の追加指定。メイン行は出さず、メイン直下の子から描く。行を押すと詳細（非 mini の Berth）を広げる（表示だけ）。 */
interface CompactOpts {
  /** 根（メイン直下の子）の並び（キー）。入力中は凍結された並びが渡る。 */
  order: string[];
  /** 根の上限件数。undefined なら全件。 */
  topLimit?: number;
  moreAll: boolean;
  onMore: (all: boolean) => void;
  detail: Set<string>;
  onToggleDetail: (key: string) => void;
  /** 子に保留中の承認・質問があるときの表示語（無ければ null）。押すと同じカード内の該当要求へスクロールする（回答はしない）。 */
  pendingLabel: (agentKey: string) => string | null;
  onJumpReq: (agentKey: string) => void;
}

/** 「子 N件（…）▸」の開閉ボタン。 */
function KidsToggle({ text, open, onToggle }: { text: string; open: boolean; onToggle: () => void }) {
  return <button className="btn-line small kids-toggle" aria-expanded={open} title={`${text}。押すと子の一覧を${open ? "閉じ" : "開き"}ます（表示だけ）`} onClick={onToggle}>{open ? "▼" : "▶"} {text}</button>;
}

function RootBlock({ snap, c, views, showHead, selected, mini, parentOnly, confirmed, dismissed, onDismiss, onOpen, onConfirmFail, onRecheck, compact, kidsToggle }: {
  snap: HostSnapshot; c: Chat; views: AgentView[]; showHead: boolean; selected: boolean; mini: boolean; parentOnly: boolean;
  confirmed: Set<string>; dismissed: Map<string, string>; onDismiss: (items: AgentView[]) => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void;
  onRecheck?: (chat: ChatKey, agents: AgentKey[]) => Promise<void>;
  compact?: CompactOpts;
  /** 「全」の見出し下に置く、チャット単位の子の開閉。 */
  kidsToggle?: { text: string; open: boolean; onToggle: () => void };
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
  // カード内では行を押すと詳細（非 mini）を広げる。詳細でも監視窓では確認済み・隠すのボタンは出さない（readOnly）。
  const rowMini = (v: AgentView) => (compact ? !compact.detail.has(keyStr(v.agent.key)) : mini);
  const rowWrap = (v: AgentView, el: React.ReactNode) => {
    if (!compact) return el;
    const k = keyStr(v.agent.key);
    const on = compact.detail.has(k);
    const pl = compact.pendingLabel(k);
    return (
      <div className={`kid-row${on ? " on" : ""}`} role="button" tabIndex={0} aria-expanded={on} title="押すとこの子の詳細を広げます・戻します（表示だけ。送信先・回答は変わりません）"
        onClick={(e) => { if (!(e.target as HTMLElement).closest("button")) compact.onToggleDetail(k); }}
        onKeyDown={(e) => { if ((e.key === "Enter" || e.key === " ") && e.target === e.currentTarget) { e.preventDefault(); compact.onToggleDetail(k); } }}>
        {pl ? <button className="btn-line small kid-req" title="このカードの該当する要求へ移動します（回答はしません）" onClick={() => compact.onJumpReq(k)}>{pl} → 要求へ</button> : null}
        {el}
      </div>
    );
  };
  // カード内ツリーの根: メイン直下の子（親が一覧に無い子を含む）。並びは凍結済みの order、上限があれば先頭から。
  const orderIdx = new Map((compact?.order ?? []).map((k, i) => [k, i] as const));
  const topAll = compact
    ? roots.flatMap((r) => (r.agent.parent.kind === "root" ? kids(keyStr(r.agent.key)) : [r]))
        .map((v, i) => ({ v, i })).sort((a, b) => (orderIdx.get(keyStr(a.v.agent.key)) ?? 1e9) - (orderIdx.get(keyStr(b.v.agent.key)) ?? 1e9) || a.i - b.i).map((x) => x.v)
    : roots;
  const top = compact?.topLimit !== undefined ? topAll.slice(0, compact.topLimit) : topAll;
  const rest = topAll.length - top.length;
  const node = (v: AgentView) => {
    const k = keyStr(v.agent.key);
    const ch = parentOnly ? [] : kids(k);
    return (
      <li key={k}>
        {rowWrap(v, <Berth a={v} outcome={outcomeOf(v)} outcomeAt={atOf(v)} depth={depthOf(v)} orphan={false} mini={rowMini(v)} readOnly={mini} confirmed={confirmed.has(k)} onConfirmFail={onConfirmFail} childNote={parentOnly && v.agent.parent.kind === "root" ? parentNote(snap, c) : undefined} rootName={v.agent.parent.kind === "root" ? (v.agent.displayName.kind === "value" ? v.agent.displayName.value : previewTitle(c) ?? "メイン") : undefined} />)}
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
      {kidsToggle ? <KidsToggle {...kidsToggle} /> : null}
      <ul className="tree">{top.map(node)}</ul>
      {compact ? (rest > 0
        ? <button className="btn-line small" onClick={() => compact.onMore(true)}>ほか{rest}件を表示</button>
        : compact.moreAll && topAll.length > DESCENDANT_LIMIT ? <button className="btn-line small" onClick={() => compact.onMore(false)}>直近{DESCENDANT_LIMIT}件だけにする</button> : null) : null}
      {histAll.length ? (
        <div style={{ margin: "6px 0 0" }}>
          <div className="grp-h">
            <button className={`btn-line small ${histB.attention ? "attn" : ""}`} aria-expanded={openHistory} title="履歴の読取りだけで終端を確認した子です(live確認ではありません)" onClick={() => setOpenHistory(!openHistory)}>{openHistory ? "▼" : "▶"} 履歴のみで確認した子 {histShown.length}件（{historyBreakdownText(histB)}）{histHidden ? `／確認済み${histHidden}件は非表示` : ""}</button>
            {openHistory && histShown.length && !mini ? <button className="btn-line small" title="この画面の表示だけを隠します。ホストの状態・判定は変わりません" onClick={() => onDismiss(histShown)}>まとめて確認済みにして隠す</button> : null}
          </div>
          {openHistory ? (
            <ul className="tree">{histShown.map((v) => (
              <li key={keyStr(v.agent.key)}>
                {rowWrap(v, <Berth a={v} outcome={outcomeOf(v)} outcomeAt={atOf(v)} depth={depthOf(v)} orphan={v.agent.parent.kind === "unknown"} mini={rowMini(v)} readOnly={mini} confirmed={false} onConfirmFail={onConfirmFail} onDismiss={() => onDismiss([v])} />)}
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
                {rowWrap(v, <Berth a={v} depth={depthOf(v)} orphan={v.agent.parent.kind === "unknown"} mini={rowMini(v)} readOnly={mini} confirmed={false} onConfirmFail={onConfirmFail} onDismiss={() => onDismiss([v])} />)}
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

/** ドックのカードから呼ぶ操作（本窓のみ。監視窓には渡さない）。回答は中央と同じ経路で、押した1件だけに作用する。 */
export interface DockActions {
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

/** タスクカード（1チャット）。状態は書き換えず、親の状態・活動を既存の文言のまま見せる。 */
function TaskCard({ snap, c, band, line, open, marked, mini, at, confirmed, actions, kidsOpen, onToggleKids, kidsOrder, moreAll, onMore, detail, onToggleDetail, dismissed, onDismiss, onRecheck, onToggle, onClose, onOpen, onConfirmFail }: {
  snap: HostSnapshot; c: Chat; band: boolean; line: boolean; open: boolean; marked: boolean; mini: boolean; at: number | undefined; confirmed: Set<string>;
  actions?: DockActions; onToggle: () => void; onClose?: () => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void;
  /** 子ツリーの開閉・「ほかN件」・詳細の開いた行は DockPane が持つ（帯とタスク列を移ってカードが作り直されても消えない）。 */
  kidsOpen: boolean; onToggleKids: () => void; kidsOrder: string[]; moreAll: boolean; onMore: (all: boolean) => void;
  detail: Set<string>; onToggleDetail: (key: string) => void;
  dismissed: Map<string, string>; onDismiss: (items: AgentView[]) => void; onRecheck?: (chat: ChatKey, agents: AgentKey[]) => Promise<void>;
}) {
  const root = rootView(snap, c);
  const working = isWorkingForDock(snap, c);
  const reqs = chatRequests(snap, c);
  // 組織実行のチャットだけ、回答を届けられなかった要求の理由を1行で残す（接続の切替などで答えられなくなったもの）。
  const gaps = snap.chatLocals.find((l) => l.chat.id === c.key.id)?.org ? chatGapRequests(snap, c.key) : [];
  const body = !line || open;
  const sum = descendantSummary(snap, c, (v) => historyOutcomeOf(snap, v));
  const sumText = descendantSummaryText(sum);
  const reqsRef = useRef<HTMLDivElement>(null);
  const reqLabel = (k: string): string | null => { const r = reqs.find((x) => keyStr(x.agent) === k); return r ? (r.kind.kind === "userInput" ? "質問待ち" : "承認待ち") : null; };
  const jumpReq = (k: string) => {
    const el = Array.from(reqsRef.current?.querySelectorAll<HTMLElement>("[data-req-agent]") ?? []).find((e) => e.dataset.reqAgent === k) ?? reqsRef.current;
    el?.scrollIntoView({ block: "nearest" });
  };
  const act = root?.currentActivity ? activityText(root.currentActivity.summary) : root ? "活動なし" : "未確認";
  const nameOf = (a: AgentKey): string => { const v = chatAgents(snap, c).find((x) => keyStr(x.agent.key) === keyStr(a)); return v ? showKnown(v.agent.displayName) : a.id; };
  const notLive = !!root && root.freshness !== "live";
  const chip = chatChipClass(snap, c);
  const stateText = chatStatusText(snap, c);
  return (
    <section className={`tcard ${chip} ${band ? "band" : ""} ${working ? "working" : "ended"} ${line ? "line" : ""} ${kidsOpen && body ? "kopen" : ""}`} data-chat={cid(c)} aria-label={`${chatName(c)}のカード`}>
      <div className="tc-h">
        {line ? (
          <button className="tc-line" aria-expanded={open} title={`${stateText}。押すとこのカードだけ展開します`} onClick={onToggle}>
            <span className={`st ${chip}`}><i aria-hidden="true" />{stateText}</span>
            <span className="tc-name">{chatName(c)}</span>
            <OrgBadge local={snap.chatLocals.find((l) => l.chat.id === c.key.id)} />
            {marked ? <span className="tc-mark" role="img" aria-label="新しい動きがあります">●</span> : null}
            {at ? <span className="tc-at">{hms(at)}</span> : null}
          </button>
        ) : (
          <>
            <span className="tc-name">{chatName(c)}</span>
            <OrgBadge local={snap.chatLocals.find((l) => l.chat.id === c.key.id)} />
            {marked ? <span className="tc-mark" role="img" aria-label="新しい動きがあります">●</span> : null}
            <span className={`st ${chip}`}><i aria-hidden="true" />{stateText}</span>
            {at ? <span className="tc-at">{hms(at)}</span> : null}
          </>
        )}
        <button className="small" onClick={() => onOpen(c.key.id)} title="中央でこのチャットを開きます">開く</button>
        {onClose ? <button className="small" aria-label={`${chatName(c)}のカードを閉じる`} title="この画面から外します（新しい活動があれば再表示します。会話・作業には影響しません）" onClick={onClose}><Icon name="x" /></button> : null}
      </div>
      {reqs.length ? (
        <div className="tc-reqs" ref={reqsRef}>
          {reqs.map((r) => (mini || !actions
            ? <div key={`${r.key.source}/${r.key.requestId}`} data-req-agent={keyStr(r.agent)} className="tc-req-mini"><Icon name="alert" />{r.kind.kind === "userInput" ? "質問" : "承認"}の回答待ち: {r.detail.summary}（通常画面で回答します）</div>
            : <div key={`${r.key.source}/${r.key.requestId}`} data-req-agent={keyStr(r.agent)}><RequestCard r={r} onRespond={(q, a) => actions.respond(q, a)} agentName={nameOf} /></div>))}
        </div>
      ) : null}
      {gaps.length ? <div className="tc-reqs">{gaps.map((r) => <RequestGapLine key={`gap:${r.key.source}/${r.key.requestId}`} r={r} guide={false} />)}</div> : null}
      {body ? (
        <>
          {notLive ? <div className="tc-fresh">{FRESH[root.freshness].t}</div> : null}
          <div className="tc-act" title={act}>{shortAct(act)}</div>
          <div className="tc-kids">
            {sum.notFetched ? <div className="tc-kids-none small muted">{sumText}</div> : <KidsToggle text={sumText} open={kidsOpen} onToggle={onToggleKids} />}
            {kidsOpen && !sum.notFetched ? (
              <div className="tc-kids-box">
                <RootBlock snap={snap} c={c} views={chatAgents(snap, c)} showHead={false} selected parentOnly={false} mini={mini} confirmed={confirmed} dismissed={dismissed}
                  onDismiss={onDismiss} onOpen={onOpen} onConfirmFail={onConfirmFail} onRecheck={mini ? undefined : onRecheck}
                  compact={{ order: kidsOrder, topLimit: working && !moreAll ? DESCENDANT_LIMIT : undefined, moreAll, onMore, detail, onToggleDetail, pendingLabel: reqLabel, onJumpReq: jumpReq }} />
              </div>
            ) : null}
          </div>
        </>
      ) : null}
    </section>
  );
}

export function DockPane({ snap: snapIn, scope, selId, mini, confirmed, onScope, onOpen, onConfirmFail, onAct, onRecheck, actions, focusReq }: {
  snap: HostSnapshot; scope: MonitorScope; selId: string | null; mini?: boolean; confirmed: Set<string>;
  /** 状態不明の子の「履歴で再確認」（本窓のみ。保存履歴を読むだけ）。結果の案内は呼び出し側が出す。 */
  onRecheck?: (chat: ChatKey, agents: AgentKey[]) => Promise<void>;
  /** カード内の指示欄・回答（本窓のみ。監視窓では渡さず、回答ボタンと指示欄を出さない）。 */
  actions?: DockActions; focusReq?: DockFocus | null;
  onScope: (s: MonitorScope) => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void; onAct: (a: string) => void;
}) {
  // 更新は1秒に間引く（活動のたびの再描画でチカチカしない）。承認・質問の増減は即時。
  const snap = useThrottled(snapIn, 1000, snapIn.requests.filter((r) => r.state.kind === "pending").map((r) => `${r.key.source}/${r.key.requestId}`).join("|"));
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
  // 子ツリーの開閉（タスク）・「ほかN件」・詳細を開いた行・「全」のチャット別開閉は、帯⇔タスク列でカードが作り直されても消えないよう DockPane が持つ（画面内の記憶。保存しない）。
  const [kidsOpen, setKidsOpen] = useState<Map<string, boolean>>(new Map());
  const [moreAll, setMoreAll] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<Set<string>>(new Set());
  // 「全」: 未指定=開（完了の子は全体の「終了済みも表示」に従う）、true=利用者が展開（このチャットだけ完了の子も出す）、false=畳む
  const [allOpen, setAllOpen] = useState<Map<string, boolean>>(new Map());
  const kidOrder = useRef<Map<string, string[]>>(new Map());
  const taskOrder = useRef<string[]>([]);
  const bandOrder = useRef<string[]>([]);
  const allOrder = useRef<string[]>([]);
  const allSig = useRef("");
  const taskSig = useRef("");
  const kidSig = useRef<Map<string, string>>(new Map());
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
  // 並び（チャット）は、作業状態（working か否か）が変わったときだけ入れ替える。それ以外は前回の並びを保つ（入力中も保つ）。
  const workSig = snap.chats.map((c) => `${cid(c)}:${isWorkingForDock(snap, c) ? 1 : 0}`).join("|");
  // 「タスク」の選定・所属の凍結だけが見る不明フラグ（「全」「選択中」の並びには効かせない）
  const unkSig = snap.chats.map((c) => `${cid(c)}:${isUnknownForDock(snap, c) ? 1 : 0}`).join("|");
  const holdAll = typing || workSig === allSig.current;
  const allIds = stableOrder(allOrder.current, orderWorkingFirst(snap.chats, rec, (c) => isWorkingForDock(snap, c)).map(cid), holdAll);
  const allById = new Map(snap.chats.map((c) => [cid(c), c] as const));
  const chats = isSel ? snap.chats.filter((c) => c.key.id === selId) : allIds.flatMap((id) => allById.get(id) ?? []);
  const visibleOf = (c: Chat) => chatAgents(snap, c).filter((v) => {
    const showFinished = scope.kind === "allChats" && scope.showFinished || (view === "all" && allOpen.get(cid(c)) === true);
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
  // 「全」の見出しの開閉用。子の数は内訳の文言（履歴確認・状態不明を含む）で出す。子孫が無いチャットには出さない。
  const allToggle = (c: Chat) => {
    if (view !== "all") return undefined;
    const sum = descendantSummary(snap, c, (v) => historyOutcomeOf(snap, v));
    if (sum.notFetched) return undefined;
    const open = allOpen.get(cid(c)) !== false;
    return { text: descendantSummaryText(sum), open, onToggle: () => setAllOpen((m) => new Map(m).set(cid(c), !open ? true : false)) };
  };
  // 「タスク」: 帯（対応待ち）＋最近動かした順の最大5件＋ほか（1行）。完了しても消さない（閉じる・アーカイブ・削除だけ）。
  const sel = useMemo(
    () => (isTask ? selectTasks(snap, rec, closed, (c) => isWorkingForDock(snap, c), undefined, (c) => isUnknownForDock(snap, c)) : { attention: [], tasks: [], overflow: [], hiddenUnknown: [] }),
    [isTask, snap, rec, closed],
  );
  // 入力中は、並びだけでなく所属（帯・タスク列）も保つ（カードを作り直さない＝フォーカス・変換を保つ）。
  // 作業状態・対応待ち・閉じたものが変わらない間は、所属と並びも前回のまま。
  const memberSig = `${workSig}#${unkSig}#${sel.attention.map(cid).join("|")}#${[...closed.keys()].join("|")}`;
  const { band: bandIds, task: taskIds } = stableMembership(bandOrder.current, taskOrder.current, sel.attention.map(cid), sel.tasks.map(cid), typing || memberSig === taskSig.current, new Set(allById.keys()));
  useEffect(() => { bandOrder.current = bandIds; taskOrder.current = taskIds; allOrder.current = allIds; allSig.current = workSig; taskSig.current = memberSig; });
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
  const kidOrderNow = new Map<string, string[]>();
  const kidSigNow = new Map<string, string>();
  useEffect(() => { kidOrder.current = kidOrderNow; kidSig.current = kidSigNow; });
  const toggleIn = <T,>(set: Set<T>, v: T): Set<T> => { const n = new Set(set); if (n.has(v)) n.delete(v); else n.add(v); return n; };
  const card = (c: Chat, band: boolean) => {
    const id = cid(c);
    // 入力中は子の並びも凍結（新しい子は末尾）
    // 子の並びは、子の作業状態が変わったときだけ入れ替える
    const ksig = chatAgents(snap, c).map((v) => `${keyStr(v.agent.key)}:${isActiveState(v.status.state) ? 1 : 0}`).join("|");
    kidSigNow.set(id, ksig);
    const order = stableOrder(kidOrder.current.get(id) ?? [], branchOrder(snap, c), typing || ksig === kidSig.current.get(id));
    kidOrderNow.set(id, order);
    const isOpen = kidsOpen.get(id) ?? isWorkingForDock(snap, c);
    return (
    <TaskCard key={cid(c)} snap={snap} c={c} band={band} line={line} open={expanded.has(cid(c))} marked={marks.has(cid(c))} mini={!!mini} at={rec.get(cid(c))?.at}
      confirmed={confirmed} actions={mini ? undefined : actions} onToggle={() => toggleOpen(cid(c))} onClose={band ? undefined : () => closeCard(c)} onOpen={onOpen} onConfirmFail={onConfirmFail}
      kidsOpen={isOpen} onToggleKids={() => setKidsOpen((m) => new Map(m).set(id, !isOpen))} kidsOrder={order} moreAll={moreAll.has(id)} onMore={(all) => setMoreAll((s) => (all ? new Set(s).add(id) : toggleIn(s, id)))}
      detail={detail} onToggleDetail={(k) => setDetail((s) => toggleIn(s, k))} dismissed={dismissed} onDismiss={mini ? () => {} : onDismiss} onRecheck={mini ? undefined : onRecheck} />
    );
  };
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
            {sel.hiddenUnknown.length ? <div className="empty-note">状態不明の{sel.hiddenUnknown.length}件は「選択中」「全」で見られます</div> : null}
          </>
        ) : blocks.length ? blocks.map(({ c, views }) => (
          <RootBlock key={keyStr(c.key)} snap={snap} c={c} views={views} showHead={!isSel || !!mini} selected={c.key.id === selId} mini={!!mini} parentOnly={parentOnly || (view === "all" && allOpen.get(cid(c)) === false)} kidsToggle={parentOnly ? undefined : allToggle(c)}
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

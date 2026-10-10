import { useState } from "react";
import type { AgentKey, AgentView, Chat, ChatKey, HistoryOutcome, HostSnapshot, MonitorScope } from "../ipc/types";
import { Icon } from "./Icon";
import { Popover } from "./Popover";
import type { Known } from "../ipc/types";
import { chatAgents, chatDisplay, descendantStates, historyOutcomeOf, chatName, isDoneLike, isWorkingForDock, previewTitle } from "./derive";
import { CHAT_DISPLAY_TEXT, HISTORY_OUTCOME_TEXT, historyBreakdown, historyBreakdownText, parkGroup, rawStateText, sortByDockRank, unknownCount, unknownNote, unknownSignature } from "./chatState";
import { FRESH, SOURCE_LABEL, STATE, hms, keyStr, showKnown } from "./format";
import { TitleBar } from "./Chrome";

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

export function DockPane({ snap, scope, selId, mini, confirmed, onScope, onOpen, onConfirmFail, onAct, onRecheck }: {
  snap: HostSnapshot; scope: MonitorScope; selId: string | null; mini?: boolean; confirmed: Set<string>;
  /** 状態不明の子の「履歴で再確認」（本窓のみ。保存履歴を読むだけ）。結果の案内は呼び出し側が出す。 */
  onRecheck?: (chat: ChatKey, agents: AgentKey[]) => Promise<void>;
  onScope: (s: MonitorScope) => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void; onAct: (a: string) => void;
}) {
  // 「実行中」表示と「親のみ」はウィンドウ内の記憶（ホストへは保存しない）。表示範囲の変更だけで送信先・中断・承認を変えない。
  const [runningView, setRunningView] = useState(false);
  const [parentOnly, setParentOnly] = useState(false);
  // 状態不明の子の「確認済みにして隠す」: 画面内の記憶（ホスト・永続化なし）。署名が変われば再表示。
  const [dismissed, setDismissed] = useState<Map<string, string>>(new Map());
  const onDismiss = (items: AgentView[]) => setDismissed((m) => { const n = new Map(m); items.forEach((v) => n.set(keyStr(v.agent.key), sigOf(v))); return n; });
  const isSel = !runningView && scope.kind === "selectedChat";
  const showFinished = scope.kind === "allChats" && scope.showFinished;
  // 「終了済みも表示」は、選択中・実行中表示では選べない（今の条件のまま）。オンの項目はボタンの文字に出して、隠れた状態を見せる。
  const showFinishedOpt = !runningView && !isSel;
  const onItems = [parentOnly ? "親のみ" : null, showFinishedOpt && showFinished ? "終了済み" : null].filter((s): s is string => s !== null);
  const viewLabel = onItems.length ? `表示: ${onItems.join("・")}` : "表示 ▾";
  const viewTitle = onItems.length ? `表示の設定（${onItems.join("・")}がオン）` : "表示の設定（親のみ表示・終了済みも表示）";
  const chats = isSel ? snap.chats.filter((c) => c.key.id === selId) : runningView ? snap.chats.filter((c) => isWorkingForDock(snap, c)) : snap.chats;
  const visibleOf = (c: Chat) => chatAgents(snap, c).filter((v) => {
    if (parentOnly) {
      if (v.agent.parent.kind !== "root") return false;
      if (isSel || runningView || isWorkingForDock(snap, c)) return true; // 子孫が作業中の親は、親自身が完了でも出す
    } else if (isSel) return true; // 選択中: 完了済みも常に表示
    if (runningView && v.agent.parent.kind === "root") return true; // 実行中表示: 子孫が作業中なら親（完了済みでも）を文脈として出す
    const st = v.status.state;
    if (st === "failed") return confirmed.has(keyStr(v.agent.key)) ? showFinished : true;
    if (isDoneLike(st) || st === "idle") return showFinished && !runningView;
    return true;
  });
  const blocks = chats.map((c) => ({ c, views: visibleOf(c) })).filter((b) => isSel || b.views.length);
  return (
    <>
      <div className="dock-top">
        <span className="seg" role="group" aria-label="表示範囲">
          <button aria-pressed={isSel} aria-label="選択中のチャット" title="選択中のチャット（完了したエージェントも表示しています）" onClick={() => { setRunningView(false); onScope({ kind: "selectedChat", chat: selId ? { backend: "codex", id: selId } : null }); }}>選択中</button>
          <button aria-pressed={!isSel && !runningView} aria-label="全チャット" title="全チャット" onClick={() => { setRunningView(false); onScope({ kind: "allChats", showFinished }); }}>全</button>
          <button aria-pressed={runningView} aria-label="実行中" title="全チャットの作業中・対応待ち（子孫が作業中のものを含む、アーカイブ済みも）。作業中・対応待ちのチャットを全て表示しています" onClick={() => setRunningView(true)}>実行中</button>
        </span>
        <Popover trigger={viewLabel} title={viewTitle} triggerClassName="btn subtle sm dock-view">
          <label className="small"><input type="checkbox" checked={parentOnly} onChange={(e) => setParentOnly(e.target.checked)} />親のみ表示（子・孫を隠す）</label>
          {showFinishedOpt ? (
            <label className="small"><input type="checkbox" checked={showFinished} onChange={(e) => onScope({ kind: "allChats", showFinished: e.target.checked })} />終了済みも表示（完了・停止・確認済みの失敗）</label>
          ) : null}
        </Popover>
        {mini ? null : (
          <Popover trigger="?" label="説明" title="状態は Codex から取得した情報です。取得できない項目は「未確認」と表示します。" triggerClassName="help" textual align="right">
            状態は Codex から取得した情報です。取得できない項目は「未確認」と表示します。
          </Popover>
        )}
        {mini ? null : <button className="only-narrow-r small" onClick={() => onAct("toggleRight")}>閉じる</button>}
      </div>
      <div className="dock-scroll">
        {blocks.length ? blocks.map(({ c, views }) => (
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

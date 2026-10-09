import { useState } from "react";
import type { AgentView, Chat, HostSnapshot, MonitorScope } from "../ipc/types";
import { Flag, Icon } from "./Icon";
import type { Known } from "../ipc/types";
import { chatAgents, chatDisplay, descendantStates, chatName, isDoneLike, isWorkingForDock, previewTitle } from "./derive";
import { CHAT_DISPLAY_TEXT, sortByDockRank, unknownCount, unknownNote, unknownSignature } from "./chatState";
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

function Berth({ a, depth, orphan, mini, confirmed, onConfirmFail, rootName, childNote, onDismiss }: {
  rootName?: string; childNote?: string; a: AgentView; depth: number; orphan: boolean; mini: boolean; confirmed: boolean; onConfirmFail: (key: string) => void;
  onDismiss?: () => void;
}) {
  const m = STATE[a.status.state];
  const rel = orphan ? "親不明" : ["メイン", "子", "孫", "ひ孫"][depth] ?? `${depth}階層下`;
  const notLive = a.freshness !== "live";
  const act = waitText(a) ?? (a.currentActivity ? activityText(a.currentActivity.summary) : a.status.state === "unknown" ? `原状態: ${a.status.raw.label}（根拠なし）` : "活動なし");
  const failUnconfirmed = a.status.state === "failed" && !confirmed;
  return (
    <div className={`berth ${m.c} ${notLive ? "stale" : ""}`}>
      <div className="l1">
        <span className="nm" title={a.agent.parent.kind === "root" ? undefined : "Codexが付けた呼び名です（役割ではありません）"}>{a.agent.parent.kind === "root" ? (rootName ?? showKnown(a.agent.displayName)) : showKnown(a.agent.displayName)}</span><span className="rel">{rel}</span>
        {a.agent.parent.kind !== "root" && a.agent.agentPath.kind === "value" ? <span className="path mono" title="Codexが返したエージェントの経路（呼び名とは別）">{a.agent.agentPath.value}</span> : null}
        <span className={`st ${m.c}`}><Flag c={m.c} />{m.t}</span>
        {childNote ? <span className="small" title="親のみ表示のため、子孫を含めたチャットの状態を示します">{childNote}</span> : null}
      </div>
      {mini ? null : <div className="l2">{a.agent.parent.kind === "root" ? "役割: メイン（会話の本体）" : <>役割: {a.agent.role.kind === "missing" ? "未提供（Codexが返していません）" : showKnown(a.agent.role)}　担当: {showKnown(a.agent.assignment)}</>}</div>}
      <div className="l3" title={act}>{act}</div>
      {mini ? null : (
        <div className="l4">
          <span>{hms(a.status.evidence.observedAt)}</span>
          <span>{SOURCE_LABEL[a.status.evidence.source]}</span>
          {notLive ? (
            a.agent.parent.kind !== "root" && a.freshness === "historyOnly"
              ? <span style={{ color: "var(--stale)" }} title="子エージェントはライブ通知ではなく、走査（約3秒間隔の読み取り）で状態を更新しています">状態は走査で更新（約3秒間隔）</span>
              : <span style={{ color: "var(--stale)" }}>{FRESH[a.freshness].t}</span>
          ) : null}
        </div>
      )}
      {onDismiss && !mini ? (
        <div className="acts"><button className="btn-line" title="この画面の表示だけを隠します。Codexやホストの状態・判定は変わりません。新しい活動が届くと再表示します" onClick={onDismiss}><Icon name="check" />確認済みにして隠す</button></div>
      ) : null}
      {failUnconfirmed && !mini ? (
        <div className="acts"><button className="btn-line" onClick={() => onConfirmFail(keyStr(a.agent.key))}><Icon name="check" />失敗を確認済みにする</button></div>
      ) : null}
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

function RootBlock({ snap, c, views, showHead, selected, mini, parentOnly, confirmed, dismissed, onDismiss, onOpen, onConfirmFail }: {
  snap: HostSnapshot; c: Chat; views: AgentView[]; showHead: boolean; selected: boolean; mini: boolean; parentOnly: boolean;
  confirmed: Set<string>; dismissed: Map<string, string>; onDismiss: (items: AgentView[]) => void; onOpen: (id: string) => void; onConfirmFail: (key: string) => void;
}) {
  const [openUnknown, setOpenUnknown] = useState(false);
  const inList = new Set(views.map((v) => keyStr(v.agent.key)));
  const st = (v: AgentView) => v.status.state;
  const childrenOf = (k: string) => views.filter((v) => v.agent.parent.kind === "explicit" && keyStr(v.agent.parent.parent) === k);
  const subtreeAllUnknown = (v: AgentView): boolean => st(v) === "unknown" && childrenOf(keyStr(v.agent.key)).every(subtreeAllUnknown);
  // 状態不明の子孫（自分以下も全て不明）は末尾の折りたたみへ。親（メイン）は対象外。確認済みにして隠したものは、新しい活動が届くまで出さない。
  const parked = (v: AgentView) => v.agent.parent.kind !== "root" && subtreeAllUnknown(v);
  const isDismissed = (v: AgentView) => dismissed.get(keyStr(v.agent.key)) === sigOf(v);
  const parkedAll = views.filter(parked);
  const parkedShown = parkedAll.filter((v) => !isDismissed(v));
  const parkedHidden = parkedAll.length - parkedShown.length;
  const kids = (k: string) => sortByDockRank(childrenOf(k).filter((v) => !parked(v)), st);
  const roots = sortByDockRank(views.filter((v) => (v.agent.parent.kind === "root" || (v.agent.parent.kind === "explicit" && !inList.has(keyStr(v.agent.parent.parent)))) && !parked(v)), st);
  const orphans = sortByDockRank(views.filter((v) => v.agent.parent.kind === "unknown" && !parked(v)), st);
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
        <Berth a={v} depth={depthOf(v)} orphan={false} mini={mini} confirmed={confirmed.has(k)} onConfirmFail={onConfirmFail} childNote={parentOnly && v.agent.parent.kind === "root" ? parentNote(snap, c) : undefined} rootName={v.agent.parent.kind === "root" ? (v.agent.displayName.kind === "value" ? v.agent.displayName.value : previewTitle(c) ?? "メイン") : undefined} />
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
      {parkedAll.length ? (
        <div style={{ margin: "6px 0 0" }}>
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <button className="btn-line small" aria-expanded={openUnknown} onClick={() => setOpenUnknown(!openUnknown)}>{openUnknown ? "▼" : "▶"} 状態不明の子 {parkedShown.length}件{parkedHidden ? `（確認済み${parkedHidden}件は非表示）` : ""}</button>
            {openUnknown && parkedShown.length && !mini ? <button className="btn-line small" title="この画面の表示だけを隠します。ホストの状態・判定は変わりません" onClick={() => onDismiss(parkedShown)}>まとめて確認済みにして隠す</button> : null}
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
              <Berth a={v} depth={0} orphan mini={mini} confirmed={false} onConfirmFail={onConfirmFail} />
            </li>
          ))}</ul>
        </>
      ) : null}
    </div>
  );
}

export function DockPane({ snap, scope, selId, mini, confirmed, onScope, onOpen, onConfirmFail, onAct }: {
  snap: HostSnapshot; scope: MonitorScope; selId: string | null; mini?: boolean; confirmed: Set<string>;
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
        {mini ? null : (
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <b style={{ flex: 1 }}>エージェントのドック</b>
            <button className="only-narrow-r small" onClick={() => onAct("toggleRight")}>閉じる</button>
          </div>
        )}
        <span className="seg" role="group" aria-label="表示範囲">
          <button aria-pressed={isSel} onClick={() => { setRunningView(false); onScope({ kind: "selectedChat", chat: selId ? { backend: "codex", id: selId } : null }); }}>選択中のチャット</button>
          <button aria-pressed={!isSel && !runningView} onClick={() => { setRunningView(false); onScope({ kind: "allChats", showFinished }); }}>全チャット</button>
          <button aria-pressed={runningView} title="全チャットの作業中・対応待ち（子孫が作業中のものを含む、アーカイブ済みも）" onClick={() => setRunningView(true)}>実行中</button>
        </span>
        <label className="small"><input type="checkbox" checked={parentOnly} onChange={(e) => setParentOnly(e.target.checked)} />親のみ表示（子・孫を隠す）</label>
        {runningView ? <span className="small muted">作業中・対応待ちのチャットを全て表示しています</span> : !isSel ? (
          <label className="small"><input type="checkbox" checked={showFinished} onChange={(e) => onScope({ kind: "allChats", showFinished: e.target.checked })} />終了済みも表示（完了・停止・確認済みの失敗）</label>
        ) : <span className="small muted">完了したエージェントも表示しています</span>}
      </div>
      <div className="dock-scroll">
        {blocks.length ? blocks.map(({ c, views }) => (
          <RootBlock key={keyStr(c.key)} snap={snap} c={c} views={views} showHead={!isSel || !!mini} selected={c.key.id === selId} mini={!!mini} parentOnly={parentOnly}
            confirmed={confirmed} dismissed={dismissed} onDismiss={onDismiss} onOpen={onOpen} onConfirmFail={onConfirmFail} />
        )) : <div className="empty-note">{snap.chats.length ? "作業中・対応待ちのエージェントはありません" : "チャットを始めると、ここにエージェントが表示されます"}</div>}
      </div>
      {mini ? null : <div className="dock-foot">状態は Codex から取得した情報です。取得できない項目は「未確認」と表示します。</div>}
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

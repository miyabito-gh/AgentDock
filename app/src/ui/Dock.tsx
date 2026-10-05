import type { AgentView, Chat, HostSnapshot, MonitorScope } from "../ipc/types";
import { Flag, Icon } from "./Icon";
import { chatAgents, chatName, isDoneLike } from "./derive";
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

function Berth({ a, depth, orphan, mini, confirmed, onConfirmFail }: {
  a: AgentView; depth: number; orphan: boolean; mini: boolean; confirmed: boolean; onConfirmFail: (key: string) => void;
}) {
  const m = STATE[a.status.state];
  const rel = orphan ? "親不明" : ["メイン", "子", "孫", "ひ孫"][depth] ?? `${depth}階層下`;
  const notLive = a.freshness !== "live";
  const act = waitText(a) ?? (a.currentActivity ? showKnown(a.currentActivity.summary) : a.status.state === "unknown" ? `原状態: ${a.status.raw.label}（根拠なし）` : "活動なし");
  const failUnconfirmed = a.status.state === "failed" && !confirmed;
  return (
    <div className={`berth ${m.c} ${notLive ? "stale" : ""}`}>
      <div className="l1">
        <span className="nm">{showKnown(a.agent.displayName)}</span><span className="rel">{rel}</span>
        <span className={`st ${m.c}`}><Flag c={m.c} />{m.t}</span>
      </div>
      {mini ? null : <div className="l2">{a.agent.parent.kind === "root" ? "役割: メイン（会話の本体）" : <>役割: {showKnown(a.agent.role)}　担当: {showKnown(a.agent.assignment)}</>}</div>}
      <div className="l3" title={act}>{act}</div>
      {mini ? null : (
        <div className="l4">
          <span>{hms(a.status.evidence.observedAt)}</span>
          <span>{SOURCE_LABEL[a.status.evidence.source]}</span>
          {notLive ? <span style={{ color: "var(--stale)" }}>{FRESH[a.freshness].t}</span> : null}
        </div>
      )}
      {failUnconfirmed && !mini ? (
        <div className="acts"><button className="btn-line" onClick={() => onConfirmFail(keyStr(a.agent.key))}><Icon name="check" />失敗を確認済みにする</button></div>
      ) : null}
    </div>
  );
}

function RootBlock({ snap, c, views, showHead, selected, mini, confirmed, onOpen, onConfirmFail }: {
  snap: HostSnapshot; c: Chat; views: AgentView[]; showHead: boolean; selected: boolean; mini: boolean;
  confirmed: Set<string>; onOpen: (id: string) => void; onConfirmFail: (key: string) => void;
}) {
  const inList = new Set(views.map((v) => keyStr(v.agent.key)));
  const kids = (k: string) => views.filter((v) => v.agent.parent.kind === "explicit" && keyStr(v.agent.parent.parent) === k);
  const roots = views.filter((v) => v.agent.parent.kind === "root" || (v.agent.parent.kind === "explicit" && !inList.has(keyStr(v.agent.parent.parent))));
  const orphans = views.filter((v) => v.agent.parent.kind === "unknown");
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
    const ch = kids(k);
    return (
      <li key={k}>
        <Berth a={v} depth={depthOf(v)} orphan={false} mini={mini} confirmed={confirmed.has(k)} onConfirmFail={onConfirmFail} />
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
  const isSel = scope.kind === "selectedChat";
  const showFinished = scope.kind === "allChats" && scope.showFinished;
  const chats = isSel ? snap.chats.filter((c) => c.key.id === selId) : snap.chats;
  const visibleOf = (c: Chat) => chatAgents(snap, c).filter((v) => {
    if (isSel) return true; // 選択中: 完了済みも常に表示
    const st = v.status.state;
    if (st === "failed") return confirmed.has(keyStr(v.agent.key)) ? showFinished : true;
    if (isDoneLike(st) || st === "idle") return showFinished;
    return true;
  });
  const blocks = chats.map((c) => ({ c, views: visibleOf(c) })).filter((b) => isSel || b.views.length);
  return (
    <>
      <div className="dock-top">
        {mini ? null : (
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <b style={{ flex: 1 }}>エージェントのドック</b>
            <button className="only-narrow small" onClick={() => onAct("toggleRight")}>閉じる</button>
          </div>
        )}
        <span className="seg" role="group" aria-label="表示範囲">
          <button aria-pressed={isSel} onClick={() => onScope({ kind: "selectedChat", chat: selId ? { backend: "codex", id: selId } : null })}>選択中のチャット</button>
          <button aria-pressed={!isSel} onClick={() => onScope({ kind: "allChats", showFinished })}>全チャット</button>
        </span>
        {!isSel ? (
          <label className="small"><input type="checkbox" checked={showFinished} onChange={(e) => onScope({ kind: "allChats", showFinished: e.target.checked })} />終了済みも表示（完了・停止・確認済みの失敗）</label>
        ) : <span className="small muted">完了したエージェントも表示しています</span>}
      </div>
      <div className="dock-scroll">
        {blocks.length ? blocks.map(({ c, views }) => (
          <RootBlock key={keyStr(c.key)} snap={snap} c={c} views={views} showHead={!isSel || !!mini} selected={c.key.id === selId} mini={!!mini}
            confirmed={confirmed} onOpen={onOpen} onConfirmFail={onConfirmFail} />
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

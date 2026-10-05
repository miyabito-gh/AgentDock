import { useCallback, useEffect, useRef, useState } from "react";
import type { HostSnapshot, ModelInfo, MonitorScope, PendingRequest, RequestAnswer } from "./ipc/types";
import { getBundle, listModels, respondRequest, sendMessage, setMonitorScope } from "./ipc/client";
import { SCENARIOS, makeBundle, type Bundle } from "./mock/data";
import { GlobalBanner, MenuBar, TitleBar } from "./ui/Chrome";
import { LeftPane } from "./ui/LeftPane";
import { CenterPane, EmptyCenter } from "./ui/CenterPane";
import { DockPane, MiniWindow } from "./ui/Dock";
import { Dialogs, type DialogState } from "./ui/Dialogs";
import { keyStr } from "./ui/format";

export default function App() {
  const [scenario, setScenario] = useState("normal");
  const [bundle, setBundle] = useState<Bundle>(() => makeBundle("normal"));
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selId, setSelId] = useState<string | null>("a");
  const [scope, setScope] = useState<MonitorScope>(bundle.snapshot.monitorScope);
  const [leftOpen, setLeftOpen] = useState(true);
  const [rightOpen, setRightOpen] = useState(true);
  const [lNarrow, setLNarrow] = useState(false);
  const [rNarrow, setRNarrow] = useState(false);
  const [mini, setMini] = useState(false);
  const [mainTop, setMainTop] = useState(false);
  const [miniTop, setMiniTop] = useState(false);
  const [menu, setMenu] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<Set<string>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [enterMode, setEnterMode] = useState<"ctrl" | "enter">("ctrl");
  const [mockOpen, setMockOpen] = useState(false);
  const toastTimer = useRef<number | undefined>(undefined);

  useEffect(() => { void listModels().then(setModels); }, []);

  const narrow = () => window.matchMedia("(max-width:900px)").matches;
  const say = useCallback((t: string) => {
    setToast(t);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2600);
  }, []);

  const snap: HostSnapshot = bundle.snapshot;
  const chat = snap.chats.find((c) => c.key.id === selId) ?? null;

  const changeScenario = async (name: string) => {
    const b = await getBundle(name);
    setScenario(name); setBundle(b);
    setSelId(b.snapshot.chats[0]?.key.id ?? null);
    setScope(b.snapshot.monitorScope); setConfirmed(new Set()); setMockOpen(false);
  };

  const updateSnap = (f: (s: HostSnapshot) => HostSnapshot) => setBundle((b) => ({ ...b, snapshot: f(b.snapshot) }));

  const onScope = (s: MonitorScope) => { setScope(s); void setMonitorScope(s); };
  const onRespond = (r: PendingRequest, a: RequestAnswer) => {
    void respondRequest(r.key, a);
    // 仮データ上の見え方だけ。実際の確定はホストの requestUpdated で行う（T5）。
    updateSnap((s) => ({ ...s, requests: s.requests.map((x) => x.key.requestId === r.key.requestId ? { ...x, state: { kind: "answered", optionId: a.kind === "decision" ? a.optionId : null, at: Date.now() } } : x) }));
    say("回答を送りました（モック）。Codex の受理はホストの通知で確認します。");
  };

  const act = (a: string) => {
    if (a.startsWith("unv:")) { setDialog({ type: "unv", why: a.slice(4) }); return; }
    if (a.startsWith("settings:")) { setDialog({ type: "settings", tab: a.split(":")[1] }); return; }
    switch (a) {
      case "noop": break;
      case "closeToTray": say("トレイに格納しました。作業と監視は続いています。"); break;
      case "newChat": setDialog({ type: "newChat" }); break;
      case "quit": setDialog({ type: "quit" }); break;
      case "force": setDialog({ type: "force" }); break;
      case "attach": setDialog({ type: "attach" }); break;
      case "toggleLeft": if (narrow()) setLNarrow((v) => !v); else setLeftOpen((v) => !v); break;
      case "toggleRight": if (narrow()) setRNarrow((v) => !v); else setRightOpen((v) => !v); break;
      case "toggleMini": setMini((v) => !v); break;
      case "closeMini": setMini(false); break;
      case "miniTop": setMiniTop((v) => !v); break;
      case "toggleDone": onScope({ kind: "allChats", showFinished: !(scope.kind === "allChats" && scope.showFinished) }); break;
      case "createChat": case "quitStop": case "doForce": setDialog(null); say("この操作はT5で配線します（モック）。"); break;
      default: say("この操作はT5以降で配線します（モック）。");
    }
  };

  const checked = {
    toggleLeft: narrow() ? lNarrow : leftOpen, toggleRight: narrow() ? rNarrow : rightOpen, toggleMini: mini,
    toggleDone: scope.kind === "allChats" && scope.showFinished,
  };

  const dockProps = {
    snap, scope, selId, confirmed, onScope, onAct: act,
    onOpen: (id: string) => { setSelId(id); },
    onConfirmFail: (k: string) => setConfirmed((s) => new Set(s).add(k)),
  };

  return (
    <>
      <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
        <defs><pattern id="hatch" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="2" height="4" fill="#8A979C" /></pattern></defs>
      </svg>
      <div className="window" role="application" aria-label="AgentDock" onClick={() => { if (menu) setMenu(null); }}>
        <TitleBar title="AgentDock" top={mainTop} onAct={act} />
        <MenuBar open={menu} setOpen={setMenu} checked={checked} onAct={act} />
        <GlobalBanner source={snap.sources[0]} onAct={act} />
        <div className={`body ${leftOpen ? "" : "l-off"} ${rightOpen ? "" : "r-off"} ${lNarrow ? "n-l" : ""} ${rNarrow ? "n-r" : ""}`}>
          <aside className="left" aria-label="チャット一覧">
            <LeftPane snap={snap} sel={chat ? keyStr(chat.key) : null} onSelect={(id) => { setSelId(id); setLNarrow(false); }} onAct={act} />
          </aside>
          <main className="center">
            {chat ? (
              <CenterPane
                snap={snap} chat={chat} turns={bundle.turns[chat.key.id] ?? []}
                attachments={bundle.attachments.filter((f) => keyStr(f.ownerChat) === keyStr(chat.key))}
                settings={bundle.modelSettings[chat.key.id]} models={models} save={bundle.save}
                externalLabel={bundle.externalLabel[chat.key.id]} enterMode={enterMode}
                draft={drafts[chat.key.id] ?? chat.draft ?? ""} setDraft={(v) => setDrafts((d) => ({ ...d, [chat.key.id]: v }))}
                onAct={act} onRespond={onRespond}
                onSend={(text, steer) => { void sendMessage(chat.key, text, steer ? "steer" : "newTurn"); say("送信を依頼しました（モック）。受理はホストの通知で確認します。"); }}
              />
            ) : <EmptyCenter onAct={act} />}
          </main>
          <aside className="right" aria-label="エージェントのドック"><DockPane {...dockProps} /></aside>
        </div>
      </div>
      {mini ? <MiniWindow {...dockProps} top={miniTop} /> : null}
      {dialog ? (
        <Dialogs d={dialog} onClose={() => setDialog(null)} chats={snap.chats} source={snap.sources[0]} models={models}
          enterMode={enterMode} setEnterMode={setEnterMode}
          top={{ main: mainTop, mini: miniTop, setMain: setMainTop, setMini: setMiniTop }}
          setTab={(t) => setDialog({ type: "settings", tab: t })} onAct={act} />
      ) : null}
      {toast ? <div className="toast" role="status">{toast}</div> : null}
      <div className="mockctl">
        <button aria-expanded={mockOpen} onClick={() => setMockOpen((v) => !v)}>モックの状態</button>
        {mockOpen ? (
          <div className="panel" role="menu">
            <p>製品の画面ではありません。仮データの状態を切り替えて表示を確認します。</p>
            {SCENARIOS.map(([k, t]) => <button key={k} role="menuitemradio" aria-current={scenario === k} onClick={() => void changeScenario(k)}>{t}</button>)}
          </div>
        ) : null}
      </div>
    </>
  );
}

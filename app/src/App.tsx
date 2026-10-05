import { useCallback, useEffect, useRef, useState } from "react";
import type { HostEvent, HostEventEnvelope, HostSnapshot, ModelInfo, MonitorScope, PendingRequest, RequestAnswer, SendAttempt, SourceInfo } from "./ipc/types";
import * as host from "./ipc/client";
import * as mock from "./mock/client";
import { applyHostEvent, emptyBundle, replaceSnapshot, seqAction, UNKNOWN_CAPS } from "./ipc/live";
import { SCENARIOS, type Bundle } from "./mock/data";
import { GlobalBanner, MenuBar, TitleBar } from "./ui/Chrome";
import { LeftPane } from "./ui/LeftPane";
import { CenterPane, EmptyCenter, type SendNotice } from "./ui/CenterPane";
import { DockPane, MiniWindow } from "./ui/Dock";
import { Dialogs, type DialogState, type NewChatInput } from "./ui/Dialogs";
import { isRunning } from "./ui/derive";
import { keyStr } from "./ui/format";

type Mode = "live" | "mock";
const EXE_KEY = "agentdock.codexExe";
const DEFAULT_EXE = String.raw`C:\Users\wmasa\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe`;

const readExe = (): string => {
  try { return localStorage.getItem(EXE_KEY) ?? ""; } catch { return ""; }
};
const writeExe = (v: string) => {
  try { localStorage.setItem(EXE_KEY, v); } catch { /* 保存できなくても動作は続ける */ }
};

/** 接続コマンド自体が失敗したときの表示用（通信断・起動失敗であり、作業の失敗ではない）。 */
const failedSource = (message: string): SourceInfo => ({
  source: "-", backend: "codex", pid: { kind: "notFetched" }, startedAt: Date.now(),
  version: { kind: "unknown", message: "未確認" }, capabilities: UNKNOWN_CAPS,
  connection: { kind: "launchFailed", reason: "spawnFailed", message },
});

/** 送信の受理状態の案内。受理不明は失敗とも成功とも言わない。 */
function noticeOf(a: SendAttempt | undefined): SendNotice | null {
  if (!a) return null;
  switch (a.state.kind) {
    case "rejected":
      return { cls: "err", title: "送信は受け付けられませんでした", body: `Codex が受け付けなかったことを確認しています（${a.state.message}）。もう一度送ることができます。`, canRetry: true };
    case "acceptanceUnknown":
      return { cls: "warn", title: "送信が受理されたか確認できません", body: "応答がなく、Codex が受け付けたか分かりません。履歴と自動で照合しています。確認できるまで、再送と新しい送信は止めます。", canRetry: false };
    case "notFoundAfterReconcile":
      return { cls: "warn", title: "履歴に受理の痕跡がないことを確認しました", body: "この送信は受理されていません。必要なら、もう一度送ることができます。", canRetry: true };
    default:
      return null;
  }
}

export default function App() {
  const [mode, setMode] = useState<Mode>("live");
  const [scenario, setScenario] = useState("normal");
  const [bundle, setBundle] = useState<Bundle>(() => emptyBundle());
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selId, setSelId] = useState<string | null>(null);
  const [scope, setScope] = useState<MonitorScope>({ kind: "selectedChat", chat: null });
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
  const [connectError, setConnectError] = useState<string | null>(null);
  const [exePath, setExePath] = useState<string>(readExe);
  const [attempts, setAttempts] = useState<Record<string, SendAttempt>>({});
  const toastTimer = useRef<number | undefined>(undefined);
  const selRef = useRef<string | null>(null);
  selRef.current = selId;
  const live = mode === "live";

  const narrow = () => window.matchMedia("(max-width:900px)").matches;
  const say = useCallback((t: string) => {
    setToast(t);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  }, []);
  const sayErr = useCallback((prefix: string, e: unknown) => say(`${prefix}: ${host.asIpcError(e).message}`), [say]);

  /** 保存履歴から会話本文を読み込む（resumeしない）。 */
  const loadChat = useCallback(async (id: string) => {
    try {
      const h = await host.openChat({ backend: "codex", id });
      setBundle((b) => ({ ...b, turns: { ...b.turns, [id]: h.turns } }));
    } catch (e) { sayErr("履歴を取得できませんでした", e); }
  }, [sayErr]);

  const noteAttempt = useCallback((chatId: string, a: SendAttempt) => {
    setAttempts((m) => ({ ...m, [chatId]: a }));
  }, []);

  /** 接続し、一覧とモデルを取得する。起動失敗は接続エラーとして表示する（作業の失敗にしない）。 */
  const boot = useCallback(async (exe: string | null): Promise<boolean> => {
    try {
      const info = await host.connectBackend(exe);
      setConnectError(null);
      if (info.connection.kind !== "connected") return false;
    } catch (e) {
      // 二重起動の競合（開発時の再マウント等）で「接続済み」と拒否された場合は、接続済みとして続ける。
      const s = await host.getSnapshot().catch(() => null);
      if (!s?.sources.some((x) => x.connection.kind === "connected")) {
        setConnectError(host.asIpcError(e).message);
        return false;
      }
      setConnectError(null);
    }
    try {
      const page = await host.listChats();
      if (!selRef.current && page.items[0]) {
        const id = page.items[0].chat.key.id;
        setSelId(id);
        void loadChat(id);
      }
    } catch (e) { sayErr("会話の一覧を取得できませんでした", e); }
    try { setModels(await host.listModels()); } catch (e) { sayErr("モデル一覧を取得できませんでした", e); }
    return true;
  }, [loadChat, sayErr]);

  // 実接続: 購読 → スナップショット → 接続。seqが飛んだらスナップショットを取り直す。
  useEffect(() => {
    if (!live) return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    let ready = false;
    let last = 0;
    let resyncing = false;
    const pending: HostEventEnvelope[] = [];

    const resync = async () => {
      if (resyncing) return;
      resyncing = true;
      try {
        const snap = await host.getSnapshot();
        if (!alive) return;
        last = snap.seq;
        setBundle((b) => replaceSnapshot(b, snap));
      } catch (e) { sayErr("状態を取り直せませんでした", e); } finally { resyncing = false; }
    };
    const effects = (e: HostEvent) => {
      if (e.kind === "turnUpdated" && e.end !== null && e.turn.agent.id === selRef.current) void loadChat(e.turn.agent.id);
      else if (e.kind === "sendUpdated") noteAttempt(e.chat.id, e.attempt);
      else if (e.kind === "warning") say(`警告: ${e.message}`);
    };
    const handle = (env: HostEventEnvelope) => {
      const a = seqAction(last, env.seq);
      if (a === "ignore") return;
      if (a === "resync") { void resync(); return; }
      last = env.seq;
      setBundle((b) => applyHostEvent(b, env.event));
      effects(env.event);
    };

    setBundle(emptyBundle()); setSelId(null); setModels([]); setAttempts({}); setScope({ kind: "selectedChat", chat: null });
    void (async () => {
      try {
        const un = await host.subscribeHostEvents((env) => { if (ready) handle(env); else pending.push(env); });
        if (!alive) { un(); return; }
        unlisten = un;
        let snap = await host.getSnapshot();
        let ok = snap.sources.some((s) => s.connection.kind === "connected");
        if (!ok) {
          ok = await boot(readExe() || null);
          snap = await host.getSnapshot();
        } else {
          const page = await host.listChats().catch(() => null);
          if (page?.items[0] && !selRef.current) { setSelId(page.items[0].chat.key.id); void loadChat(page.items[0].chat.key.id); }
          setModels(await host.listModels().catch(() => []));
        }
        if (!alive) return;
        last = snap.seq;
        setBundle((b) => replaceSnapshot(b, snap));
        setScope(snap.monitorScope);
        ready = true;
        for (const env of pending.splice(0)) handle(env);
      } catch (e) {
        setConnectError(host.asIpcError(e).message);
        ready = true;
      }
    })();
    return () => { alive = false; unlisten?.(); };
  }, [live, boot, loadChat, noteAttempt, say, sayErr]);

  const snap: HostSnapshot = bundle.snapshot;
  const src: SourceInfo | undefined = snap.sources[0] ?? (live && connectError ? failedSource(connectError) : undefined);
  const chat = snap.chats.find((c) => c.key.id === selId) ?? null;

  const selectChat = (id: string | null) => {
    setSelId(id);
    if (live && id) void loadChat(id);
  };

  const changeScenario = async (name: string) => {
    const b = await mock.getBundle(name);
    setScenario(name); setBundle(b);
    setSelId(b.snapshot.chats[0]?.key.id ?? null);
    setScope(b.snapshot.monitorScope); setConfirmed(new Set()); setMockOpen(false);
  };

  const changeMode = async (m: Mode) => {
    setMockOpen(false);
    if (m === mode) return;
    setDialog(null);
    if (m === "mock") {
      const b = await mock.getBundle(scenario);
      setModels(await mock.listModels());
      setBundle(b); setSelId(b.snapshot.chats[0]?.key.id ?? null); setScope(b.snapshot.monitorScope); setConfirmed(new Set());
    }
    setMode(m);
  };

  const updateSnap = (f: (s: HostSnapshot) => HostSnapshot) => setBundle((b) => ({ ...b, snapshot: f(b.snapshot) }));

  const onScope = (s: MonitorScope) => { setScope(s); void (live ? host.setMonitorScope(s) : mock.setMonitorScope(s)); };

  const onRespond = async (r: PendingRequest, a: RequestAnswer) => {
    if (!live) {
      void mock.respondRequest(r.key, a);
      updateSnap((s) => ({ ...s, requests: s.requests.map((x) => x.key.requestId === r.key.requestId ? { ...x, state: { kind: "answered", optionId: a.kind === "decision" ? a.optionId : null, at: Date.now() } } : x) }));
      say("回答を送りました（モック）。");
      return;
    }
    // 画面の状態はホストの requestUpdated で更新する。ここでは結果の案内だけ。
    try {
      const out = await host.respondRequest(r.key, a);
      say(out.kind === "delivered" ? "回答を送りました。反映は Codex の通知で確認します。"
        : out.kind === "alreadyResolved" ? "すでに解決済みでした。回答は送っていません。"
        : `回答の受理を確認できません（${out.message}）。再送はしません。`);
    } catch (e) { sayErr("回答を送れませんでした", e); }
  };

  const onSend = async (text: string, steer: boolean): Promise<boolean> => {
    if (!chat) return false;
    if (!live) { void mock.sendMessage(chat.key, text, steer ? "steer" : "newTurn"); say("送信を依頼しました（モック）。"); return true; }
    const running = isRunning(snap, chat);
    if (running && !steer) { say("実行中は「追加指示」で送れます。完了後に送る依頼の登録は段階②で対応します。"); return false; }
    try {
      const a = await host.sendMessage(chat.key, text, running ? "steer" : "newTurn");
      noteAttempt(chat.key.id, a);
      return true;
    } catch (e) { sayErr("送信できませんでした", e); return false; }
  };

  const onRetrySend = async () => {
    if (!chat || !live) return;
    const a = attempts[chat.key.id];
    if (!a) return;
    try { noteAttempt(chat.key.id, await host.retrySend(chat.key, a.attemptId)); } catch (e) { sayErr("再送できませんでした", e); }
  };

  const onModel = (model: string, effort: string) => {
    if (!chat || !live) return;
    host.setChatModel(chat.key, { model, effort: effort || null }).catch((e) => sayErr("モデルの選択を保存できませんでした", e));
  };

  const interrupt = async () => {
    if (!chat) return;
    if (!live) { say("この操作はモックでは動きません。"); return; }
    try {
      const r = await host.interruptChat(chat.key);
      say(r.ack.kind === "requested" ? "中断を要求しました。停止はまだ確認できていません。"
        : r.ack.kind === "notRunning" ? "すでに実行中ではありません。終端は別途確認します。"
        : `中断要求の応答を確認できません（${r.ack.message}）。停止は確認できていません。`);
    } catch (e) { sayErr("中断を要求できませんでした", e); }
  };

  const createChat = async (i: NewChatInput) => {
    if (!live) { setDialog(null); say("この操作はモックでは動きません。"); return; }
    const m = models.find((x) => x.id === i.model);
    try {
      const r = await host.startChat(i.cwd, i.model ? { model: i.model, effort: m?.defaultEffort ?? null } : null, i.firstMessage);
      setDialog(null);
      setSelId(r.chat.key.id);
      if (r.firstSend) noteAttempt(r.chat.key.id, r.firstSend);
    } catch (e) { sayErr("チャットを作成できませんでした", e); }
  };

  const retryLaunch = (path: string) => {
    const p = path.trim();
    setExePath(p); writeExe(p);
    void boot(p || null);
  };

  const act = (a: string) => {
    if (a.startsWith("unv:")) { setDialog({ type: "unv", why: a.slice(4) }); return; }
    if (a.startsWith("settings:")) { setDialog({ type: "settings", tab: a.split(":")[1] }); return; }
    switch (a) {
      case "noop": break;
      case "closeToTray": say("トレイへの格納は後続の段階で対応します。"); break;
      case "newChat": setDialog({ type: "newChat" }); break;
      case "quit": setDialog({ type: "quit" }); break;
      case "force": setDialog({ type: "force" }); break;
      case "attach": setDialog({ type: "attach" }); break;
      case "interrupt": void interrupt(); break;
      case "retryLaunch": if (live) retryLaunch(exePath); break;
      case "toggleLeft": if (narrow()) setLNarrow((v) => !v); else setLeftOpen((v) => !v); break;
      case "toggleRight": if (narrow()) setRNarrow((v) => !v); else setRightOpen((v) => !v); break;
      case "toggleMini": setMini((v) => !v); break;
      case "closeMini": setMini(false); break;
      case "miniTop": setMiniTop((v) => !v); break;
      case "toggleDone": onScope({ kind: "allChats", showFinished: !(scope.kind === "allChats" && scope.showFinished) }); break;
      case "quitStop": case "doForce": setDialog(null); say("この操作は後続の段階で対応します。"); break;
      default: say("この操作は後続の段階で対応します。");
    }
  };

  const checked = {
    toggleLeft: narrow() ? lNarrow : leftOpen, toggleRight: narrow() ? rNarrow : rightOpen, toggleMini: mini,
    toggleDone: scope.kind === "allChats" && scope.showFinished,
  };

  const dockProps = {
    snap, scope, selId, confirmed, onScope, onAct: act,
    onOpen: (id: string) => { selectChat(id); },
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
        <GlobalBanner source={src} onAct={act} />
        <div className={`body ${leftOpen ? "" : "l-off"} ${rightOpen ? "" : "r-off"} ${lNarrow ? "n-l" : ""} ${rNarrow ? "n-r" : ""}`}>
          <aside className="left" aria-label="チャット一覧">
            <LeftPane snap={snap} sel={chat ? keyStr(chat.key) : null} onSelect={(id) => { selectChat(id); setLNarrow(false); }} onAct={act} />
          </aside>
          <main className="center">
            {chat ? (
              <CenterPane
                snap={snap} chat={chat} turns={bundle.turns[chat.key.id] ?? []}
                attachments={bundle.attachments.filter((f) => keyStr(f.ownerChat) === keyStr(chat.key))}
                settings={bundle.modelSettings[chat.key.id]} models={models} save={bundle.save}
                externalLabel={bundle.externalLabel[chat.key.id]} enterMode={enterMode}
                draft={drafts[chat.key.id] ?? chat.draft ?? ""} setDraft={(v) => setDrafts((d) => ({ ...d, [chat.key.id]: v }))}
                onAct={act} onRespond={(r, a) => void onRespond(r, a)}
                onSend={onSend} onModel={onModel}
                notice={live ? noticeOf(attempts[chat.key.id]) : null} onRetrySend={() => void onRetrySend()}
              />
            ) : <EmptyCenter onAct={act} />}
          </main>
          <aside className="right" aria-label="エージェントのドック"><DockPane {...dockProps} /></aside>
        </div>
      </div>
      {mini ? <MiniWindow {...dockProps} top={miniTop} /> : null}
      {dialog ? (
        <Dialogs d={dialog} onClose={() => setDialog(null)} chats={snap.chats} source={src} models={models}
          enterMode={enterMode} setEnterMode={setEnterMode}
          top={{ main: mainTop, mini: miniTop, setMain: setMainTop, setMini: setMiniTop }}
          exe={{ path: exePath, setPath: setExePath, placeholder: DEFAULT_EXE, connect: retryLaunch, live: live && snap.sources.every((s) => s.connection.kind !== "connected") }}
          onCreateChat={(i) => void createChat(i)}
          setTab={(t) => setDialog({ type: "settings", tab: t })} onAct={act} />
      ) : null}
      {toast ? <div className="toast" role="status">{toast}</div> : null}
      <div className="mockctl">
        <button aria-expanded={mockOpen} onClick={() => setMockOpen((v) => !v)}>{live ? "実接続" : "モック"}</button>
        {mockOpen ? (
          <div className="panel" role="menu">
            <p>接続先を切り替えます。モックは製品の画面ではなく、仮データの表示確認用です。</p>
            <button role="menuitemradio" aria-current={live} onClick={() => void changeMode("live")}>実接続（Codex App Server）</button>
            <button role="menuitemradio" aria-current={!live} onClick={() => void changeMode("mock")}>モック（仮データ）</button>
            {!live ? SCENARIOS.map(([k, t]) => <button key={k} role="menuitemradio" aria-current={scenario === k} onClick={() => void changeScenario(k)}>{t}</button>) : null}
          </div>
        ) : null}
      </div>
    </>
  );
}

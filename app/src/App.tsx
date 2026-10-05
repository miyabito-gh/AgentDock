import { useCallback, useEffect, useRef, useState } from "react";
import type { Chat, ForceKillPreview, HostEvent, HostEventEnvelope, HostSnapshot, ModelInfo, MonitorScope, PendingRequest, PermissionPreset, QuitDecision, QuitPhase, RequestAnswer, SendAttempt, SettingsImpact, SourceInfo } from "./ipc/types";
import * as host from "./ipc/client";
import { applyHostEvent, emptyBundle, replaceSnapshot, seqAction, UNKNOWN_CAPS } from "./ipc/live";
import type { Bundle } from "./mock/data";

// 仮データはmockモードに入るときだけ読み込む（live製品バンドルに含めない）。
const loadMock = async () => {
  const [client, data] = await Promise.all([import("./mock/client"), import("./mock/data")]);
  return { client, scenarios: data.SCENARIOS };
};
import { GlobalBanner, MenuBar, SaveBanner, TitleBar } from "./ui/Chrome";
import { LeftPane } from "./ui/LeftPane";
import { CenterPane, EmptyCenter, type CwdResult, type SendNotice } from "./ui/CenterPane";
import { DockPane, MiniWindow } from "./ui/Dock";
import { Dialogs, type DialogState, type NewChatInput } from "./ui/Dialogs";
import { isRunning, stopOpen } from "./ui/derive";
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
function noticeOf(a: SendAttempt | undefined, steer: { attemptId: string; queueInstead: () => void } | null = null): SendNotice | null {
  if (!a) return null;
  switch (a.state.kind) {
    case "rejected":
      if (steer && steer.attemptId === a.attemptId) {
        return { cls: "err", title: "追加指示は受け付けられませんでした", body: `対象のturnがすでに終わっている可能性があります（${a.state.message}）。自動では切り替えません。`, canRetry: false,
          alt: { label: "完了後に送信へ切り替える", run: steer.queueInstead } };
      }
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
  const [scenarios, setScenarios] = useState<Array<[string, string]>>([]);
  const [bundle, setBundle] = useState<Bundle>(() => emptyBundle());
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selId, setSelId] = useState<string | null>(null);
  const [scope, setScope] = useState<MonitorScope>({ kind: "selectedChat", chat: null });
  const [leftOpen, setLeftOpen] = useState(true);
  const [rightOpen, setRightOpen] = useState(true);
  const [lNarrow, setLNarrow] = useState(false);
  const [rNarrow, setRNarrow] = useState(false);
  const [mini, setMini] = useState(false);
  // 最前面。実接続ではホストの設定（再起動後も保持）、モックでは画面内だけ。
  const [mockMainTop, setMockMainTop] = useState(false);
  const [mockMiniTop, setMockMiniTop] = useState(false);
  const [quit, setQuit] = useState<QuitPhase>({ kind: "idle" });
  const [force, setForce] = useState<{ preview: ForceKillPreview | null; error: string | null; running: boolean }>({ preview: null, error: null, running: false });
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
  const [warnHidden, setWarnHidden] = useState(false);
  const [steerOf, setSteerOf] = useState<Record<string, { attemptId: string; text: string }>>({});
  const toastTimer = useRef<number | undefined>(undefined);
  const selRef = useRef<string | null>(null);
  const quitKindRef = useRef<QuitPhase["kind"]>("idle");
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
      else if (e.kind === "navigateToChat") { setSelId(e.chat.id); void loadChat(e.chat.id); } // 通知を開いた操作。表示だけで、回答・再実行はしない
      else if (e.kind === "quitUpdated") {
        // 終了手順の進行（確認・停止照合・保存）。段階が変わったときだけ確認画面を開く（「待つ」で閉じた後に開き直さない）。
        const prev = quitKindRef.current;
        quitKindRef.current = e.phase.kind;
        setQuit(e.phase);
        if (e.phase.kind === "idle") {
          setDialog((d) => (d?.type === "quit" ? null : d));
          if (prev !== "idle") say("終了を取り消しました。すでに送った中断要求は取り消せません。作業と監視は続いています。");
        } else if (prev !== e.phase.kind) setDialog((d) => (d?.type === "force" ? d : { type: "quit" }));
      }
      else if (e.kind === "systemResumed") {
        // sleepからの復帰。状態はホストが再照合する。表示中の履歴を取り直すだけで、再送・再開はしない。
        say("スリープから復帰しました。状態を再照合しています。");
        if (selRef.current) void loadChat(selRef.current);
      }
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
  const mainTop = live ? snap.settings.mainWindow.alwaysOnTop : mockMainTop;
  const miniTop = live ? snap.settings.monitorWindow.alwaysOnTop : mockMiniTop;
  /** 最前面の切替（窓ごと）。フォーカスは移さない。設定はホストが保存する。 */
  const setTop = (kind: "main" | "monitor", on: boolean) => {
    if (!live) { if (kind === "main") setMockMainTop(on); else setMockMiniTop(on); return; }
    host.setAlwaysOnTop(kind, on).catch((e) => sayErr("最前面を切り替えられませんでした", e));
  };
  const src: SourceInfo | undefined = snap.sources[0] ?? (live && connectError ? failedSource(connectError) : undefined);
  const chat = snap.chats.find((c) => c.key.id === selId) ?? null;
  // 選択中のチャットをホストへ伝える（通知の抑制判定だけに使う）。
  const selectedKey = chat?.key ?? null;
  useEffect(() => {
    if (live) void host.setSelectedChat(selectedKey).catch(() => { /* 抑制判定が古いままになるだけ（通知は出る側に倒れる） */ });
  }, [live, selectedKey?.id]); // eslint-disable-line react-hooks/exhaustive-deps
  // 監視窓（別窓）へ選択中のチャットを伝える（画面の表示状態だけ）。監視窓の起動時の問い合わせにも答える。
  useEffect(() => {
    if (live) void host.publishSelectedChat(selectedKey?.id ?? null).catch(() => undefined);
  }, [live, selectedKey?.id]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!live) return;
    let un: (() => void) | null = null;
    let alive = true;
    void host.onSelectedChatRequest(() => { void host.publishSelectedChat(selRef.current).catch(() => undefined); }).then((u) => { if (alive) un = u; else u(); });
    return () => { alive = false; un?.(); };
  }, [live]);
  /** 通知設定の変更（ホストが保存する。モックでは画面内だけ）。 */
  const onNotifySettings = (n: HostSnapshot["settings"]["notifications"]) => {
    const next = { ...snap.settings, notifications: n };
    if (!live) { setBundle((b) => ({ ...b, snapshot: { ...b.snapshot, settings: next } })); return; }
    host.setAppSettings(next).catch((e) => sayErr("通知設定を保存できませんでした", e));
  };
  /** 失敗を「確認済み」にする（印を外すだけ。再実行・成功化はしない）。 */
  const onAcknowledge = (c: Chat["key"], agent: Parameters<typeof host.acknowledgeFailure>[1]) => {
    if (!live) return;
    host.acknowledgeFailure(c, agent).catch((e) => sayErr("確認済みにできませんでした", e));
  };
  // 保存に失敗している単位。選択中チャットのものはチャット内の帯、それ以外は画面上部の帯に出す。
  const failedSaves = snap.saveStatus.filter((s) => s.state.kind === "saveFailed");
  const forChat = (s: (typeof failedSaves)[number]) => !!chat && "chat" in s.scope && s.scope.chat.id === chat.key.id;
  const chatSave = failedSaves.find(forChat)?.state ?? bundle.save;
  const localDraft = chat ? snap.chatLocals.find((l) => l.chat.id === chat.key.id)?.draft.text : undefined;
  const v = src?.version;
  // 現在の接続先を示す。実接続では接続中のCodexの版を出す（版が取れなければ、そう表示する）。
  const modeTag = !live ? "モック（仮データ）"
    : !src ? "Codex 未接続"
    : v?.kind === "match" ? `Codex ${v.version}`
    : v?.kind === "mismatch" ? `Codex ${v.actual}（対象版外）`
    : "Codex 版を確認できません";

  const selectChat = (id: string | null) => {
    setSelId(id);
    if (live && id) void loadChat(id);
  };

  const changeScenario = async (name: string) => {
    const { client } = await loadMock();
    const b = await client.getBundle(name);
    setScenario(name); setBundle(b);
    setSelId(b.snapshot.chats[0]?.key.id ?? null);
    setScope(b.snapshot.monitorScope); setConfirmed(new Set()); setMockOpen(false);
  };

  const changeMode = async (m: Mode) => {
    setMockOpen(false);
    if (m === mode) return;
    setDialog(null);
    if (m === "mock") {
      const { client, scenarios: list } = await loadMock();
      setScenarios(list);
      const b = await client.getBundle(scenario);
      setModels(await client.listModels());
      setBundle(b); setSelId(b.snapshot.chats[0]?.key.id ?? null); setScope(b.snapshot.monitorScope); setConfirmed(new Set());
    }
    setMode(m);
  };

  /** 入力途中の文章を画面に保持し、ホストへ渡す（保存は500msまとめ、ホスト側）。復元しても送信しない。 */
  const onDraft = (c: Chat, v: string) => {
    setDrafts((d) => ({ ...d, [c.key.id]: v }));
    // 保存の成否は保存状態（saveStatusUpdated）で表示する。ここでのIPC失敗は通信断で、入力欄の内容は画面に残る。
    if (live) host.setDraft(c.key, v).catch(() => undefined);
  };

  /** 保存に失敗している単位をすべて書き直す（ユーザー操作）。成功を確認できた分だけ「保存しました」と言う。 */
  const retrySave = async () => {
    if (!live) { say("この操作はモックでは動きません。"); return; }
    const failed = snap.saveStatus.filter((s) => s.state.kind === "saveFailed");
    let still = 0;
    let last = "";
    for (const f of failed) {
      try {
        const st = await host.retrySave(f.scope);
        if (st.state.kind === "saveFailed") { still++; last = st.state.message; }
      } catch (e) { still++; last = host.asIpcError(e).message; }
    }
    say(still === 0 ? "保存しました。" : `まだ保存できません: ${last}`);
  };

  const updateSnap = (f: (s: HostSnapshot) => HostSnapshot) => setBundle((b) => ({ ...b, snapshot: f(b.snapshot) }));

  const onScope = (s: MonitorScope) => { setScope(s); void (live ? host.setMonitorScope(s) : loadMock().then((m) => m.client.setMonitorScope(s))); };

  const onRespond = async (r: PendingRequest, a: RequestAnswer) => {
    if (!live) {
      void loadMock().then((m) => m.client.respondRequest(r.key, a));
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
    if (!live) { void loadMock().then((m) => m.client.sendMessage(chat.key, text, steer ? "steer" : "newTurn")); say("送信を依頼しました（モック）。"); return true; }
    const running = isRunning(snap, chat);
    if (running && !steer) {
      // 完了後に送る依頼として登録する（送信はホストが条件を判断して1件ずつ行う）。
      try { await host.enqueue(chat.key, text); say("完了後に送る依頼として登録しました。"); return true; } catch (e) { sayErr("依頼を登録できませんでした", e); return false; }
    }
    try {
      const a = await host.sendMessage(chat.key, text, running ? "steer" : "newTurn");
      noteAttempt(chat.key.id, a);
      if (running) {
        setSteerOf((m) => ({ ...m, [chat.key.id]: { attemptId: a.attemptId, text } }));
        if (a.state.kind === "accepted") say("追加指示を Codex が受理しました。現在のturnへの反映は会話の表示で確認してください（受理と反映は別です）。");
      }
      return true;
    } catch (e) { sayErr("送信できませんでした", e); return false; }
  };

  const qact = {
    edit: async (id: string, text: string): Promise<boolean> => {
      if (!chat || !live) return false;
      try { await host.editQueueEntry(chat.key, id, text); return true; } catch (e) { sayErr("依頼を編集できませんでした", e); return false; }
    },
    cancel: (id: string) => { if (chat && live) host.cancelQueueEntry(chat.key, id).catch((e) => sayErr("依頼を取り消せませんでした", e)); },
    resume: () => {
      if (!chat || !live) return;
      host.resumeQueue(chat.key).then(() => say("キューを再開しました。条件がそろえば、登録順に1件ずつ送ります。")).catch((e) => sayErr("キューを再開できませんでした", e));
    },
    reconcile: (attempt: string) => {
      if (!chat || !live) return;
      host.reconcileSend(chat.key, attempt).then((a) => say(a.state.kind === "acceptanceUnknown" ? "まだ受理を確認できません。照合を続けます（再送はしません）。" : "履歴との照合で確定しました。")).catch((e) => sayErr("照合できませんでした", e));
    },
    retry: (attempt: string) => {
      if (!chat || !live) return;
      host.retrySend(chat.key, attempt).then((a) => noteAttempt(chat.key.id, a)).catch((e) => sayErr("再送できませんでした", e));
    },
  };

  const impactText = (what: string, r: SettingsImpact) =>
    r.affectedEntries.length ? `${what}を変更しました。送信待ちの依頼 ${r.affectedEntries.length}件にも、送信時にこの設定が適用されます。` : `${what}を変更しました。次の送信から適用されます。`;

  const onPermission = (p: PermissionPreset) => {
    if (!chat || !live) return;
    host.setChatPermission(chat.key, p).then((r) => say(impactText("権限", r))).catch((e) => sayErr("権限を変更できませんでした", e));
  };

  const onCwd = async (cwd: string, confirmed: boolean): Promise<CwdResult> => {
    if (!chat || !live) return { kind: "error", message: "モックでは動きません" };
    try {
      say(impactText("作業フォルダ", await host.setChatCwd(chat.key, cwd, confirmed)));
      return { kind: "ok" };
    } catch (e) {
      const er = host.asIpcError(e);
      if (er.blocked?.kind === "queueRetargetUnconfirmed") return { kind: "needsConfirm", waiting: er.blocked.waiting };
      return { kind: "error", message: er.message };
    }
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

  /** 確認ダイアログのあとで、ユーザー操作として再開する（確認前・監視目的では呼ばない）。 */
  const resumeExternal = async (id: string) => {
    setDialog(null);
    if (!live) { say("この操作はモックでは動きません。"); return; }
    try {
      const out = await host.resumeChat({ backend: "codex", id });
      if (out.kind === "resumed") { say("会話を再開しました。続きを送れます。"); void loadChat(id); }
      else if (out.kind === "runningElsewhere") say("外部で実行中のため再開できません。終了してからもう一度お試しください。");
      else say(`再開できませんでした: ${out.reason}`);
    } catch (e) { sayErr("再開できませんでした", e); }
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

  /** 完全終了の選択（ホストが中断要求・停止照合・保存を進める）。「待つ」は画面を閉じるだけで、手順は続く。 */
  const decide = (d: QuitDecision) => {
    if (d === "wait") { setDialog((x) => (x?.type === "quit" ? null : x)); return; }
    host.quitDecision(d).then(setQuit).catch((e) => sayErr("終了の操作を実行できませんでした", e));
  };

  /** 強制終了の確認を開く（影響を受ける全チャットを先に示す。実行はしない）。 */
  const openForce = () => {
    if (!live) { setDialog({ type: "force" }); return; }
    const source = snap.sources[0]?.source;
    setForce({ preview: null, error: source ? null : "強制終了できる接続がありません。", running: false });
    setDialog({ type: "force" });
    if (!source) return;
    host.previewForceKill(source)
      .then((p) => setForce({ preview: p, error: null, running: false }))
      .catch((e) => setForce({ preview: null, error: host.asIpcError(e).message, running: false }));
  };

  /** 強制終了の実行（確認画面の「強制終了する」だけが呼ぶ）。プロセスの消滅を確認できなければ停止未確認のまま案内する。 */
  const runForce = async () => {
    const source = snap.sources[0]?.source;
    if (!source) return;
    setForce((f) => ({ ...f, running: true }));
    try {
      const recs = await host.forceKill(source);
      say(recs.some(stopOpen)
        ? "強制終了を要求しました。プロセスの消滅をまだ確認できていません。停止未確認のまま扱います。"
        : "強制終了しました。プロセスの消滅を確認しました。");
      setForce({ preview: null, error: null, running: false });
      setDialog(quit.kind !== "idle" ? { type: "quit" } : null);
    } catch (e) { setForce((f) => ({ ...f, running: false, error: host.asIpcError(e).message })); }
  };

  /** ダイアログを閉じる。終了の確認中は、段階に応じて「取り消す」か「待つ」にする（手順は画面を閉じても進み続ける）。 */
  const closeDialog = () => {
    if (live && dialog?.type === "quit") { decide(quit.kind === "confirming" || quit.kind === "saveFailed" ? "cancel" : "wait"); return; }
    if (live && dialog?.type === "force") { setDialog(quit.kind !== "idle" ? { type: "quit" } : null); return; }
    setDialog(null);
  };

  const act = (a: string) => {
    if (a.startsWith("unv:")) { setDialog({ type: "unv", why: a.slice(4) }); return; }
    if (a.startsWith("settings:")) { setDialog({ type: "settings", tab: a.split(":")[1] }); return; }
    switch (a) {
      case "noop": break;
      case "closeToTray":
        // 通常画面の閉じる＝トレイへ格納（確認なし。作業・収集・保存は続く）。終了は「AgentDock を終了」。
        if (live) host.closeThisWindow().catch((e) => sayErr("窓を閉じられませんでした", e)); else say("モックではトレイへ格納しません。");
        break;
      case "newChat": setDialog({ type: "newChat" }); break;
      case "quit":
        if (!live) { setDialog({ type: "quit" }); break; }
        host.requestQuit().then((p) => { setQuit(p); setDialog({ type: "quit" }); }).catch((e) => sayErr("終了を要求できませんでした", e));
        break;
      case "force": openForce(); break;
      case "attach": setDialog({ type: "attach" }); break;
      case "interrupt": void interrupt(); break;
      case "modeMenu": setMockOpen((o) => !o); break;
      case "resumeExternal": if (chat) setDialog({ type: "resumeExternal", chatId: chat.key.id }); break;
      case "doResumeExternal": if (dialog?.type === "resumeExternal") void resumeExternal(dialog.chatId); break;
      case "retryLaunch": if (live) retryLaunch(exePath); break;
      case "retrySave": void retrySave(); break;
      case "toggleLeft": if (narrow()) setLNarrow((v) => !v); else setLeftOpen((v) => !v); break;
      case "toggleRight": if (narrow()) setRNarrow((v) => !v); else setRightOpen((v) => !v); break;
      case "toggleMini":
        // 実接続では別窓の監視窓を開く（なければ作る）。モックでは画面内の仮の窓。
        if (live) host.openMonitorWindow().catch((e) => sayErr("監視窓を開けませんでした", e)); else setMini((v) => !v);
        break;
      case "closeMini": setMini(false); break;
      case "miniTop": case "toggleMiniTop": setTop("monitor", !miniTop); break;
      case "toggleMainTop": setTop("main", !mainTop); break;
      case "toggleDone": onScope({ kind: "allChats", showFinished: !(scope.kind === "allChats" && scope.showFinished) }); break;
      case "quitStop": case "doForce": setDialog(null); say("この操作はモックでは動きません。"); break;
      default: say("この操作は後続の段階で対応します。");
    }
  };

  const checked = {
    toggleLeft: narrow() ? lNarrow : leftOpen, toggleRight: narrow() ? rNarrow : rightOpen, toggleMini: mini,
    toggleDone: scope.kind === "allChats" && scope.showFinished, toggleMainTop: mainTop, toggleMiniTop: miniTop,
  };

  // 確認済みの失敗。実接続ではホストの記録（chatLocals）と現在の失敗turnの一致で判定し、左一覧と揃える。モックは画面内の集合。
  const confirmedKeys: Set<string> = live
    ? new Set(snap.agents.filter((a) => {
      if (a.status.state !== "failed") return false;
      const turn = a.status.turn ?? a.agent.latestTurn ?? "";
      const acked = snap.chatLocals.find((l) => l.chat.id === a.agent.chat.id)?.acknowledgedFailures ?? [];
      return acked.some((t) => t.agent.id === a.agent.key.id && t.turnId === turn);
    }).map((a) => keyStr(a.agent.key)))
    : confirmed;

  const dockProps = {
    snap, scope, selId, confirmed: confirmedKeys, onScope, onAct: act,
    onOpen: (id: string) => { selectChat(id); },
    onConfirmFail: (k: string) => {
      if (!live) setConfirmed((s) => new Set(s).add(k));
      const v = snap.agents.find((a) => keyStr(a.agent.key) === k);
      if (v) onAcknowledge(v.agent.chat, v.agent.key);
    },
  };

  return (
    <>
      <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
        <defs><pattern id="hatch" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="2" height="4" fill="#8A979C" /></pattern></defs>
      </svg>
      <div className="window" role="application" aria-label="AgentDock" onClick={() => { if (menu) setMenu(null); if (mockOpen) setMockOpen(false); }}>
        <TitleBar title="AgentDock" tag={modeTag} top={mainTop} onAct={act} />
        <MenuBar open={menu} setOpen={setMenu} checked={checked} onAct={act} />
        <GlobalBanner source={src} onAct={act} />
        {live && quit.kind !== "idle" && dialog?.type !== "quit" && dialog?.type !== "force" ? (
          <div className="gbanner warn" role="status">
            <span className="grow">終了の手順を続けています（停止と保存を確認中。確認できるまで終了しません）。</span>
            <button className="btn-line" onClick={() => setDialog({ type: "quit" })}>状況を見る</button>
          </div>
        ) : null}
        <SaveBanner failed={failedSaves.filter((s) => !forChat(s))} warnings={warnHidden ? [] : snap.startupWarnings} onDismissWarnings={() => setWarnHidden(true)} onAct={act} />
        <div className={`body ${leftOpen ? "" : "l-off"} ${rightOpen ? "" : "r-off"} ${lNarrow ? "n-l" : ""} ${rNarrow ? "n-r" : ""}`}>
          <aside className="left" aria-label="チャット一覧">
            <LeftPane snap={snap} sel={chat ? keyStr(chat.key) : null} onSelect={(id) => { selectChat(id); setLNarrow(false); }} onAct={act} onAcknowledge={(c) => onAcknowledge(c.key, null)} />
          </aside>
          <main className="center">
            {chat ? (
              <CenterPane
                snap={snap} chat={chat} turns={bundle.turns[chat.key.id] ?? []}
                attachments={bundle.attachments.filter((f) => keyStr(f.ownerChat) === keyStr(chat.key))}
                settings={bundle.modelSettings[chat.key.id]} models={models} save={chatSave}
                externalLabel={bundle.externalLabel[chat.key.id]} enterMode={enterMode}
                draft={drafts[chat.key.id] ?? localDraft ?? chat.draft ?? ""} setDraft={(v) => onDraft(chat, v)}
                onAct={act} onRespond={(r, a) => void onRespond(r, a)}
                onSend={onSend} onModel={onModel}
                notice={live ? noticeOf(attempts[chat.key.id], steerOf[chat.key.id] ? { attemptId: steerOf[chat.key.id].attemptId, queueInstead: () => { void onSend(steerOf[chat.key.id].text, false); } } : null) : null} onRetrySend={() => void onRetrySend()}
                queue={snap.queues.find((q) => q.chat.id === chat.key.id)} local={snap.chatLocals.find((l) => l.chat.id === chat.key.id)}
                qact={qact} onPermission={onPermission} onCwd={onCwd}
              />
            ) : <EmptyCenter onAct={act} />}
          </main>
          <aside className="right" aria-label="エージェントのドック"><DockPane {...dockProps} /></aside>
        </div>
      </div>
      {mini && !live ? <MiniWindow {...dockProps} top={miniTop} /> : null}
      {dialog ? (
        <Dialogs d={dialog} onClose={closeDialog} chats={snap.chats} source={src} models={models}
          enterMode={enterMode} setEnterMode={setEnterMode}
          notify={{ value: snap.settings.notifications, set: onNotifySettings }}
          top={{ main: mainTop, mini: miniTop, setMain: (b) => setTop("main", b), setMini: (b) => setTop("monitor", b) }}
          autostart={{
            value: snap.settings.autostart,
            set: (on) => {
              if (!live) { updateSnap((s) => ({ ...s, settings: { ...s.settings, autostart: on } })); return; }
              host.setAppSettings({ ...snap.settings, autostart: on }).catch((e) => sayErr("自動起動の設定を変更できませんでした", e));
            },
          }}
          quit={{ phase: quit, stops: snap.stops, failedSaves, live, decide, onForce: openForce, onRetrySave: () => void retrySave() }}
          force={{ ...force, run: () => void runForce() }}
          exe={{ path: exePath, setPath: setExePath, placeholder: DEFAULT_EXE, connect: retryLaunch, openDiag: () => { host.openDiagDir().then((p) => say(`診断ログの場所: ${p}`)).catch((e) => sayErr("診断ログの場所を開けませんでした", e)); }, live: live && snap.sources.every((s) => s.connection.kind !== "connected") }}
          onCreateChat={(i) => void createChat(i)}
          setTab={(t) => setDialog({ type: "settings", tab: t })} onAct={act} />
      ) : null}
      {toast ? <div className="toast" role="status">{toast}</div> : null}
      <div className="mockctl">
        {mockOpen ? (
          <div className="panel" role="menu">
            <p>接続先を切り替えます。モックは製品の画面ではなく、仮データの表示確認用です。</p>
            <button role="menuitemradio" aria-current={live} onClick={() => void changeMode("live")}>実接続（Codex App Server）</button>
            <button role="menuitemradio" aria-current={!live} onClick={() => void changeMode("mock")}>モック（仮データ）</button>
            {!live ? scenarios.map(([k, t]) => <button key={k} role="menuitemradio" aria-current={scenario === k} onClick={() => void changeScenario(k)}>{t}</button>) : null}
          </div>
        ) : null}
      </div>
    </>
  );
}

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { askConfirm } from "./ui/appConfirm";
import { archivedToast } from "./ui/chatDeleteUi";
import { openFileConfirm } from "./ui/confirmTexts";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import type { RepoRecordChoice, AgentKey, AppSettings, AttachmentEntry, Chat, ChatKey, Goal, GoalUpdate, Known, WorkMode, WorkModeInfo, WorkFoldersView, DeleteOutcome, DeletePreview, UsageReport, ForceKillPreview, HostEvent, HostEventEnvelope, HostSnapshot, ModelInfo, PathInfo, ProjectsOutcome, MonitorScope, PendingRequest, PermissionPreset, QuitDecision, QuitPhase, RequestAnswer, SendAttempt, SettingsImpact, SkillInfo, SourceInfo, OrgSnapshotView, IntakeBeginMode } from "./ipc/types";
import * as host from "./ipc/client";
import { LinkContext, RefContext, type PathAction, type RefActions } from "./ui/Markdown";
import { copyText } from "./ui/markdown/parts";
import { usePathStats } from "./ui/usePathStats";
import { PROJECTS_ENABLED } from "./ui/features";
import { MAX_FETCH_PAGES, countFetched, fetchAllPages, loadMoreMessage, projectFetchMessage } from "./ui/listFetch";
import { addDecision, mergeDecisions, type DecisionItem } from "./ui/decisions";
import { ACCEPTED_HOLD_MS, hasInFlight, mergeAttempt, outcomeOfAttempt, rebindPending, reconcilePending, setPhase, type PendingOutcome, type PendingSend } from "./ui/pendingSend";
import { withOrgTitles } from "./ui/orgChat";
import { GAP_GUIDE, GAP_TEXT, shownOrgIds } from "./ui/org";
import { applyHostEvent, createEventPipeline, emptyBundle, replaceSnapshot, seqAction, UNKNOWN_CAPS } from "./ipc/live";
import type { Bundle } from "./mock/data";

// 仮データはmockモードに入るときだけ読み込む（live製品バンドルに含めない）。
const loadMock = async () => {
  const [client, data] = await Promise.all([import("./mock/client"), import("./mock/data")]);
  return { client, scenarios: data.SCENARIOS };
};
import { GlobalBanner, MenuBar, SaveBanner } from "./ui/Chrome";
import { LeftPane, type ChatOp } from "./ui/LeftPane";
import { histStart, histOk, histFail, type HistLoad } from "./ui/historyLoad";
import { CenterPane, EmptyCenter, RequestCard, type FileActions } from "./ui/CenterPane";
import { hideClosed, noticeOf } from "./ui/sendNotice";
import { fenceCode, insertBlock } from "./ui/attach";
import { DockPane, MiniWindow, type DockActions, type DockFocus } from "./ui/Dock";
import { commandViews } from "./ui/commandViews";
import { cwdOpenWhy } from "./ui/commands";
import { matchShortcut } from "./ui/shortcuts";
import { recheckSummary } from "./ui/chatState";
import { gitKindOf, ownerMarkVisible, type GitKind } from "./ui/revertView";
import { CommandPalette } from "./ui/CommandPalette";
import { Dialogs, type DialogState, type NewChatInput } from "./ui/Dialogs";
import { OrgPane } from "./ui/OrgPane";
import { QuickNewPane } from "./ui/QuickNewPane";
import { pickQuickModel } from "./ui/quickNew";
import { enterModeFromSetting, enterModeToSetting, type EnterMode } from "./ui/sendKeys";
import { addCreating, creatingLabel, foundSince, removeCreating, resolveCreating, resultOfError, setCheckedNote, setCreatingPhase, type CreatingChat } from "./ui/creating";
import type { SaveResult, WorkFoldersApi } from "./ui/WorkFoldersUi";
import type { FolderTools } from "./ui/WorkFolderSetsUi";
import { SAME_FOLDER_UNCONFIRMED_NOTE, sameFolderInit } from "./ui/workFolderSets";
import { ackText } from "./ui/PrefsDialogs";
import { SidePanel } from "./ui/SidePanel";
import { fromLocal, type SentLocal } from "./ui/chips";
import { resendCtxOf, resendTargetOf, rootTurns, type ResendHandlers, type ResendState } from "./ui/ResendDialog";
import { resendAvailability } from "./ui/resend";
import { PaneSplitter } from "./ui/PaneSplitter";
import { displayWidths, loadSaved, paneModes, type PaneKind } from "./ui/paneWidth";
import { chatName, isRunning, isWorkingForDock, stopOpen } from "./ui/derive";
import { hms, keyStr, knownValue } from "./ui/format";
import { pushRecent } from "./ui/skillPick";
import { SendKeyContext } from "./ui/SendKeyContext";
import { applyAppearance, watchSystemTheme } from "./ui/appearance";

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
  connection: { kind: "launchFailed", reason: "spawnFailed", message }, sandboxWorkaround: false,
});

/** 送信の受理状態の案内。受理不明は失敗とも成功とも言わない。 */
const NO_SENDS: SentLocal[] = [];
const NO_DECISIONS: DecisionItem[] = [];
const NO_PENDING: PendingSend[] = [];
/** 「もう一度送る」で再送できる試行（最新の試行が未受理）の行には、「入力欄へ戻す」を出さない。 */
function markRetryable(list: PendingSend[] | undefined, a: SendAttempt | undefined): PendingSend[] {
  if (!list) return NO_PENDING;
  const can = a && (a.state.kind === "rejected" || a.state.kind === "notFoundAfterReconcile") ? a.attemptId : null;
  return can ? list.map((p) => (p.outcome?.kind === "rejected" && p.outcome.attemptId === can ? { ...p, retryable: true } : p)) : list;
}

export default function App() {
  const [mode, setMode] = useState<Mode>("live");
  const [scenario, setScenario] = useState("normal");
  const [scenarios, setScenarios] = useState<Array<[string, string]>>([]);
  const [bundle, setBundle] = useState<Bundle>(() => emptyBundle());
  const [models, setModels] = useState<ModelInfo[]>([]);
  /** プロジェクト一覧の取得結果（null＝未取得）。取得できなかったことを空の一覧にしない。 */
  const [projects, setProjects] = useState<ProjectsOutcome | null>(null);
  const [selId, setSelId] = useState<string | null>(null);
  const [scope, setScope] = useState<MonitorScope>({ kind: "selectedChat", chat: null });
  const [leftOpen, setLeftOpen] = useState(true);
  const [rightOpen, setRightOpen] = useState(true);
  const [lNarrow, setLNarrow] = useState(false);
  const [rNarrow, setRNarrow] = useState(false);
  const [mini, setMini] = useState(false);
  /** 実接続での監視窓の開閉（ホストの通知とメニューを開いたときの確認で更新。不明は false＝✓なし）。 */
  const [monitorOpen, setMonitorOpen] = useState(false);
  // 最前面。実接続ではホストの設定（再起動後も保持）、モックでは画面内だけ。
  const [mockMainTop, setMockMainTop] = useState(false);
  const [mockMiniTop, setMockMiniTop] = useState(false);
  const [quit, setQuit] = useState<QuitPhase>({ kind: "idle" });
  const [force, setForce] = useState<{ preview: ForceKillPreview | null; error: string | null; running: boolean }>({ preview: null, error: null, running: false });
  // 削除確認・エクスポート・使用量（P7）。実行結果はホストの応答を確認できたときだけ表示する。
  const [del, setDel] = useState<{ preview: DeletePreview | null; error: string | null; running: boolean; outcome: DeleteOutcome | null }>({ preview: null, error: null, running: false, outcome: null });
  const [exp, setExp] = useState<{ include: boolean; running: boolean; error: string | null }>({ include: false, running: false, error: null });
  const [usage, setUsage] = useState<{ report: UsageReport | null; error: string | null; loading: boolean }>({ report: null, error: null, loading: false });
  const [menu, setMenu] = useState<string | null>(null);
  const [palette, setPalette] = useState(false);
  const [dialog, setDialog] = useState<DialogState | null>(null);
  // 本文のリンクは確認ダイアログを経て開く（DESIGN_P5 §3.3）。
  const linkActions = useMemo(() => ({ openLink: (url: string, text: string) => setDialog({ type: "openLink", url, text }) }), []);
  /** 表示しているside相談（主会話を選んでいる間だけ出す。相談そのものはホストが持つ）。 */
  const [sideId, setSideId] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<Set<string>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  // 通知を押したチャットをドックのカードへ知らせる（スクロール・展開はドック側が入力中でないときだけ行う）
  const [dockFocus, setDockFocus] = useState<DockFocus | null>(null);
  const [mockOpen, setMockOpen] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [exePath, setExePath] = useState<string>(readExe);
  const [attempts, setAttempts] = useState<Record<string, SendAttempt>>({});
  // 送った発言のローカルの記録（送信直後の添付チップ用。メモリのみ。再読込み・再起動後は履歴から復元する）
  const [sends, setSends] = useState<Record<string, SentLocal[]>>({});
  /** キュー項目から追加指示で送り、受理不明のままの項目（項目ID→送信の試行ID）。 */
  /** 送信ボタンを押した直後からの依頼（チャット別。画面内の一時表示。受理されたものは本文に現れるまでの間だけ置く）。 */
  const [pendingSends, setPendingSends] = useState<Record<string, PendingSend[]>>({});
  const pendingSeq = useRef(0);
  /** 新しいチャットの作成中の仮行（左一覧の先頭）。作成中の同じ入力の多重呼出しは creatingKeys で止める。 */
  const [creating, setCreating] = useState<CreatingChat<NewChatInput>[]>([]);
  const creatingKeys = useRef(new Set<string>());
  const creatingSeq = useRef(0);
  const lastListItems = useRef<Chat[]>([]);
  const attemptsRef = useRef<Record<string, SendAttempt>>({});
  /** クイック新規の下書き（まだ会話を作っていない。メモリだけ）。 */
  /** 閉じた送信結果の通知（チャットID→通知の識別子）。状態が変われば別の識別子になり再び出る。 */
  const [noticeClosed, setNoticeClosed] = useState<Record<string, string>>({});
  const [quick, setQuick] = useState<{ open: boolean; text: string; model: string | null; busy?: boolean }>({ open: false, text: "", model: null });
  const quickRef = useRef<HTMLTextAreaElement>(null);
  const [steerUnknown, setSteerUnknown] = useState<Record<string, string>>({});
  const [warnHidden, setWarnHidden] = useState(false);
  // 組織実行（P-ORG-7b）。状態はホストの org_view（読取りのみ）。orgUpdated の合図と開いたときに取り直す。null＝未取得（ワークフローなしとは別）。
  const [org, setOrg] = useState<OrgSnapshotView | null>(null);
  const [orgErr, setOrgErr] = useState<string | null>(null);
  const [orgTick, setOrgTick] = useState(0);
  // 進行タイムラインの合図（orgTimelineUpdated。1秒に1回まで）。経過の表示が差分を取り直す。
  const [orgTlTick, setOrgTlTick] = useState(0);
  // 組織ワークフローの削除の進行（orgDeleteProgress。表示だけ）。
  const [orgDelProg, setOrgDelProg] = useState<{ workflowId: string; done: number; total: number } | null>(null);
  /** 中央にワークフロー画面を出しているか（役割チャットを選ぶと閉じる）。 */
  const [orgScreen, setOrgScreen] = useState(false);
  /** ワークフロー画面に出す過去のワークフローのID（null＝実行中の1件）。保存済みの記録を読み取り専用で開くだけ。 */
  const [orgSel, setOrgSel] = useState<string | null>(null);
  const orgSeq = useRef(0);
  const [listStatus, setListStatus] = useState<{ ok: boolean; text: string } | null>(null);
  /** 通常の一覧の続き（nextCursor）。null＝続きなし（または未取得）。「続きを読み込む」だけが使う。 */
  const [listCursor, setListCursor] = useState<string | null>(null);
  const [steerOf, setSteerOf] = useState<Record<string, { attemptId: string; text: string; attachments: string[] }>>({});
  const [dragging, setDragging] = useState(false);
  const [changesTick, setChangesTick] = useState(0);
  // 目標（Goal）の表示。読取りで得た値と、更新通知で置き換えた値（ホストが持つ値のコピー。保存はしない）。
  const [goals, setGoals] = useState<Record<string, Known<Goal>>>({});
  // 応答した承認・質問・MCP許可の記録（項目51。ホストが保存したものと、このセッションで受けた通知。チャット別）
  const [decisions, setDecisions] = useState<Record<string, DecisionItem[]>>({});
  const [goalBusy, setGoalBusy] = useState(false);
  const [workModes, setWorkModes] = useState<WorkModeInfo[]>([]);
  const toastTimer = useRef<number | undefined>(undefined);
  const selRef = useRef<string | null>(null);
  const quitKindRef = useRef<QuitPhase["kind"]>("idle");
  selRef.current = selId;
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const dropRef = useRef<(paths: string[]) => void>(() => undefined);
  const live = mode === "live";

  // 画面幅の区分（D07）。並置か一時パネルかは paneModes（中央480・ドック最小260・左の表示幅から判定）。
  // 一時パネルの開閉は通知・新しい活動では変えない（利用者の操作だけ）。保存幅は変えない。
  const [winW, setWinW] = useState(() => window.innerWidth);
  useEffect(() => {
    const h = () => setWinW(window.innerWidth);
    window.addEventListener("resize", h);
    return () => window.removeEventListener("resize", h);
  }, []);
  const say = useCallback((t: string, ms = 3200) => {
    setToast(t);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), ms);
  }, []);
  const sayErr = useCallback((prefix: string, e: unknown) => say(`${prefix}: ${host.asIpcError(e).message}`), [say]);

  /** 保存履歴から会話本文を読み込む（resumeしない）。 */
  /** プロジェクト一覧を取り直す（読取りのみ。一覧の更新と同時に呼ぶ）。通信などで取れなかったときは「取得できなかった」と記録する。 */
  const refreshProjects = useCallback(async () => {
    if (!PROJECTS_ENABLED) return; // プロジェクト機能の一時停止中は project/list を呼ばない
    try { setProjects(await host.listProjects()); }
    catch (e) { setProjects({ kind: "failed", reason: host.asIpcError(e).message }); }
  }, []);

  const [histLoads, setHistLoads] = useState<Record<string, HistLoad>>({});
  const loadChat = useCallback(async (id: string) => {
    setHistLoads((m) => ({ ...m, [id]: histStart(m[id]) }));
    try {
      const h = await host.openChat({ backend: "codex", id });
      setBundle((b) => ({ ...b, turns: { ...b.turns, [id]: h.turns } }));
      setHistLoads((m) => ({ ...m, [id]: histOk() }));
    } catch (e) {
      // 取得失敗では既存の表示（turns）を消さない。失敗は中央ペインに常時表示する。
      const st = histFail(host.asIpcError(e).message);
      setHistLoads((m) => ({ ...m, [id]: st }));
      if (st.kind !== "empty") sayErr("履歴を取得できませんでした", e);
    }
  }, [sayErr]);

  const noteAttempt = useCallback((chatId: string, a: SendAttempt) => {
    attemptsRef.current = { ...attemptsRef.current, [chatId]: mergeAttempt(attemptsRef.current[chatId], a) };
    // 最初の依頼の行は、受理後に一定時間で外す（本文に現れれば先に外れる）。
    if (a.state.kind === "accepted") window.setTimeout(() => setPendingSends((m) => (m[chatId]?.some((x) => x.attemptId === a.attemptId) ? { ...m, [chatId]: m[chatId].filter((x) => x.attemptId !== a.attemptId) } : m)), ACCEPTED_HOLD_MS);
    setAttempts((m) => ({ ...m, [chatId]: mergeAttempt(m[chatId], a) }));
    // 受理不明だった依頼の行は、照合で確定したときにここで更新する（同じ試行の結果だけ）。
    setPendingSends((m) => {
      const cur = m[chatId];
      if (!cur) return m;
      const next = reconcilePending(cur, a);
      return next === cur ? m : { ...m, [chatId]: next };
    });
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
      setListCursor(page.nextCursor);
      if (!selRef.current && page.items[0]) {
        const id = page.items[0].chat.key.id;
        setSelId(id);
        void loadChat(id);
      }
    } catch (e) { sayErr("会話の一覧を取得できませんでした", e); }
    try { setModels(await host.listModels()); } catch (e) { sayErr("モデル一覧を取得できませんでした", e); }
    void refreshProjects();
    return true;
  }, [loadChat, sayErr, refreshProjects]);

  // 実接続: 購読 → スナップショット → 接続。seqが飛んだらスナップショットを取り直す。
  useEffect(() => {
    if (!live) return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    let ready = false;
    let last = 0;
    let resyncing = false;
    const pipe = createEventPipeline((events) => setBundle((b) => events.reduce(applyHostEvent, b)));
    const pending: HostEventEnvelope[] = [];

    /** スナップショットの終了手順の状態を反映する（再読込み後の復元）。段階が変わった／初回で手順の途中なら確認画面を開く。 */
    const restoreQuit = (s: HostSnapshot, initial: boolean) => {
      const prev = quitKindRef.current;
      quitKindRef.current = s.quit.kind;
      setQuit(s.quit);
      if (s.quit.kind === "idle") { if (prev !== "idle") setDialog((d) => (d?.type === "quit" ? null : d)); }
      else if (initial || prev !== s.quit.kind) setDialog((d) => (d?.type === "force" ? d : { type: "quit" }));
    };
    const resync = async () => {
      if (resyncing) return;
      resyncing = true;
      try {
        const snap = await host.getSnapshot();
        if (!alive) return;
        last = snap.seq;
        pipe.flushAll();
        setBundle((b) => replaceSnapshot(b, snap));
        restoreQuit(snap, false);
        setOrgTick((n) => n + 1);
      } catch (e) { sayErr("状態を取り直せませんでした", e); } finally { resyncing = false; }
    };
    const effects = (e: HostEvent) => {
      if (e.kind === "turnUpdated" && e.end !== null && e.turn.agent.id === selRef.current) void loadChat(e.turn.agent.id);
      else if (e.kind === "orgUpdated") setOrgTick((n) => n + 1); // 組織実行の状態の合図。画面側が org_view を取り直す（読取りのみ）
      else if (e.kind === "orgDeleteProgress") setOrgDelProg({ workflowId: e.workflowId, done: e.done, total: e.total }); // 削除の進行（表示だけ）
      else if (e.kind === "orgTimelineUpdated") setOrgTlTick((n) => n + 1); // 経過の合図。画面側が org_timeline で差分だけ取り直す（読取りのみ）
      else if (e.kind === "sendUpdated") noteAttempt(e.chat.id, e.attempt);
      else if (e.kind === "sendPhaseUpdated") setPendingSends((m) => { const cur = m[e.chat.id]; if (!cur) return m; const next = setPhase(cur, e.phase); return next === cur ? m : { ...m, [e.chat.id]: next }; }); // 送信中の段階（表示だけ）
      else if (e.kind === "changesUpdated") setChangesTick((n) => n + 1); // 開いている差分表示が取り直す（読取りのみ）
      else if (e.kind === "decisionRecorded") setDecisions((m) => ({ ...m, [e.record.chat.id]: addDecision(m[e.record.chat.id] ?? [], { record: e.record, save: e.save }) })); // 応答の記録（表示だけ。応答の代行・再送はしない）
      else if (e.kind === "goalUpdated") setGoals((m) => ({ ...m, [e.chat.id]: e.goal ? { kind: "value", value: e.goal, basis: "direct" } : { kind: "missing" } })); // 目標の表示を置き換える（エージェント状態には使わない）
      else if (e.kind === "navigateToChat") { setOrgScreen(false); setSelId(e.chat.id); setQuick((q) => (q.open ? { ...q, open: false } : q)); void loadChat(e.chat.id); setDockFocus((f) => ({ id: e.chat.id, tick: (f?.tick ?? 0) + 1 })); } // 通知を開いた操作。表示だけで、回答・再実行はしない
      else if (e.kind === "quitPrompt") { quitKindRef.current = e.phase.kind; setQuit(e.phase); setDialog((d) => (d?.type === "force" ? d : { type: "quit" })); } // もう一度「終了」が要求された。確認画面を開き直す
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
      else if (e.kind === "warning") say(`警告${e.rawLabel ? `（${e.rawLabel}）` : ""}: ${e.message}`, 15000); // Codex 由来の通知などは読めるよう長めに出す
    };
    const handle = (env: HostEventEnvelope) => {
      const a = seqAction(last, env.seq);
      if (a === "ignore") return;
      if (a === "resync") { void resync(); return; }
      last = env.seq;
      pipe.push(env.event);
      effects(env.event);
    };

    setBundle(emptyBundle()); setSelId(null); setModels([]); setProjects(null); setAttempts({}); setScope({ kind: "selectedChat", chat: null });
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
          if (page) setListCursor(page.nextCursor);
          if (page?.items[0] && !selRef.current) { setSelId(page.items[0].chat.key.id); void loadChat(page.items[0].chat.key.id); }
          setModels(await host.listModels().catch(() => []));
          void refreshProjects();
        }
        if (!alive) return;
        last = snap.seq;
        pipe.flushAll();
        setBundle((b) => replaceSnapshot(b, snap));
        setScope(snap.monitorScope);
        restoreQuit(snap, true);
        ready = true;
        for (const env of pending.splice(0)) handle(env);
      } catch (e) {
        setConnectError(host.asIpcError(e).message);
        ready = true;
      }
    })();
    return () => { alive = false; unlisten?.(); pipe.dispose(); };
  }, [live, boot, loadChat, noteAttempt, say, sayErr, refreshProjects]);

  // 組織の役割チャットの表示名だけ上書きする（表示用。保存済みの名前は変えない）
  const snap: HostSnapshot = useMemo(() => [org?.workflow, ...(org?.history ?? []), ...(org?.archived ?? [])].reduce((s, w) => withOrgTitles(s, w), bundle.snapshot), [bundle.snapshot, org]);
  // 過去のワークフローを開いているときは、その1件を画面に出す（なければ実行中の1件）。役割チャットを通常の一覧から隠すかは、出せるワークフローかで決める。
  const orgHist = orgSel ? [...(org?.history ?? []), ...(org?.archived ?? [])].find((w) => w.workflowId === orgSel) ?? null : null;
  const orgShown: OrgSnapshotView | null = org && orgHist ? { ...org, workflow: orgHist } : org;
  const orgShownIds = useMemo(() => shownOrgIds(org, !!orgErr), [org, orgErr]);

  // 組織実行の状態を取り直す（読取りのみ。何も送らない）。古い応答は捨てる。
  useEffect(() => {
    const seq = ++orgSeq.current;
    void (async () => {
      try {
        const v = live ? await host.orgView() : await (await loadMock()).client.orgView(scenario);
        if (seq === orgSeq.current) { setOrg(v); setOrgErr(null); }
      } catch (e) { if (seq === orgSeq.current) setOrgErr(host.asIpcError(e).message); }
    })();
  }, [live, scenario, orgTick]);
  /** 組織実行の命令の実行。実接続でなければ動かさない。成功したら状態を取り直し、失敗は理由を通知する（成功を断定する文は f が返す）。 */
  const orgRun = useCallback(async (label: string, f: () => Promise<string | void>): Promise<boolean> => {
    if (!live) { say("この操作はモックでは動きません。"); return false; }
    try {
      const m = await f();
      setOrgTick((n) => n + 1);
      if (typeof m === "string" && m) say(m, 6000);
      return true;
    } catch (e) { sayErr(`${label}に失敗しました`, e); return false; }
  }, [live, say, sayErr]);
  // 依頼整理のチャットを作って開く（組織実行はまだ始まらない。開始は要約の確認ダイアログの「開始」だけ）。
  const intakeBegin = async (folder: string, mode: IntakeBeginMode): Promise<string | null> => {
    if (!live) return "モックでは動きません。";
    try {
      const r = await host.orgIntakeBegin(folder, mode);
      setDialog(null); setOrgScreen(false); setSelId(r.chat.key.id);
      say(mode === "write" ? "依頼整理のチャットを作りました（組織実行はまだ始まっていません）。下の要約に依頼を書いてください。" : "依頼整理のチャットを作りました（組織実行はまだ始まっていません）。AI と依頼を詰めて、「要約を作る」を押してください。", 9000);
      return null;
    } catch (e) { return host.asIpcError(e).message; }
  };
  // 依頼整理の確認ダイアログの「開始」で組織実行が始まった。
  const intakeStarted = (_workflowId: string, warning: string | null) => {
    setDialog(null); setOrgScreen(true); setOrgTick((n) => n + 1);
    say(warning ?? "組織実行を開始しました。Planner が計画を作ります。計画に確認が必要な内容があれば、承認するまでタスクは送られません。", warning ? 10000 : 8000);
  };
  // このアプリの起動中に一度でも live 監視になったエージェント（再起動後の保存履歴表示と、接続断からの回復を区別する）。
  const liveSeen = useRef<Set<string>>(new Set());
  // 設定の直列更新（updateSettings）用。ref は最後に保存できた設定。保存中でなければホストの最新に追従する。
  const settingsRef = useRef(snap.settings);
  const settingsQueue = useRef<Promise<void>>(Promise.resolve());
  const settingsPending = useRef(0);
  useEffect(() => { if (settingsPending.current === 0) settingsRef.current = snap.settings; }, [snap.settings]);
  // 依頼整理の帯・要約カード（入力欄まわり）からの合図: ダイアログを開く／ワークフロー画面を開く。送信・開始はしない（開くだけ）。
  useEffect(() => {
    const onDialog = (e: Event) => { const d = (e as CustomEvent<DialogState | undefined>).detail; if (d) setDialog((cur) => cur ?? d); };
    const onOrg = () => { setOrgScreen(true); setLNarrow(false); };
    window.addEventListener("agentdock:dialog", onDialog);
    window.addEventListener("agentdock:open-org", onOrg);
    return () => { window.removeEventListener("agentdock:dialog", onDialog); window.removeEventListener("agentdock:open-org", onOrg); };
  }, []);
  for (const v of snap.agents) if (v.freshness === "live") liveSeen.current.add(v.agent.key.id);
  const mainTop = live ? snap.settings.mainWindow.alwaysOnTop : mockMainTop;
  const miniTop = live ? snap.settings.monitorWindow.alwaysOnTop : mockMiniTop;
  /** 最前面の切替（窓ごと）。フォーカスは移さない。設定はホストが保存する。 */
  const setTop = (kind: "main" | "monitor", on: boolean) => {
    if (!live) { if (kind === "main") setMockMainTop(on); else setMockMiniTop(on); return; }
    host.setAlwaysOnTop(kind, on).catch((e) => sayErr("最前面を切り替えられませんでした", e));
  };
  const src: SourceInfo | undefined = snap.sources[0] ?? (live && connectError ? failedSource(connectError) : undefined);
  const chat = snap.chats.find((c) => c.key.id === selId) ?? null;
  const connected = src?.connection.kind === "connected";
  /** 主要ボタン・メニュー「作業」・Ctrl+K が共有する操作の可否（台帳から導く）。 */
  // 選択中チャットの作業フォルダがGitリポジトリか（`git_info` の読取りだけ。チャット選択時と作業フォルダ変更時に1回）。確認できていないときは無効にしない。
  const gitCwd = chat?.cwd.kind === "value" ? chat.cwd.value : null;
  const [gitKinds, setGitKinds] = useState<Record<string, GitKind>>({});
  const [gitRoots, setGitRoots] = useState<Record<string, string | null>>({});
  useEffect(() => {
    if (!live || !gitCwd) return;
    let alive = true;
    setGitKinds((m) => { const n = { ...m }; delete n[gitCwd]; return n; }); // 取り直す間は古い判定を残さない（unknown＝塞がない）
    host.gitInfo(gitCwd).then((i) => ({ k: gitKindOf(i), root: i.root ?? null }), () => ({ k: "unknown" as GitKind, root: null })).then(({ k, root }) => { if (alive) { setGitKinds((m) => ({ ...m, [gitCwd]: k })); setGitRoots((m) => ({ ...m, [gitCwd]: root })); } });
    return () => { alive = false; };
  }, [live, gitCwd, selId]);
  // 入口の印: 作業フォルダが所有者違いで、「記録しない」も許可も選んでいないとき、「変更を確認」に中立の小さな印を付ける。
  const changesMark = live && !!gitCwd && ownerMarkVisible(gitKinds[gitCwd], snap.settings.baselines.repos, gitRoots[gitCwd], gitCwd);
  const goalSupported = snap.opCapabilities.some((c) => c.op === "goal" && c.support === "supported");
  const workModeSupported = snap.opCapabilities.some((c) => c.op === "workMode" && c.support === "supported");
  /** 目標を読む（読取りのみ。会話は再開しない）。読めなければ未取得のまま表示する。 */
  const loadGoal = useCallback(async (key: ChatKey) => {
    try {
      const g = await host.getGoal(key);
      setGoals((m) => ({ ...m, [key.id]: g }));
    } catch { setGoals((m) => ({ ...m, [key.id]: { kind: "notFetched" } })); }
  }, []);
  const decisionChatKey = chat?.key ?? null;
  useEffect(() => {
    if (!live || !decisionChatKey) return;
    const id = decisionChatKey.id;
    let alive = true;
    // 読めなかったときは、読めた分（このセッションで受けた通知）だけを出す。「記録なし」とは言わない。
    host.listDecisionRecords(decisionChatKey).then((l) => { if (alive) setDecisions((m) => ({ ...m, [id]: mergeDecisions(l, m[id] ?? []) })); }).catch(() => undefined);
    return () => { alive = false; };
  }, [live, decisionChatKey?.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const goalChatKey = chat?.key ?? null;
  useEffect(() => {
    if (live && connected && goalSupported && goalChatKey) void loadGoal(goalChatKey);
  }, [live, connected, goalSupported, goalChatKey?.id, loadGoal]); // eslint-disable-line react-hooks/exhaustive-deps
  // 計画／実行の選択肢（Codex から取得。取れなければ画面側の既定の名前を使う）。
  useEffect(() => {
    if (!live || !connected || !workModeSupported) { setWorkModes([]); return; }
    host.listWorkModes().then(setWorkModes).catch(() => setWorkModes([]));
  }, [live, connected, workModeSupported]);
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
  // 左一覧・ドックの幅（UI-7）。保存値は窓幅で変えず、表示幅だけを縮める。保存は離したとき・キー操作の最後から500ms後に1回。
  const [layoutOv, setLayoutOv] = useState<{ leftWidth: number | null; dockWidth: number | null } | null>(null);
  const savedLayout = layoutOv ?? snap.settings.layout;
  // ホストの保存値が追いついたら一時値を捨てる（以後はホストの値に従う。他の経路で更新されても反映される）。保存に失敗している間だけ一時値が残る。
  useEffect(() => {
    const l = snap.settings.layout;
    setLayoutOv((o) => (o && o.leftWidth === l.leftWidth && o.dockWidth === l.dockWidth ? null : o));
  }, [snap.settings.layout.leftWidth, snap.settings.layout.dockWidth]); // eslint-disable-line react-hooks/exhaustive-deps
  const { leftTemp, rightTemp } = paneModes({ winW, leftOpen, rightOpen, left: loadSaved("left", savedLayout.leftWidth) });
  // 広い幅へ戻ったら、一時パネルの「開いた」状態は持ち越さない（次に狭くなったとき勝手に開かない）。
  useEffect(() => { if (!leftTemp) setLNarrow(false); }, [leftTemp]);
  useEffect(() => { if (!rightTemp) setRNarrow(false); }, [rightTemp]);
  const paneW = displayWidths({
    winW, left: loadSaved("left", savedLayout.leftWidth), dock: loadSaved("dock", savedLayout.dockWidth),
    leftShown: !leftTemp && leftOpen, dockShown: !rightTemp && rightOpen,
  });
  const onPaneCommit = (kind: PaneKind, w: number | null) => {
    const layout = kind === "left" ? { ...savedLayout, leftWidth: w } : { ...savedLayout, dockWidth: w };
    setLayoutOv(layout);
    updateSettings((s) => ({ ...s, layout }), "パネルの幅を保存できませんでした");
  };
  const onNotifySettings = (n: HostSnapshot["settings"]["notifications"]) => {
    updateSettings((s) => ({ ...s, notifications: n }), "通知設定を保存できませんでした");
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
  const curDraft = chat ? (drafts[chat.key.id] ?? localDraft ?? chat.draft ?? "") : "";
  // 送信前の添付。実接続ではホストの下書き（draft.attachments）の順。モックは仮データの添付をすべて出す。
  const draftAttachments: AttachmentEntry[] = !chat ? [] : live
    ? (snap.chatLocals.find((l) => l.chat.id === chat.key.id)?.draft.attachments ?? []).flatMap((id) => snap.attachments.find((a) => a.id === id) ?? [])
    : snap.attachments.filter((a) => a.chat.id === chat.key.id);
  // 表示するside相談。選んでいるチャットが相談の主会話のときだけ出す。
  const sideShown = chat && sideId ? snap.sideSessions.find((x) => x.id === sideId && x.main.id === chat.key.id) : undefined;
  const v = src?.version;
  // 現在の接続先を示す。実接続では接続中のCodexの版を出す（版が取れなければ、そう表示する）。
  const modeTag = !live ? "モック（仮データ）"
    : !src ? "Codex 未接続"
    : v?.kind === "match" ? `Codex ${v.version}`
    : v?.kind === "mismatch" ? `Codex ${v.actual}（対象版外）`
    : "Codex 版を確認できません";

  const selectChat = (id: string | null) => {
    setOrgScreen(false);
    setSelId(id);
    setQuick((q) => (q.open ? { ...q, open: false } : q)); // 下書きは隠すだけ（文は残る）
    // 履歴のないチャット（記録だけの表示）は、読み込みを試みない。
    if (live && id && !snap.chats.find((c) => c.key.id === id)?.noHistory) void loadChat(id);
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

  /** 設定の更新（直列化）。`setAppSettings` は全置換なので、前の保存の完了（結果）を待ってからその最新値に `f` を適用して送る。連続操作で古い値に戻らない。
   *  失敗したらトースト（画面の表示は戻さない。保存されていないので次回起動で保存値に戻る）。モックは画面内だけ。 */
  const updateSettings = (f: (s: AppSettings) => AppSettings, failText: string): Promise<boolean> => {
    if (!live) { updateSnap((s) => ({ ...s, settings: f(s.settings) })); return Promise.resolve(true); }
    settingsPending.current++;
    const run = settingsQueue.current.then(async (): Promise<boolean> => {
      try {
        settingsRef.current = await host.setAppSettings(f(settingsRef.current));
        return true;
      } catch (e) {
        sayErr(failText, e);
        return false;
      } finally {
        settingsPending.current--;
      }
    });
    settingsQueue.current = run.then(() => undefined);
    return run;
  };
  // 送信キー（AppSettings.sendKey）。以前は画面内の状態で再起動すると戻っていたため、設定に保存する
  const enterMode = enterModeFromSetting(snap.settings.sendKey);
  const setEnterMode = (m: EnterMode) => { void updateSettings((st) => ({ ...st, sendKey: enterModeToSetting(m) }), "送信キーの設定を保存できませんでした。次回の起動では保存されている値に戻ります"); };
  // 表示設定（テーマ・文字サイズ）の反映。ホストの設定を受け取るまで（既定値のまま）は写しを上書きしない。
  const appearance = snap.settings.appearance;
  const appearanceReady = !live || snap.seq > 0;
  useEffect(() => {
    if (!appearanceReady) return;
    applyAppearance(appearance);
    return watchSystemTheme(appearance);
  }, [appearanceReady, appearance.theme, appearance.uiText, appearance.bodyText]); // eslint-disable-line react-hooks/exhaustive-deps

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
        : out.kind === "notDeliverable" ? `${GAP_TEXT[out.cause]} ${GAP_GUIDE}`
        : `回答の受理を確認できません（${out.message}）。再送はしません。`);
    } catch (e) { sayErr("回答を送れませんでした", e); }
  };

  /** 送信（対象チャットを引数で受ける。中央の入力欄とドックの指示欄が同じ判定を通る）。 */
  const sendTo = async (chat: Chat, text: string, steer: boolean, attachments: string[]): Promise<boolean> => {
    if (!live) { void loadMock().then((m) => m.client.sendMessage(chat.key, text, steer ? "steer" : "newTurn")); say("送信を依頼しました（モック）。"); return true; }
    const running = isRunning(snap, chat);
    if (running && !steer) {
      // 完了後に送る依頼として登録する（送信はホストが条件を判断して1件ずつ行う）。
      try { await host.enqueue(chat.key, text, attachments); say("完了後に送る依頼として登録しました。"); return true; } catch (e) { sayErr("依頼を登録できませんでした", e); return false; }
    }
    // 応答待ちの送信がある間は、同じ入力を二重に送らない（下書きは残す）。
    const chatId = chat.key.id;
    if (hasInFlight(pendingSends[chatId] ?? [])) return false;
    // 送る添付の名前・種類・コピー先を、送信前の台帳から控える（送信で下書きから外れる前）
    const sentAtts = attachments.map((id) => snap.attachments.find((x) => x.id === id)).filter((x): x is AttachmentEntry => !!x).map(fromLocal);
    // 押した直後に、会話へ「送信中」の依頼として出す（入力欄は空になる）。結果は下の応答で確定する。
    const pid = `ps${++pendingSeq.current}`;
    const setOutcome = (outcome: PendingOutcome) => {
      setPendingSends((m) => ({ ...m, [chatId]: (m[chatId] ?? []).map((x) => (x.id === pid ? { ...x, outcome } : x)) }));
      // 受理後は、会話の本文に現れるのを待つ。現れなくても一定時間で仮の行は外す（受理済みの記録はホストと履歴が持つ）。
      if (outcome.kind === "accepted") window.setTimeout(() => setPendingSends((m) => ({ ...m, [chatId]: (m[chatId] ?? []).filter((x) => x.id !== pid) })), ACCEPTED_HOLD_MS);
    };
    setPendingSends((m) => ({ ...m, [chatId]: [...(m[chatId] ?? []), { id: pid, text, startedAt: Date.now(), steer: running, phase: null, outcome: null }] }));
    void (async () => {
      try {
        const a = await host.sendMessage(chat.key, text, running ? "steer" : "newTurn", attachments);
        noteAttempt(chatId, a);
        const o = outcomeOfAttempt(a);
        if (o) setOutcome(o);
        // 受理を確認できた送信だけを、添付なしも含めて控える（同じ本文の別の発言に他の送信の添付が付かないように）
        if (a.state.kind === "accepted") setSends((m) => ({ ...m, [chatId]: [...(m[chatId] ?? []), { text, at: a.at, atts: sentAtts, steer: running }].slice(-30) }));
        if (running) {
          setSteerOf((m) => ({ ...m, [chatId]: { attemptId: a.attemptId, text, attachments } }));
          if (a.state.kind === "accepted") say("追加指示を Codex が受理しました。現在のやりとりへの反映は会話の表示で確認してください（受理と反映は別です）。");
        }
      } catch (e) {
        // ホストが送る前に止めた（何も送っていない）。入力した文は仮の行に残し、入力欄へ戻せる。
        setOutcome({ kind: "failed", message: host.asIpcError(e).message });
        sayErr("送信できませんでした", e);
      }
    })();
    return true;
  };
  const onSend = async (text: string, steer: boolean, attachments: string[]): Promise<boolean> => (chat ? sendTo(chat, text, steer, attachments) : false);

  /** 「引き継いで送信」（DESIGN_CONT §7.3）。利用者の送信操作の中だけで呼ぶ。拒否・受理不明で再送しない。 */
  const onHandoffSend = async (text: string, attachments: string[]): Promise<boolean> => {
    if (!chat) return false;
    if (!live) { say("引き継ぎはモックでは動きません。"); return false; }
    try {
      const r = await host.handoffAndSend(chat.key, text, attachments);
      if (r.stage === "notStarted") {
        say(r.startUnknown ? `新しい thread を開始できたか確認できません（${r.message}）。再送はしません。一覧で確認してください。` : `引き継ぎを始められませんでした（${r.message}）。何も送っていません。`, 7000);
        return false;
      }
      const nid = r.chat.key.id;
      selectChat(nid);
      if (r.stage === "createdNotSent") {
        // 依頼の文を新しい thread の下書きへ入れる（画面に出し、保存も試みる）。元のチャットの下書きは消さない（false を返す）
        setDrafts((d) => ({ ...d, [nid]: text }));
        const saved = await host.setDraft(r.chat.key, text).then(() => true, () => false);
        say(`新しい thread は作りましたが、依頼は送っていません（${r.message}）。依頼の文は新しい thread の入力欄に入れました${saved ? "" : "（下書きの保存は確認できていません）"}。新しい thread の入力欄の添付を確認してください（付いていなければ付け直してください）。`, 8000);
        return false;
      }
      noteAttempt(nid, r.send);
      const cur = attemptsRef.current[nid] ?? r.send;
      const row: PendingSend = { id: `ps${++pendingSeq.current}`, text, startedAt: Date.now(), steer: false, phase: null, outcome: outcomeOfAttempt(cur), attemptId: cur.attemptId, first: true };
      setPendingSends((m) => ({ ...m, [nid]: [...(m[nid] ?? []), row] }));
      return true;
    } catch (e) { sayErr("引き継いで送信できませんでした", e); return false; }
  };
  /** 引き継ぎ資料の確認（読取りのみ）。モックは仮の資料。 */
  const loadHandoffPreview = (k: ChatKey) => (live ? host.previewHandoff(k) : loadMock().then((m) => m.client.previewHandoff(k)));
  /** 目的メモの保存。失敗の文（成功なら null）を返す。 */
  const savePurpose = async (chatId: string, text: string, expectedRevision: number | null): Promise<string | null> => {
    if (!live) return "モックでは保存できません。";
    try { await host.setPurposeNote({ backend: "codex", id: chatId }, text, expectedRevision); say(text.trim() === "" ? "目的メモを削除しました。" : "目的メモを保存しました。"); return null; } catch (e) { return host.asIpcError(e).message; }
  };

  const pendingRestore =(c: Chat, p: PendingSend) => {
    // 入力欄に別の文があれば、その前に戻した文を置く（どちらも失わない）。
    const cur = drafts[c.key.id] ?? "";
    onDraft(c, cur.trim() ? `${p.text}

${cur}` : p.text);
    pendingDismiss(c, p);
    composerRef.current?.focus();
  };
  const pendingDismiss = (c: Chat, p: PendingSend) => setPendingSends((m) => ({ ...m, [c.key.id]: (m[c.key.id] ?? []).filter((x) => x.id !== p.id) }));

  const qact = {
    edit: async (id: string, text: string): Promise<boolean> => {
      if (!chat || !live) return false;
      // 添付は今のまま（編集で外れないよう、登録済みの添付を渡す）。
      const kept = snap.queues.find((q) => q.chat.id === chat.key.id)?.entries.find((x) => x.id === id)?.attachments ?? [];
      try { await host.editQueueEntry(chat.key, id, text, kept); return true; } catch (e) { sayErr("依頼を編集できませんでした", e); return false; }
    },
    cancel: (id: string) => { if (chat && live) host.cancelQueueEntry(chat.key, id).catch((e) => sayErr("依頼を取り消せませんでした", e)); },
    resume: () => {
      if (!chat || !live) return;
      host.resumeQueue(chat.key).then(() => say("キューを再開しました。条件がそろえば、登録順に1件ずつ送ります。")).catch((e) => sayErr("キューを再開できませんでした", e));
    },
    recheck: () => {
      if (!chat || !live) return;
      host.recheckQueueState(chat.key).then(() => say("子孫の履歴を再確認しました。終端を確認できていれば、まもなく自動で送信に進みます。確認できなければ、保留のままです。")).catch((e) => sayErr("状態を再確認できませんでした", e));
    },
    sendNow: async (entry: string): Promise<boolean> => {
      if (!chat || !live) return false;
      try { await host.sendQueueEntryNow(chat.key, entry); say("先頭の依頼の送信を依頼しました。受理されたかどうかは、会話とキューの表示で確認してください。"); return true; } catch (e) { sayErr("今すぐ送れませんでした", e); return false; }
    },
    /** キュー項目を追加指示として今送る（押したとき1回だけ）。受理されたら元の項目を消す。受理不明は項目を残し、再送しない。 */
    steerUnknown,
    steerEntry: async (e: { id: string; text: string; attachments: string[] }): Promise<void> => {
      if (!chat || !live) return;
      if (!isRunning(snap, chat)) { say("実行中ではないため、追加指示としては送れません。"); return; }
      const sentAtts = e.attachments.map((id) => snap.attachments.find((x) => x.id === id)).filter((x): x is AttachmentEntry => !!x).map(fromLocal);
      const key = chat.key;
      try {
        // 項目の取消し・受理不明の保留は、ホストが行う（受理→項目を取り消す／受理不明→項目を残しキューを保留／受理なし→待機のまま）
        const a = await host.sendMessage(key, e.text, "steer", e.attachments, e.id);
        switch (a.state.kind) {
          case "accepted":
            setSends((m) => ({ ...m, [key.id]: [...(m[key.id] ?? []), { text: e.text, at: a.at, atts: sentAtts, steer: true }].slice(-30) }));
            say("追加指示を Codex が受理しました。元の依頼は取り消しました。現在のやりとりへの反映は会話の表示で確認してください（受理と反映は別です）。");
            break;
          case "acceptanceUnknown":
            setSteerUnknown((m) => ({ ...m, [e.id]: a.attemptId }));
            say("追加指示を受理したか確認できません。元の依頼は残し、自動送信を保留しています。再送はしません。履歴との照合で確定します（「履歴と照合」でも確認できます）。");
            break;
          case "rejected": say(`追加指示は受け付けられませんでした（${a.state.message}）。元の依頼はそのまま残しています。`); break;
          default: say("追加指示の結果を確認できませんでした。元の依頼はそのまま残しています。");
        }
      } catch (er) { sayErr("追加指示を送れませんでした。元の依頼はそのまま残しています", er); }
    },
    steerReconcile: (entryId: string) => {
      const attempt = steerUnknown[entryId];
      if (!chat || !live || !attempt) return;
      const key = chat.key;
      const clear = () => setSteerUnknown((m) => { const n = { ...m }; delete n[entryId]; return n; });
      // 照合は読み取りのみ。受理と確定したらホストが元の項目を取り消し、受理なしなら項目は待機に戻る。
      host.reconcileSend(key, attempt).then((a) => {
        if (a.state.kind === "accepted") {
          clear();
          say("履歴との照合で、追加指示の受理を確認しました。元の依頼は取り消しました。");
        } else if (a.state.kind === "acceptanceUnknown") say("まだ受理を確認できません。元の依頼は残しています（再送はしません）。");
        else { clear(); say("履歴に受理の痕跡がないことを確認しました。元の依頼はそのまま残しています。"); }
      }).catch((er) => sayErr("照合できませんでした", er));
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

  /** 作業フォルダのセットの保存と「参照…」（新しいチャットと作業フォルダの帯で共有）。保存は設定の直列更新に乗せ、保存できたときだけ true を返す。 */
  const folderTools: FolderTools = {
    sets: snap.settings.workFolderSets ?? [],
    update: async (f) => {
      if (!live) {
        const n = f(snap.settings.workFolderSets ?? []);
        if (!n) return false;
        updateSnap((s) => ({ ...s, settings: { ...s.settings, workFolderSets: n } }));
        return true;
      }
      let applied = false;
      const ok = await updateSettings((s) => { const n = f(s.workFolderSets ?? []); if (!n) return s; applied = true; return { ...s, workFolderSets: n }; }, "作業フォルダのセットを保存できませんでした");
      return ok && applied;
    },
    browse: async () => {
      if (!live) { say("モックでは動きません。"); return null; }
      try { return await host.pickFolder(); } catch (e) { sayErr("フォルダを選択できませんでした", e); return null; }
    },
  };

  /** 作業フォルダの確認・変更（ホストが検査し直す。強い確認は確認済みのパスだけ渡す）。モックでは動かさない。 */
  const wf: WorkFoldersApi | null = chat && live ? {
    tools: folderTools,
    load: () => host.getWorkFolders(chat.key),
    check: (path, current) => host.checkWorkFolder(path, current),
    save: async (folders, confirmedStrong, queueConfirmed): Promise<SaveResult> => {
      try {
        say(impactText("作業フォルダ", await host.setNextWorkFolders(chat.key, folders, confirmedStrong, queueConfirmed)));
        return { kind: "ok" };
      } catch (e) {
        const er = host.asIpcError(e);
        if (er.blocked?.kind === "queueRetargetUnconfirmed") return { kind: "needsQueueConfirm", waiting: er.blocked.waiting };
        return { kind: "error", message: er.message };
      }
    },
  } : null;

  const onRetrySend = async () => {
    if (!chat || !live) return;
    const a = attempts[chat.key.id];
    if (!a) return;
    try {
      const r = await host.retrySend(chat.key, a.attemptId);
      noteAttempt(chat.key.id, r);
      // 再送の結果を元の仮の行へ結び付ける（受理されたのに「未送信」と残さない）
      const id = chat.key.id;
      setPendingSends((m) => { const cur = m[id]; if (!cur) return m; const n = rebindPending(cur, a.attemptId, r); return n === cur ? m : { ...m, [id]: n }; });
    } catch (e) { sayErr("再送できませんでした", e); }
  };

  const onModel = (model: string, effort: string, speed: string) => {
    if (!chat || !live) return;
    host.setChatModel(chat.key, speed ? { model, effort: effort || null, speedTier: speed } : { model, effort: effort || null }).catch((e) => sayErr("モデルの選択を保存できませんでした", e));
  };

  /** 計画／実行の選択（次の送信から適用。受理済みは設定の更新通知を受けたときだけ表示される）。 */
  const onWorkMode = (mode: WorkMode | null) => {
    if (!chat || !live) return;
    host.setWorkMode(chat.key, mode).then((r) => say(mode === null ? "計画／実行の選択を外しました。" : impactText("計画／実行", r))).catch((e) => sayErr("計画／実行を変更できませんでした", e));
  };

  /** 目標の設定・変更。戻り値は表示する文（受け付けた事実だけ。結果は常設欄の更新で確認する）。 */
  const saveGoal = async (chatId: string, update: GoalUpdate): Promise<string> => {
    const c = snap.chats.find((x) => x.key.id === chatId);
    if (!c || !live) return "モックでは動きません。";
    setGoalBusy(true);
    try { return ackText(await host.setGoal(c.key, update), "目標の設定"); } finally { setGoalBusy(false); }
  };
  const clearGoal = () => {
    if (!chat || !live) return;
    const key = chat.key;
    setGoalBusy(true);
    host.clearGoal(key).then((a) => say(ackText(a, "目標の解除"))).catch((e) => sayErr("目標を解除できませんでした", e)).finally(() => setGoalBusy(false));
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
    const key = JSON.stringify(i);
    if (creatingKeys.current.has(key)) return; // 二重押し（同じ入力）では作らない
    creatingKeys.current.add(key);
    const tempId = `creating-${++creatingSeq.current}`;
    setCreating((l) => addCreating(l, { tempId, label: creatingLabel(i.firstMessage, i.cwd), startedAt: Date.now(), phase: i.isolate && i.cwd ? "worktree" : "starting", input: i, result: null, body: i.firstMessage }));
    // 検査は済んでいる。以後は一覧の仮行で示す（閉じても作成は続く。取り消せる操作には見せない）。
    setDialog(null);
    const m = models.find((x) => x.id === i.model);
    let retryInput: NewChatInput = i;
    try {
      let cwd = i.cwd;
      let worktree = i.worktree;
      if (i.isolate && i.cwd) {
        // 「分離」を選んだときだけ、worktreeを作ってから開始する（作れなければチャットは作らない。作られた分は自動削除しない）。
        const rec = await host.createWorktree(i.cwd, null, null, i.firstMessage?.split(/\r?\n/).find((l) => l.trim())?.trim().slice(0, 40) ?? null);
        if (rec.state.kind === "failed") {
          setCreating((l) => resolveCreating(l, tempId, { kind: "failed", message: `worktreeを作成できませんでした: ${rec.state.kind === "failed" ? rec.state.message : ""}（メニュー「作業 → worktree を管理」で確認できます）` }));
          return;
        }
        if (rec.state.kind !== "ready") {
          setCreating((l) => resolveCreating(l, tempId, { kind: "unknown", message: "worktreeの作成を確認できません。メニュー「作業 → worktree を管理」で確認してください。" }));
          return;
        }
        cwd = rec.path;
        worktree = rec.id;
        // 入力し直すときは、作られたworktreeを使う（もう一度作らない）。
        retryInput = { ...i, cwd: rec.path, worktree: rec.id, isolate: false };
        say(`worktreeを作成しました（ブランチ ${rec.branch}）。`);
        setCreating((l) => setCreatingPhase(l, tempId, "starting"));
      }
      const r = await host.startChat(cwd, i.model ? { model: i.model, effort: m?.defaultEffort ?? null } : null, i.firstMessage, null, worktree, i.extraFolders, i.confirmedStrong);
      setCreating((l) => removeCreating(l, tempId));
      setSelId(r.chat.key.id);
      // 作成に成功したときだけ、モデルを次のクイック新規へ引き継ぐ（作業フォルダ・権限は持ち越さない）。
      if (i.model) void updateSettings((s) => ({ ...s, lastNewChat: { model: { model: i.model, effort: m?.defaultEffort ?? null } } }), "前回のモデルを保存できませんでした");
      // 戻り値は送信中（Sending）。確定が先に届いていれば mergeAttempt が確定を残す。
      if (r.firstSend) {
        const chatId = r.chat.key.id;
        noteAttempt(chatId, r.firstSend);
        // 最初の依頼を「送信中」の行として出す。確定が先に届いていればその結果で出す。確定までは入力欄を無効にする。
        const cur = attemptsRef.current[chatId] ?? r.firstSend;
        if (i.firstMessage) {
          const row: PendingSend = { id: `ps${++pendingSeq.current}`, text: i.firstMessage, startedAt: Date.now(), steer: false, phase: null, outcome: outcomeOfAttempt(cur), attemptId: cur.attemptId, first: true };
          setPendingSends((m) => ({ ...m, [chatId]: [...(m[chatId] ?? []), row] }));
        }
      }
    } catch (e) {
      const er = host.asIpcError(e);
      const res = resultOfError(er.code, er.message);
      setCreating((l) => resolveCreating(l, tempId, res.kind === "failed" ? { kind: "failed", message: `チャットを作成できませんでした: ${er.message}` } : { kind: "unknown", message: er.message }));
      // 入力し直しの値だけ差し替える（worktreeを作った後の失敗）。
      if (retryInput !== i) setCreating((l) => l.map((c) => (c.tempId === tempId ? { ...c, input: retryInput } : c)));
    } finally { creatingKeys.current.delete(key); }
  };
  /** 仮行の操作。「入力し直す」は同じ値でダイアログを開くだけ（送らない）。「一覧を更新」は読み取りのみ。 */
  /** クイック新規の送信: 一般チャット・worktreeなし・権限は初期値で作成する（最初の依頼とともに）。 */
  const quickSend = () => {
    const text = quick.text;
    if (!text.trim() || quick.busy) return;
    const model = quick.model ?? pickQuickModel(models, snap.settings.lastNewChat, snap.settings.defaultModel);
    // 作成が終わる（新チャットへ移る、または一覧の仮行に失敗が出る）まで下書き画面に「作成中」を残し、前の会話へ戻さない。
    setQuick((q) => ({ ...q, busy: true }));
    void createChat({ cwd: null, model, firstMessage: text, isolate: false, worktree: null, extraFolders: [], confirmedStrong: [] })
      .finally(() => setQuick((q) => (q.text === text ? { open: false, text: "", model: null } : { ...q, busy: false })));
  };
  const creatingAct = (id: string, op: "retry" | "dismiss" | "refresh") => {
    const c = creating.find((x) => x.tempId === id);
    if (!c) return;
    if (op === "refresh") {
      void refreshList().then((ok) => {
        if (!ok) return;
        // 見つかったか（作成開始以降のチャット）を示す。見つからなくても「作られていない」とは断定しない。
        const n = foundSince(lastListItems.current, c.startedAt, c.input.cwd);
        if (n > 0) { setCreating((l) => removeCreating(l, id)); say(`一覧に作成されたチャットが見つかりました（${n}件）。`); }
        else setCreating((l) => setCheckedNote(l, id, "一覧に見つかりませんでした。作られていないとは断定できません（反映が遅れている場合があります）。必要なら入力し直してください（自動では送りません）。"));
      });
      return;
    }
    setCreating((l) => removeCreating(l, id));
    if (op === "retry") {
      const i = c.input;
      setDialog({ type: "newChat", dev: i.cwd !== null, cwd: i.cwd ?? undefined, initialFolders: i.cwd ? [i.cwd, ...i.extraFolders] : undefined, prefill: { model: i.model, first: i.firstMessage ?? "", isolate: i.isolate, worktree: i.worktree } });
    }
  };

  const retryLaunch = (path: string) => {
    const p = path.trim();
    setExePath(p); writeExe(p);
    void boot(p || null);
  };

  // ── 添付・成果物（§3.8）。コピー・実在確認・上書き判定はホスト。ここは操作の入口と結果の案内だけ。 ──
  const addFiles = async (c: ChatKey, paths: string[]) => {
    for (const path of paths) {
      try {
        const e = await host.addAttachmentFile(c, path);
        if (e.state.kind === "copyFailed") say(`コピーできませんでした（${e.displayName}）: ${e.state.message}`);
      } catch (er) { sayErr("添付できませんでした", er); }
    }
  };
  dropRef.current = (paths) => {
    if (!chat) { say("チャットを選んでからドロップしてください。"); return; }
    void addFiles(chat.key, paths);
  };
  // ファイルのドロップ（Tauriのドラッグ＆ドロップ。パスを受け取り、ホストがチャット領域へコピーする）。
  useEffect(() => {
    if (!live) return;
    let alive = true;
    let un: (() => void) | null = null;
    getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === "enter" || p.type === "over") setDragging(true);
      else if (p.type === "leave") setDragging(false);
      else { setDragging(false); dropRef.current(p.paths); }
    }).then((u) => { if (alive) un = u; else u(); }).catch(() => { /* ドロップが使えなくても、選択・貼り付けで添付できる */ });
    return () => { alive = false; un?.(); };
  }, [live]);

  const pickFiles = async () => {
    setDialog(null);
    if (!live || !chat) { say("この操作はモックでは動きません。"); return; }
    try {
      const r = await openDialog({ multiple: true, directory: false, title: "添付するファイルを選ぶ" });
      if (r) await addFiles(chat.key, Array.isArray(r) ? r : [r]);
    } catch (e) { sayErr("ファイルを選べませんでした", e); }
  };

  /** クリップボードの文章を、コード枠付きで入力欄のカーソル位置へ入れる（ホストは関与しない）。 */
  const pasteSelection = async () => {
    setDialog(null);
    if (!chat) return;
    let text: string | null = null;
    try { text = await readText(); } catch { text = null; }
    if (!text) { say("クリップボードに文章がありません。"); return; }
    const pos = composerRef.current?.selectionStart ?? curDraft.length;
    const r = insertBlock(curDraft, pos, fenceCode(text));
    onDraft(chat, r.text);
    window.requestAnimationFrame(() => { const el = composerRef.current; if (el) { el.focus(); el.setSelectionRange(r.cursor, r.cursor); } });
  };

  /** 文章を入力欄のカーソル位置へ入れる（枠は付けない。送信しない）。参考指定・指示の依頼文・side相談の引渡しで使う。 */
  const insertIntoDraft = (text: string) => {
    if (!chat) return;
    const pos = composerRef.current?.selectionStart ?? curDraft.length;
    const r = insertBlock(curDraft, pos, text.replace(/\n$/, ""));
    onDraft(chat, r.text);
    window.requestAnimationFrame(() => { const el = composerRef.current; if (el) { el.focus(); el.setSelectionRange(r.cursor, r.cursor); } });
  };

  /** Skillを入力欄の指定（チップ）にする。コピーせず、送信時に明示的に呼び出す。 */
  const pickSkill = async (c: ChatKey, name: string, path: string): Promise<string> => {
    if (!live) { say("この操作はモックでは動きません。"); return "モックでは動きません"; }
    let msg: string;
    try {
      const e = await host.addSkillAttachment(c, name, path);
      msg = `Skill「${e.displayName}」を指定しました（入力欄のチップ。送信時に明示的に呼び出します）。`;
      // 最近使ったSkill（チャット共通・最大5件）。指定できたものだけ記録する。
      updateSettings((st) => ({ ...st, recentSkills: pushRecent(st.recentSkills ?? [], { name, path }) }), "最近使ったSkillを保存できませんでした");
    } catch (er) { msg = `Skillを指定できませんでした: ${host.asIpcError(er).message}`; }
    say(msg);
    return msg;
  };

  // 入力欄の `$` 候補用の、チャット別の取得済みSkill一覧。無ければ初回の `$` 入力時に読取りで取得する（resumeしない。失敗しても繰り返さない）。
  const [skillLists, setSkillLists] = useState<Record<string, SkillInfo[]>>({});
  const skillTried = useRef(new Set<string>());
  const noteSkills = useCallback((chatId: string, skills: SkillInfo[]) => { skillTried.current.add(chatId); setSkillLists((m) => ({ ...m, [chatId]: skills })); }, []);
  const loadSkills = (c: ChatKey) => {
    if (!live || skillTried.current.has(c.id)) return;
    skillTried.current.add(c.id);
    host.listSkills(c, false).then((l) => noteSkills(c.id, l.skills)).catch(() => { /* 取れなければ候補を出さない（通常の入力を妨げない） */ });
  };

  /** side相談を開く（読み取り専用の一時的な分岐）。開けたか不明のときは再送せず、状況を知らせる。 */
  const openSide = async () => {
    setDialog(null);
    if (!chat) { say("チャットを選んでください。"); return; }
    if (!live) { say("この操作はモックでは動きません。"); return; }
    try {
      const r = await host.openSide(chat.key);
      if (r.ack.kind === "accepted") {
        if (r.side) { setSideId(r.side.id); say("side相談を開きました（読み取り専用の一時的な相談）。"); }
        else say("side相談を開けたか確認できません。再送はしていません。必要なら開き直してください。");
      } else if (r.ack.kind === "rejected") say(`side相談を開けませんでした: ${r.ack.message}`);
      else say(`side相談を開けたか確認できません（${r.ack.message}）。再送はしていません。必要なら開き直してください。`);
    } catch (e) { sayErr("side相談を開けませんでした", e); }
  };

  const files: FileActions = {
    open: (t) => {
      if (!live) { say("この操作はモックでは動きません。"); return; }
      void (async () => {
        try { await host.openFile(t); return; } catch (e) {
          const er = host.asIpcError(e);
          const b = er.blocked;
          if (b?.kind !== "openNeedsConfirm") { sayErr("開けませんでした", e); return; }
          if (!(await askConfirm(openFileConfirm({ executable: !!b.executable, outsideProject: !!b.outsideProject, path: b.path })))) { say("開くのをやめました。"); return; }
          try { await host.openFile(t, true); } catch (e2) { sayErr("開けませんでした", e2); }
        }
      })();
    },
    save: (t, name) => {
      if (!live) { say("この操作はモックでは動きません。"); return; }
      void (async () => {
        const dest = await saveDialog({ title: "名前を付けて保存", defaultPath: name }).catch(() => null);
        if (!dest) return;
        try { await host.saveFileAs(t, dest, false); say("保存しました。"); return; } catch (e) {
          const er = host.asIpcError(e);
          if (er.blocked?.kind !== "targetExists") { sayErr("保存できませんでした", e); return; }
        }
        // 同名のファイルがある。確認のうえでだけ上書きする。
        if (!window.confirm(`同じ名前のファイルがあります。上書きしますか？\n${dest}`)) { say("保存をやめました（既存のファイルは変更していません）。"); return; }
        try { await host.saveFileAs(t, dest, true); say("保存しました。"); } catch (e) { sayErr("保存できませんでした", e); }
      })();
    },
    preview: (t) => (live ? host.readFilePreview(t) : Promise.reject(new Error("モックではプレビューしません"))),
    remove: (id) => {
      if (!chat || !live) return;
      host.removeAttachment(chat.key, id).catch((e) => sayErr("取り外せませんでした", e));
    },
    pasteImage: (name, bytes) => {
      if (!chat || !live) { say("この操作はモックでは動きません。"); return; }
      host.addAttachmentImageBytes(chat.key, name, bytes)
        .then((e) => { if (e.state.kind === "copyFailed") say(`コピーできませんでした（${e.displayName}）: ${e.state.message}`); })
        .catch((e) => sayErr("画像を添付できませんでした", e));
    },
  };

  // ── 会話内のチャットID・ファイルパス（項目5・24・30）。リンク・メニューはユーザーのクリックだけで動く。 ──
  const { pathInfo, requestPath } = usePathStats(live, chat?.key ?? null);
  const pathAct = (act: PathAction, path: string, info: PathInfo, strict = false) => {
    if (!chat || !live || !info.absolute) { say("この操作はモックでは動きません。"); return; }
    const abs = info.absolute;
    const copy = (text: string, what: string) => { void copyText(text).then((ok) => say(ok ? `${what}をコピーしました。` : `${what}をコピーできませんでした。`)); };
    switch (act) {
      case "attach": void addFiles(chat.key, [abs]); break;
      case "copyAbs": copy(abs, "絶対パス"); break;
      case "reveal": host.revealPath(chat.key, path, strict).catch((e) => sayErr("表示できませんでした", e)); break;
      case "open":
        if (strict) host.openPath(chat.key, path, false, true).catch((e) => sayErr("開けませんでした", e));
        else openWithConfirm(chat.key, path);
        break;
    }
  };
  /** 開く（作業フォルダ外・実行形式はホストが止めるので、確認のうえで再実行）。ユーザーのクリックから呼ぶ。 */
  const openWithConfirm = (chatKey: ChatKey, path: string) => {
    void (async () => {
      try { await host.openPath(chatKey, path); return; } catch (e) {
        const b = host.asIpcError(e).blocked;
        if (b?.kind !== "openNeedsConfirm") { sayErr("開けませんでした", e); return; }
        if (!(await askConfirm(openFileConfirm({ executable: !!b.executable, outsideProject: !!b.outsideProject, path: b.path })))) { say("開くのをやめました。"); return; }
        try { await host.openPath(chatKey, path, true); } catch (e2) { sayErr("開けませんでした", e2); }
      }
    })();
  };
  // ヘッダーの作業フォルダの実在確認（読取りのメタデータだけ。開くのは押したときだけ）
  const headCwd = chat ? knownValue(chat.cwd) : null;
  useEffect(() => { if (live && headCwd) requestPath(headCwd); }, [live, headCwd, chat?.key.id, requestPath]);

  // メニュー・ヘッダー・Ctrl+K が使う可否（台帳から。✓ と「作業フォルダを開けない理由」もここで渡す）。
  const checked = {
    toggleLeft: leftTemp ? lNarrow : leftOpen, toggleRight: rightTemp ? rNarrow : rightOpen, toggleMonitor: live ? monitorOpen : mini,
    toggleDone: scope.kind === "allChats" && scope.showFinished, toggleMainTop: mainTop, toggleMiniTop: miniTop,
    workMode: !!chat && bundle.modelSettings[chat.key.id]?.workMode === "plan",
  };
  const cmdViews = commandViews(snap, chat, connected, !!gitCwd && gitKinds[gitCwd] === "notRepo", changesMark ? { changeList: "記録できません" } : {},
    { checked, folderCount: snap.chatLocals.find((l) => l.chat.id === chat?.key.id)?.nextFolders?.folders.length ?? 0, cwdWhy: cwdOpenWhy(headCwd, headCwd ? pathInfo(headCwd)?.kind : undefined, live) });
  // 監視窓の開閉の通知（ホストの開閉・×・トレイすべて）。実接続のみ。
  useEffect(() => {
    if (!live) return;
    let un: (() => void) | undefined;
    let alive = true;
    host.subscribeMonitorWindow(setMonitorOpen).then((u) => { if (alive) un = u; else u(); }).catch(() => undefined);
    return () => { alive = false; un?.(); };
  }, [live]);
  // 「表示」メニューを開いたとき1回だけ監視窓の開閉を確認する（失敗は不明＝✓なし）。
  const setMenuOpen = (m: string | null) => {
    setMenu(m);
    if (m === "表示" && live) host.monitorWindowOpen().then(setMonitorOpen).catch(() => setMonitorOpen(false));
  };
  const refActions: RefActions = {
    chatTitle: (id) => { const c = snap.chats.find((x) => x.key.id.toLowerCase() === id.toLowerCase()); return c ? chatName(c) : undefined; },
    openChat: (id) => { const c = snap.chats.find((x) => x.key.id.toLowerCase() === id.toLowerCase()); if (c) selectChat(c.key.id); },
    refreshList: async () => { await refreshList(); },
    pathInfo, requestPath, pathAct,
    canAttach: !!chat && live,
    cwd: headCwd,
    openChanges: chat ? () => setDialog({ type: "changes", chatId: chat.key.id }) : undefined,
    insertPrompt: chat ? (text) => {
      const cur = drafts[chat.key.id] ?? "";
      if (cur.trim() !== "" && !window.confirm("入力欄に書きかけの文があります。この文に置き換えますか？（送信はしません）")) return;
      onDraft(chat, text);
    } : undefined,
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

  // ── 編集して再送・再生成（M50）。分岐・下書き・送信の順序と最終判定はホスト。ここは結果の反映だけ ──
  const resendHandlers: ResendHandlers = {
    forked: (key, text, attempt) => {
      selectChat(key.id);
      if (text !== null) setDrafts((d) => ({ ...d, [key.id]: text }));
      if (attempt) noteAttempt(key.id, attempt);
    },
    // 最初のturn: 新しいチャットを作って下書きに入れるだけ（送らない）。
    startFirst: async (text, existing, carry) => {
      if (!live) throw new Error("モックでは新しいチャットを作りません");
      let key = existing;
      const src = dialog?.type === "resend" ? snap.chats.find((c) => c.key.id === dialog.chatId) : undefined;
      if (!key) {
        if (!src) throw new Error("元のチャットを確認できません");
        if (src.cwd.kind !== "value" && src.kind === "development") throw new Error("元の作業フォルダを確認できないため、新しいチャットを作れません");
        const local = snap.chatLocals.find((l) => l.chat.id === src.key.id);
        const model = bundle.modelSettings[src.key.id]?.selected ?? local?.model ?? null;
        // 元が一般チャットなら新しい一般チャット（cwd なし。管理領域のパスは使わない）。開発チャットは同じフォルダ。
        // 強い確認の対象のフォルダ（ドライブ直下・ホーム直下など）はホストが止め、理由を出す（確認UIは足さない）。
        const cwd = src.kind === "general" ? null : src.cwd.kind === "value" ? src.cwd.value : null;
        // AgentDock が作った worktree の場所なら、新しいチャットも同じ記録に結び付ける（削除確認に出るように）。
        const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
        const wt = cwd ? snap.worktrees.find((w) => norm(w.path) === norm(cwd))?.id ?? null : null;
        const r = await host.startChat(cwd, model, null, local?.permission ?? null, wt);
        key = r.chat.key;
      }
      // 作った後の保存の失敗は例外にしない（押し直しで空のチャットが増えないよう、呼び出し側は作ったチャットを使い回す）。
      setSelId(key.id); setQuick((q) => (q.open ? { ...q, open: false } : q));
      setDrafts((d) => ({ ...d, [key.id]: text }));
      // 元の依頼の添付を、新しいチャットの送信前の添付へ新しいコピーとして付ける（送らない）。元の台帳のIDだけをホストへ渡し、ホストが台帳からコピーする。
      // 失敗は黙って落とさず知らせる。付け終えた分は返し、押し直しで二重に付けない。
      const carryOver = async (): Promise<{ carried: string[]; carryFailed: string | null }> => {
        const carried: string[] = [], fails: string[] = [];
        for (const l of carry) {
          try {
            if (!src || !key) throw new Error("元のチャットを確認できません");
            await host.carryAttachment(src.key, key, l.id); carried.push(l.id);
          } catch (e) { fails.push(`${l.displayName}: ${host.asIpcError(e).message}`); }
        }
        return { carried, carryFailed: fails.length ? fails.join("、") : null };
      };
      try {
        await host.setDraft(key, text);
        return { key, draftSaved: true, message: null, ...(await carryOver()) };
      } catch (e) {
        return { key, draftSaved: false, message: host.asIpcError(e).message, ...(await carryOver()) };
      }
    },
    saveDraft: async (key, text) => {
      if (!live) return "saved";
      // 分岐先に利用者の別の下書きがあれば上書きしない。
      const existing = snap.chatLocals.find((l) => l.chat.id === key.id)?.draft.text ?? "";
      if (existing.trim() !== "" && existing !== text) return "kept";
      await host.setDraft(key, text);
      return "saved";
    },
  };
  let resendState: ResendState | null = null;
  if (dialog?.type === "resend") {
    const target = snap.chats.find((c) => c.key.id === dialog.chatId);
    if (target) {
      const turns = bundle.turns[target.key.id] ?? [];
      const t = resendTargetOf(rootTurns(snap, target, turns), dialog.turnId, dialog.purpose, dialog.itemId);
      const ctx = resendCtxOf(snap, target, connected, turns);
      resendState = { plan: resendAvailability(ctx, t), purpose: dialog.purpose, requestText: t.requestText, attachments: t.attachments ?? [], locals: snap.attachments.filter((a) => keyStr(a.chat) === keyStr(target.key)) };
    }
  }

  // ── 削除・アーカイブ・エクスポート・使用量（P7） ──

  /** 削除確認を開く（対象と削除しないものを先に示す。実行はしない）。 */
  const openDelete = (chatId: string | undefined) => {
    const target = snap.chats.find((x) => x.key.id === chatId);
    if (!target) return;
    if (!live) { say("この操作はモックでは動きません。"); return; }
    setDel({ preview: null, error: null, running: false, outcome: null });
    setDialog({ type: "delete", chatId: target.key.id });
    host.previewDelete(target.key)
      .then((p) => setDel((s) => ({ ...s, preview: p })))
      .catch((e) => setDel((s) => ({ ...s, error: host.asIpcError(e).message })));
  };

  /** 削除の実行（確認画面の「削除」だけが呼ぶ）。完了と言うのは deleted を確認できたときだけ。保留・部分失敗は画面に残して理由を示す。 */
  const runDelete = async () => {
    if (dialog?.type !== "delete") return;
    const c = snap.chats.find((x) => x.key.id === dialog.chatId);
    if (!c) return;
    const external = del.preview?.deletesBackendHistory === false;
    setDel((s) => ({ ...s, running: true, error: null, outcome: null }));
    try {
      const out = await host.deleteChat(c.key);
      if (out.kind === "deleted") {
        setDialog((cur) => (cur?.type === "delete" ? null : cur)); // 閉じて他の画面を開いていたら、それは閉じない
        say(external ? "一覧から外しました。元の履歴は削除していません。" : "削除しました。");
        if (selRef.current === c.key.id) setSelId(null);
        return;
      }
      setDel((s) => ({ ...s, running: false, outcome: out }));
      say("削除は完了していません。もう一度「削除」を開くと、残りを続けられます。", 7000); // 画面を閉じていても分かるように
    } catch (e) { setDel((s) => ({ ...s, running: false, error: host.asIpcError(e).message })); say(`削除できませんでした: ${host.asIpcError(e).message.split("\n")[0]}`, 7000); }
  };

  const openExport = (chatId: string | undefined) => {
    const target = snap.chats.find((x) => x.key.id === chatId);
    if (!target) return;
    if (!live) { say("この操作はモックでは動きません。"); return; }
    setExp({ include: false, running: false, error: null });
    setDialog({ type: "export", chatId: target.key.id });
  };

  /** 保存先を選んで書き出す。書き込みの成功を確認できたときだけ完了と言う。 */
  const runExport = async () => {
    if (dialog?.type !== "export") return;
    const c = snap.chats.find((x) => x.key.id === dialog.chatId);
    if (!c) return;
    setExp((s) => ({ ...s, running: true, error: null }));
    try {
      const base = chatName(c).replace(/[\\/:*?"<>|]/g, "_").trim() || "chat";
      const dest = await host.pickSaveFile(`${base}.md`);
      if (!dest) { setExp((s) => ({ ...s, running: false })); return; }
      // 同名ファイルの上書きは、保存ダイアログ（OS）で確認済み。
      await host.exportMarkdown(c.key, exp.include, dest, true);
      setDialog(null);
      say(`書き出しました: ${dest}`);
    } catch (e) { setExp((s) => ({ ...s, running: false, error: host.asIpcError(e).message })); }
  };

  const archiveOp = (on: boolean, chatId: string | undefined) => {
    const target = snap.chats.find((x) => x.key.id === chatId);
    if (!target) return;
    if (!live) { say("この操作はモックでは動きません。"); return; }
    if (on) {
      const working = isRunning(snap, target);
      host.archiveChat(target.key)
        .then(() => say(archivedToast(working), 7000))
        .catch((e) => sayErr("アーカイブできませんでした", e));
    } else {
      host.unarchiveChat(target.key).then(() => say("アーカイブを解除しました。")).catch((e) => sayErr("アーカイブを解除できませんでした", e));
    }
  };

  const loadUsage = useCallback(() => {
    if (!live) return;
    setUsage((u) => ({ ...u, loading: true, error: null }));
    host.getUsage(null)
      .then((r) => setUsage({ report: r, error: null, loading: false }))
      .catch((e) => setUsage((u) => ({ ...u, loading: false, error: host.asIpcError(e).message })));
  }, [live]);
  const storageOpen = dialog?.type === "settings" && dialog.tab === "storage";
  useEffect(() => { if (storageOpen) loadUsage(); }, [storageOpen, loadUsage]);

  /** codex.exe の「参照…」。選んだパスを入力欄と設定へ入れる（接続は「この場所で接続」で別に行う）。 */
  const browseExe = () => {
    host.pickCodexExecutable().then((p) => {
      if (!p) return;
      setExePath(p); writeExe(p);
      updateSettings((s) => ({ ...s, executables: { ...s.executables, codex: p } }), "codex.exe の場所を保存できませんでした");
    }).catch((e) => sayErr("ファイルを選択できませんでした", e));
  };

  /** ピン留めの切替（ホストが chat.json へ保存する。再起動後も戻る）。 */
  const togglePin = (chatId: string | undefined) => {
    const target = snap.chats.find((x) => x.key.id === chatId);
    if (!target) return;
    const on = !target.pinned;
    if (!live) { updateSnap((s) => ({ ...s, chats: s.chats.map((c) => (c.key.id === target.key.id ? { ...c, pinned: on } : c)) })); return; }
    host.setPinned(target.key, on).then(() => say(on ? "ピン留めしました。" : "ピン留めを外しました。")).catch((e) => sayErr("ピン留めを変更できませんでした", e));
  };

  /** 変更の控えの記録のオン／オフ（設定へ保存。すでに取った控えは消えない）。 */
  const setBaselinesEnabled = (on: boolean) => {
    updateSettings((s) => ({ ...s, baselines: { ...s.baselines, enabled: on } }), "控えの記録の設定を変更できませんでした");
  };
  /** 所有者違いのリポジトリの選択（記録する／しない／AgentDockのgit操作にだけ許可）。保存して `settingsUpdated` で画面へ戻る。 */
  const setRepoBaseline = async (repo: string, choice: RepoRecordChoice | null, allowGit: boolean) => {
    if (!live) { say("モックでは動きません。"); return; }
    try { await host.setRepoBaseline(repo, choice, allowGit); } catch (e) { sayErr("リポジトリの記録の設定を保存できませんでした", e); }
  };
  /** 選択中チャットの控えを削除する（確認ダイアログの後だけ呼ばれる。ホストが作業中なら拒否する）。 */
  const deleteChatBaselines = async () => {
    if (!live || !chat) return;
    try { await host.deleteBaselines(chat.key); say("このチャットの控えを削除しました。"); loadUsage(); } catch (e) { sayErr("控えを削除できませんでした", e); }
  };

  /** 起動時の保存データ警告を閉じる。確認済みとして設定へ保存し、以後は（状況が変わるまで）出さない。保存できなければ次回また出る。 */
  const dismissWarnings = () => {
    setWarnHidden(true);
    // 確認済みにできるのは領域・ファイルに紐づく警告だけ。保存できない状態の警告は、この起動中だけ隠す（ホストも保存しない）。
    if (!live || !snap.startupWarnings.some((w) => w.acknowledgeable)) return;
    host.acknowledgeWarnings(false).catch((e) => sayErr("警告を確認済みとして保存できませんでした（次回起動時にもう一度表示されます）", e));
  };
  /** 確認済みにした警告を解除する（次回起動から再び表示）。 */
  const resetWarnings = async () => {
    if (!live) { say("この操作はモックでは動きません。"); return; }
    try { await host.acknowledgeWarnings(true); say("確認済みの警告を解除しました。次回起動から、該当する警告が再び表示されます。"); } catch (e) { sayErr("確認済みの警告を解除できませんでした", e); }
  };

  /** チャット一覧を取り直す（読み取りのみ。再開・送信はしない）。 */
  const refreshList = async (skipProjects = false): Promise<boolean> => {
    if (!live) { say("この操作はモックでは動きません。"); return false; }
    const at = hms(Date.now());
    try {
      const page = await host.listChats();
      lastListItems.current = page.items.map((s) => s.chat);
      setListCursor(page.nextCursor);
      if (!skipProjects) void refreshProjects();
      const text = `一覧を更新しました（Codex から${page.items.length}件${page.nextCursor ? "、続きあり" : ""}）`;
      setListStatus({ ok: true, text: `${text}　${at}` });
      if (!skipProjects) say(`${text}。`);
      return true;
    } catch (e) {
      const msg = host.asIpcError(e).message;
      setListStatus({ ok: false, text: `一覧を更新できませんでした: ${msg}　${at}` });
      sayErr("一覧を更新できませんでした", e);
      return false;
    }
  };
  /** プロジェクト一覧を更新して振り分け直す（project/list の再取得だけ。チャット一覧は取らない。押したときだけ）。取れなかったときは前の振り分けを残す。戻り値は失敗の理由（成功は null）。 */
  const reclassify = async (): Promise<string | null> => {
    if (!PROJECTS_ENABLED) return null; // 一時停止中は通信しない
    if (!live) return "この操作はモックでは動きません。";
    try {
      const o = await host.listProjects();
      if (o.kind !== "loaded") return `プロジェクトを取得できませんでした（前の振り分けのままです）: ${o.reason}`;
      setProjects(o);
    } catch (e) { return `プロジェクトを取得できませんでした（前の振り分けのままです）: ${host.asIpcError(e).message}`; }
    return null;
  };
  /** 一覧末尾の「続きを読み込む」。次の1ページだけ取得してマージする（読み取りのみ）。失敗時は続きの位置を保つ。 */
  const loadMoreChats = async () => {
    if (!live || !listCursor) return;
    const known = new Set(snap.chats.map((c) => c.key.id));
    try {
      const page = await host.listChats(listCursor);
      setListCursor(page.nextCursor);
      const n = countFetched(page.items.map((s) => s.chat.key.id), known);
      say(loadMoreMessage({ ...n, more: page.nextCursor !== null }));
    } catch (e) { sayErr("続きを取得できませんでした（取得済みの一覧はそのままです）", e); }
  };
  /** プロジェクト見出しの「このプロジェクトのチャットを取得」。roots を cwd 条件にして、続きも上限ページまで取得する（読み取りのみ）。 */
  const fetchProjectChats = async (label: string, roots: string[]) => {
    if (!PROJECTS_ENABLED) return; // 一時停止中（見出しメニューも出さない）
    if (!live) { say("この操作はモックでは動きません。"); return; }
    const known = new Set(snap.chats.map((c) => c.key.id));
    const r = await fetchAllPages((cursor) => host.listChats(cursor, false, roots), MAX_FETCH_PAGES, (e) => host.asIpcError(e).message);
    const n = countFetched(r.items.map((s) => s.chat.key.id), known);
    say(projectFetchMessage(label, { ...n, truncated: r.truncated, error: r.error }), r.error || r.truncated ? 8000 : 5000);
  };

  /** 名前の変更。Codex が受け付けたと確認できたときだけダイアログを閉じる。失敗の理由は返してダイアログに残す。 */
  const renameChat = async (chatId: string, name: string): Promise<string | null> => {
    if (!live) return "モックでは動きません。";
    const c = snap.chats.find((x) => x.key.id === chatId);
    if (!c) return "チャットが見つかりません。";
    try {
      const out = await host.renameChat(c.key, name);
      if (out.kind !== "done") return "名前の変更を確認できませんでした。";
      setDialog(null); say("名前を変更しました。"); return null;
    } catch (e) { return host.asIpcError(e).message; }
  };

  /** 左一覧の行の「…」メニュー。対象はその行のチャット（選択中とは限らない。選択と最近利用は変えない）。 */
  const chatAct = (op: ChatOp, chatId: string) => {
    switch (op) {
      case "rename": setDialog({ type: "rename", chatId }); break;
      case "pin": togglePin(chatId); break;
      case "archive": archiveOp(true, chatId); break;
      case "unarchive": archiveOp(false, chatId); break;
      case "export": openExport(chatId); break;
      case "delete": openDelete(chatId); break;
      case "sameFolderNew": void sameFolderNew(chatId); break;
    }
  };

  /** 「同じ作業フォルダで新しいチャット…」。そのチャットの作業フォルダ一式を初期値に、新しいチャットのダイアログを開くだけ（送信は引き継がない。検査はダイアログが行う）。 */
  const sameFolderNew = async (chatId: string | undefined) => {
    const c = snap.chats.find((x) => x.key.id === chatId);
    if (!c) { say("チャットを選んでください。"); return; }
    if (c.kind === "general") { say("一般チャット（作業フォルダなし）では使えません。"); return; }
    let view: WorkFoldersView | null = null;
    if (live) { try { view = await host.getWorkFolders(c.key); } catch { view = null; } }
    const init = sameFolderInit(view, knownValue(c.cwd));
    if (!init) { say("作業フォルダを取得できていません。"); return; }
    setDialog({ type: "newChat", dev: true, cwd: init.folders[0], initialFolders: init.folders, folderNote: init.extrasUnconfirmed ? SAME_FOLDER_UNCONFIRMED_NOTE : undefined });
  };

  const act = (a: string) => {
    if (a.startsWith("unv:")) { setDialog({ type: "unv", why: a.slice(4) }); return; }
    if (a.startsWith("resend:")) {
      // 編集して再送・再生成。押したときはダイアログを開くだけ（無効なら理由だけを示す。送らない）。
      const [purpose, turnId, itemId] = a.slice(7).split("|");
      if (chat && turnId && (purpose === "editResend" || purpose === "regenerate")) setDialog({ type: "resend", chatId: chat.key.id, purpose, turnId, itemId: itemId || null });
      return;
    }
    if (a.startsWith("gitPush:")) { setDialog({ type: "gitPush", folder: a.slice(8) }); return; }
    // 組織実行の成果物: `orgArtifacts:<workflowId>[:<task>]`（見るだけ）、`orgImport:<workflowId>:<task,task…>`（確認ダイアログを開くだけ。取り込むのは確認ダイアログの主操作）。
    if (a.startsWith("orgArtifacts:")) { const [wid, task] = a.slice(13).split(":"); setDialog({ type: "orgArtifacts", workflowId: wid, task: task ?? null }); return; }
    if (a.startsWith("orgImport:")) { const [wid, tasks] = a.slice(10).split(":"); setDialog({ type: "orgImport", workflowId: wid, tasks: (tasks ?? "").split(",").filter((x) => x) }); return; }
    if (a.startsWith("fork:")) { if (chat) setDialog({ type: "fork", chatId: chat.key.id, throughTurn: a.slice(5) }); return; }
    if (a.startsWith("settings:")) { setDialog({ type: "settings", tab: a.split(":")[1] }); return; }
    switch (a) {
      case "noop": break;
      case "closeToTray":
        // 通常画面の閉じる＝トレイへ格納（確認なし。作業・収集・保存は続く）。終了は「AgentDock を終了」。
        if (live) host.closeThisWindow().catch((e) => sayErr("窓を閉じられませんでした", e)); else say("モックではトレイへ格納しません。");
        break;
      case "orgStart": setLNarrow(false); setDialog({ type: "orgStart" }); break;
      case "orgIntakeFromChat":
        if (!chat) { say("チャットを選んでください。"); break; }
        if (!live) { say("この操作はモックでは動きません。"); break; }
        host.orgIntakeAttach(chat.key).then(() => say("このチャットを、組織実行の依頼整理にしました（組織実行はまだ始まっていません）。入力欄の上の帯から進めます。", 8000)).catch((e) => sayErr("依頼整理にできませんでした", e));
        break;
      case "newChatQuick":
        if (!live) { say("この操作はモックでは動きません。"); break; }
        setOrgScreen(false);
        setLNarrow(false); // 狭い画面の一時パネルから開いたとき、左を閉じて下書きを見せる
        setQuick((q) => ({ ...q, open: true }));
        window.requestAnimationFrame(() => quickRef.current?.focus());
        break;
      case "newChat": setDialog({ type: "newChat" }); break;
      case "newChatWorktree": setDialog({ type: "newChat", dev: true }); break;
      case "newChatSameFolder": void sameFolderNew(chat?.key.id); break;
      case "worktrees": setDialog({ type: "worktrees" }); break;
      case "cloud": setDialog({ type: "cloud", chatId: chat?.key.id ?? null }); break;
      case "parity": setDialog({ type: "parity" }); break;
      case "goal": if (chat) setDialog({ type: "goal", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "purpose": if (chat) setDialog({ type: "purpose", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "purposeDraft": if (chat) setDialog({ type: "purposeDraft", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "handoffPreview": if (chat) setDialog({ type: "handoffPreview", chatId: chat.key.id }); break;
      case "handoff": if (chat && !dialog) window.dispatchEvent(new Event("agentdock:handoff-open")); break;
      case "review": if (chat) setDialog({ type: "review", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "fork": if (chat) setDialog({ type: "fork", chatId: chat.key.id, throughTurn: null }); else say("チャットを選んでください。"); break;
      case "compact": if (chat) setDialog({ type: "compact", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "palette": if (!dialog) setPalette(true); break;
      case "status": setDialog({ type: "status", chatId: chat?.key.id ?? null }); break;
      case "usage": setDialog({ type: "usage" }); break;
      case "workMode":
        if (!chat) { say("チャットを選んでください。"); break; }
        onWorkMode(bundle.modelSettings[chat.key.id]?.workMode === "plan" ? "default" : "plan");
        break;
      case "diff": if (chat) setDialog({ type: "changes", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "revert": if (chat) setDialog({ type: "revert", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "quit":
        if (!live) { setDialog({ type: "quit" }); break; }
        host.requestQuit().then((p) => { setQuit(p); setDialog({ type: "quit" }); }).catch((e) => sayErr("終了を要求できませんでした", e));
        break;
      case "force": openForce(); break;
      case "attach": setDialog({ type: "attach" }); break;
      case "reference": if (chat) setDialog({ type: "reference", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "skills": if (chat) setDialog({ type: "skills", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "instructions": if (chat) setDialog({ type: "instructions", chatId: chat.key.id }); else say("チャットを選んでください。"); break;
      case "sideOpen": void openSide(); break;
      case "archiveChat": archiveOp(true, chat?.key.id); break;
      case "unarchiveChat": archiveOp(false, chat?.key.id); break;
      case "exportMd": openExport(chat?.key.id); break;
      case "deleteChat": openDelete(chat?.key.id); break;
      case "togglePin": togglePin(chat?.key.id); break;
      case "renameChat": if (chat) setDialog({ type: "rename", chatId: chat.key.id }); break;
      case "refreshList": void refreshList(); break;
      case "openCwd": { const c = chat ? knownValue(chat.cwd) : null; if (chat && live && c) openWithConfirm(chat.key, c); break; }
      case "reloadHistory": if (chat && live) { void loadChat(chat.key.id); say("保存履歴を取り直しています（読み取りのみ）。"); } break;
      case "attachPick": void pickFiles(); break;
      case "attachPasteSelection": void pasteSelection(); break;
      case "interrupt": void interrupt(); break;
      case "modeMenu": setMockOpen((o) => !o); break;
      case "resumeExternal": if (chat) setDialog({ type: "resumeExternal", chatId: chat.key.id }); break;
      case "doResumeExternal": if (dialog?.type === "resumeExternal") void resumeExternal(dialog.chatId); break;
      case "retryLaunch": if (live) retryLaunch(exePath); break;
      case "retrySave": void retrySave(); break;
      case "toggleLeft": if (leftTemp) setLNarrow((v) => !v); else setLeftOpen((v) => !v); break;
      case "toggleRight": if (rightTemp) setRNarrow((v) => !v); else setRightOpen((v) => !v); break;
      case "find": if (chat && !dialog) window.dispatchEvent(new Event("agentdock:find")); break;
      case "toggleMini": case "toggleMonitor":
        // 実接続では別窓の監視窓を開閉する（開いていれば閉じる。開くときフォーカスは奪わない）。モックでは画面内の仮の窓。
        if (live) host.toggleMonitorWindow().then(setMonitorOpen).catch((e) => sayErr("監視窓を開閉できませんでした", e)); else setMini((v) => !v);
        break;
      case "cwdChange": if (chat && chat.kind === "development") window.dispatchEvent(new Event("agentdock:cwd-toggle")); else say("開発チャットだけで使えます。"); break;
      case "shortcuts": setDialog({ type: "shortcuts" }); break;
      case "howto": setDialog({ type: "howto" }); break;
      case "about": setDialog({ type: "about" }); break;
      case "closeMini": setMini(false); break;
      case "miniTop": case "toggleMiniTop": setTop("monitor", !miniTop); break;
      case "toggleMainTop": setTop("main", !mainTop); break;
      case "toggleDone": onScope({ kind: "allChats", showFinished: !(scope.kind === "allChats" && scope.showFinished) }); break;
      case "quitStop": case "doForce": setDialog(null); say("この操作はモックでは動きません。"); break;
      default: say("この操作は後続の段階で対応します。");
    }
  };

  // キー処理は shortcuts.ts の matchShortcut に集約（変換中は無視。capture で WebView 既定の動作より先に受ける）。
  // Ctrl+Shift+M（監視窓の開閉）は画面の状態を変えないので、ダイアログ・コマンドメニューの表示中も効く。
  // それ以外は、ダイアログ・コマンドメニューの表示中は動かさない（Ctrl+K だけは開いているコマンドメニューを閉じられる）。
  const findable = !!chat && !dialog && !palette;
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (matchShortcut(e, "toggleMonitor")) { e.preventDefault(); act("toggleMonitor"); return; }
      if (matchShortcut(e, "palette")) {
        e.preventDefault();
        if (dialog) return;
        setMenu(null);
        setPalette((v) => !v);
        return;
      }
      if (matchShortcut(e, "quickNew")) { e.preventDefault(); if (!dialog && !palette) { setMenu(null); act("newChatQuick"); } return; }
      if (matchShortcut(e, "newChat")) { e.preventDefault(); if (!dialog && !palette) { setMenu(null); act("newChat"); } return; }
      if (matchShortcut(e, "toggleLeft") || matchShortcut(e, "toggleRight")) {
        e.preventDefault();
        if (!dialog && !palette) act(matchShortcut(e, "toggleRight") ? "toggleRight" : "toggleLeft");
        return;
      }
      if (matchShortcut(e, "find")) { e.preventDefault(); if (findable) window.dispatchEvent(new Event("agentdock:find")); }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  });

  // 確認済みの失敗。実接続ではホストの記録（chatLocals）と現在の失敗turnの一致で判定し、左一覧と揃える。モックは画面内の集合。
  const confirmedKeys: Set<string> = live
    ? new Set(snap.agents.filter((a) => {
      if (a.status.state !== "failed") return false;
      const turn = a.status.turn ?? a.agent.latestTurn ?? "";
      const acked = snap.chatLocals.find((l) => l.chat.id === a.agent.chat.id)?.acknowledgedFailures ?? [];
      return acked.some((t) => t.agent.id === a.agent.key.id && t.turnId === turn);
    }).map((a) => keyStr(a.agent.key)))
    : confirmed;

  // ドックのカードの回答（中央と同じ onRespond を通す。対象は押したカード1件）
  const dockActions: DockActions = {
    respond: (r, a) => void onRespond(r, a),
  };
  const dockProps = {
    snap, scope, selId, confirmed: confirmedKeys, onScope, onAct: act, actions: dockActions, focusReq: dockFocus,
    onOpen: (id: string) => { selectChat(id); },
    onConfirmFail: (k: string) => {
      if (!live) setConfirmed((s) => new Set(s).add(k));
      const v = snap.agents.find((a) => keyStr(a.agent.key) === k);
      if (v) onAcknowledge(v.agent.chat, v.agent.key);
    },
    // 状態不明の子の履歴再確認（保存履歴の読取りだけ。結果は実際に履歴で確認できた件数だけを案内する）。
    onRecheck: async (c: ChatKey, agents: AgentKey[]) => {
      try {
        const r = live ? await host.recheckUnknownAgents(c, agents) : await (await loadMock()).client.recheckUnknownAgents(c, agents);
        say(recheckSummary(r.results.map((x) => ({ kind: x.outcome.kind, reason: x.outcome.kind === "skipped" ? x.outcome.reason : undefined }))), 6000);
      } catch (e) { sayErr("履歴で再確認できませんでした（状態不明のままです）", e); }
    },
  };

  return (
    <SendKeyContext.Provider value={enterMode}>
    <LinkContext.Provider value={linkActions}>
    <RefContext.Provider value={refActions}>
      <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
        <defs><pattern id="hatch" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="2" height="4" style={{ fill: "var(--unk)" }} /></pattern></defs>
      </svg>
      <div className="window" role="application" aria-label="AgentDock" onClick={() => { if (menu) setMenu(null); if (mockOpen) setMockOpen(false); }}>
        <MenuBar open={menu} setOpen={setMenuOpen} views={cmdViews} onAct={act} tag={modeTag} live={live} />
        <GlobalBanner source={src} dismissed={snap.settings.dismissedVersionNotice}
          onDismiss={(key) => updateSettings((s) => ({ ...s, dismissedVersionNotice: key }), "版の注意を閉じた記録を保存できませんでした。次回の起動ではまた表示されます")} onAct={act} />
        {live && quit.kind !== "idle" && dialog?.type !== "quit" && dialog?.type !== "force" ? (
          <div className="gbanner warn" role="status">
            <span className="grow">終了の手順を続けています（停止と保存を確認中。確認できるまで終了しません）。</span>
            <button className="btn-line" onClick={() => setDialog({ type: "quit" })}>状況を見る</button>
          </div>
        ) : null}
        <SaveBanner failed={failedSaves.filter((s) => !forChat(s))} warnings={warnHidden ? [] : snap.startupWarnings} onDismissWarnings={dismissWarnings} onAct={act} />
        <div
          className={`body ${leftOpen ? "" : "l-off"} ${rightOpen ? "" : "r-off"} ${lNarrow ? "n-l" : ""} ${rNarrow ? "n-r" : ""}`}
          data-left-temp={leftTemp ? "" : undefined} data-right-temp={rightTemp ? "" : undefined}
          style={{ ...(!leftTemp && leftOpen ? { "--lw": `${paneW.left}px` } : {}), ...(!rightTemp && rightOpen ? { "--rw": `${paneW.dock}px` } : {}) } as CSSProperties}
        >
          <aside className="left" id="pane-left" aria-label="チャット一覧">
            <LeftPane org={{ wf: org?.workflow ?? null, selected: orgScreen && !orgHist, onOpen: () => { setOrgSel(null); setOrgScreen(true); setLNarrow(false); setQuick((q) => (q.open ? { ...q, open: false } : q)); }, onStart: () => act("orgStart"), history: org?.history ?? [], deletionOverflow: org?.deletionOverflow ?? 0, archived: org?.archived ?? [], onUnarchive: (id) => void orgRun("アーカイブの解除", async () => { await host.orgUnarchiveWorkflow(id); return "アーカイブを解除しました。"; }), onArchive: (id) => void orgRun("アーカイブ", async () => { await host.orgArchiveWorkflow(id); return "ワークフローをアーカイブしました（アーカイブ一覧から戻せます）。"; }), live, onDelete: (id) => setDialog({ type: "orgDelete", workflowId: id }), selectedHistory: orgHist?.workflowId ?? null, onOpenHistory: (id) => { setOrgSel(id); setOrgScreen(true); setLNarrow(false); setQuick((q) => (q.open ? { ...q, open: false } : q)); }, shown: orgShownIds }} creating={creating} onCreatingAct={creatingAct} snap={snap} sel={chat ? keyStr(chat.key) : null} onSelect={(id) => { selectChat(id); setLNarrow(false); }} onAct={act} onAcknowledge={(c) => onAcknowledge(c.key, null)}
              onChatAct={chatAct} chatList={snap.settings.chatList} projects={projects} onNotice={say} onReclassify={reclassify} onFetchProjectChats={fetchProjectChats} onLoadMore={listCursor ? loadMoreChats : undefined}
              onNewChatIn={(cwd, choices) => { setDialog({ type: "newChat", dev: true, cwd, cwdChoices: choices.length > 1 ? choices : undefined }); setLNarrow(false); }}
              onChatList={(f) => updateSettings((s) => ({ ...s, chatList: f(s.chatList) }), "一覧の表示設定を保存できませんでした。次回の起動では保存されている値に戻ります")} listStatus={listStatus} uiSize={snap.settings.appearance.uiText} />
          </aside>
          <main className="center">
            {orgScreen ? (
              <OrgPane snap={snap} org={orgShown} error={orgErr} live={live} timelineTick={orgTlTick} leftShown={leftTemp ? lNarrow : leftOpen} rightShown={rightTemp ? rNarrow : rightOpen}
                onAct={act} onOpenChat={(k) => selectChat(k.id)} onReload={() => setOrgTick((n) => n + 1)} run={orgRun} onDelete={(id) => setDialog({ type: "orgDelete", workflowId: id })}
                renderRequest={(r, agentName) => <RequestCard r={r} onRespond={onRespond} agentName={agentName} />} />
            ) : quick.open || (!chat && live) ? (
              <QuickNewPane models={models} model={quick.model ?? pickQuickModel(models, snap.settings.lastNewChat, snap.settings.defaultModel)} setModel={(id) => setQuick((q) => ({ ...q, model: id }))}
                text={quick.text} setText={(v) => setQuick((q) => ({ ...q, text: v }))} enterMode={enterMode} busy={!!quick.busy} inputRef={quickRef}
                onSend={quickSend} onDetail={() => { setDialog({ type: "newChat", prefill: { model: quick.model ?? pickQuickModel(models, snap.settings.lastNewChat, snap.settings.defaultModel), first: quick.text, isolate: false, worktree: null } }); setQuick({ open: false, text: "", model: null }); }} />
            ) : chat ? (
              <CenterPane
                snap={snap} chat={chat} turns={bundle.turns[chat.key.id] ?? []} history={histLoads[chat.key.id]} onReloadHistory={() => void loadChat(chat.key.id)}
                attachments={draftAttachments} files={files} inputRef={composerRef}
                skillPick={live ? { skills: skillLists[chat.key.id], load: () => loadSkills(chat.key), recent: snap.settings.recentSkills ?? [], pick: (sk) => { void pickSkill(chat.key, sk.name, sk.path); } } : null}
                settings={bundle.modelSettings[chat.key.id]} models={models} save={chatSave}
                externalLabel={bundle.externalLabel[chat.key.id]} wasLive={liveSeen.current.has(chat.key.id)} enterMode={enterMode} runningInput={snap.settings.runningInput ?? "queue"}
                draft={curDraft} setDraft={(v) => onDraft(chat, v)}
                onAct={act} commands={cmdViews} onRespond={(r, a) => void onRespond(r, a)}
                onSend={onSend} loadHandoffPreview={loadHandoffPreview} onHandoffSend={onHandoffSend} onModel={onModel} onWorkMode={onWorkMode} workModes={workModes}
                decisions={decisions[chat.key.id] ?? NO_DECISIONS} pending={markRetryable(pendingSends[chat.key.id], attempts[chat.key.id])} onPendingRestore={(p) => pendingRestore(chat, p)} onPendingDismiss={(p) => pendingDismiss(chat, p)} goal={goals[chat.key.id]} goalBusy={goalBusy} onGoalReload={() => void loadGoal(chat.key)} onGoalClear={clearGoal}
                notice={live ? hideClosed(noticeOf(attempts[chat.key.id], steerOf[chat.key.id] ? { attemptId: steerOf[chat.key.id].attemptId, queueInstead: () => { void onSend(steerOf[chat.key.id].text, false, steerOf[chat.key.id].attachments); } } : null), noticeClosed[chat.key.id]) : null} onCloseNotice={(key) => setNoticeClosed((m) => ({ ...m, [chat.key.id]: key }))} onRetrySend={() => void onRetrySend()}
                queue={snap.queues.find((q) => q.chat.id === chat.key.id)} local={snap.chatLocals.find((l) => l.chat.id === chat.key.id)}
                qact={qact} onPermission={onPermission} wf={wf} sends={sends[chat.key.id] ?? NO_SENDS}
                orgInterject={(() => { const o = snap.chatLocals.find((l) => l.chat.id === chat.key.id)?.org; const w = org?.workflow ?? null; return o && !o.released && w && w.workflowId === o.workflowId ? { workflowId: w.workflowId, task: w.tasks.find((t) => t.id === o.taskId) ?? null, interjects: w.interjects, readonly: w.readonly, run: orgRun } : null; })()}
                cwdInfo={headCwd ? pathInfo(headCwd) : undefined}
                leftShown={leftTemp ? lNarrow : leftOpen} rightShown={rightTemp ? rNarrow : rightOpen}
              />
            ) : <EmptyCenter onAct={act} />}
          </main>
          <aside className="right" id="pane-dock" aria-label="エージェントのドック"><DockPane {...dockProps} /></aside>
          {!leftTemp && leftOpen ? <PaneSplitter kind="left" width={paneW.left} max={paneW.leftMax} onCommit={(w) => onPaneCommit("left", w)} /> : null}
          {!rightTemp && rightOpen ? <PaneSplitter kind="dock" width={paneW.dock} max={paneW.dockMax} onCommit={(w) => onPaneCommit("dock", w)} /> : null}
          {lNarrow || rNarrow ? <div className="panel-scrim" aria-hidden="true" onClick={() => { setLNarrow(false); setRNarrow(false); }} /> : null}
        </div>
      </div>
      {live && chat && sideId && sideShown ? (
        <SidePanel snap={snap} side={sideShown} live={live} cap={snap.opCapabilities.find((c) => c.op === "sideChat")} onClose={() => setSideId(null)}
          onRespond={(r, a) => void onRespond(r, a)} onInsertMain={insertIntoDraft} say={say} />
      ) : null}
      {mini && !live ? <MiniWindow {...dockProps} top={miniTop} /> : null}
      {palette && !dialog ? <CommandPalette views={cmdViews} onRun={act} onClose={() => setPalette(false)} /> : null}
      {dialog ? (
        <Dialogs d={dialog} modelDefault={{ saved: snap.settings.defaultModel?.model ?? null, set: (id) => updateSettings((s) => ({ ...s, defaultModel: id ? { model: id, effort: models.find((m) => m.id === id)?.defaultEffort ?? null } : null }), "新しいチャットの既定のモデルを保存できませんでした") }} cont={{
          value: snap.settings.continuation, locals: snap.chatLocals, savePurpose, loadPreview: (id) => loadHandoffPreview({ backend: "codex", id }),
          setValue: (c) => updateSettings((s) => ({ ...s, continuation: c }), "継続チャットの設定を保存できませんでした。次回の起動では保存されている値に戻ります"),
        }} org={{
          settings: { live, stallMinutes: snap.settings.orgStallMinutes, setStall: async (m) => { if (!live) return "モックでは動きません。"; try { await host.orgSetStallMinutes(m); return null; } catch (e) { return host.asIpcError(e).message; } } },
          begin: intakeBegin, browse: folderTools.browse, onIntakeStarted: intakeStarted, go: setDialog,
          previewDelete: (id) => (live ? host.orgPreviewDelete(id) : Promise.reject(new Error("モックでは動きません。"))),
          deleteWorkflow: (id, del, expect) => host.orgDeleteWorkflow(id, del, expect),
          deleteProgress: orgDelProg, onDeleteIssue: (m) => say(m, 8000),
          onDeleted: (id) => { setDialog((d) => (d?.type === "orgDelete" && d.workflowId === id ? null : d)); if (orgSel === id) setOrgSel(null); setOrgTick((n) => n + 1); say("ワークフローを削除しました。worktree とブランチは残しています。", 6000); },
        }} folderTools={folderTools} workspaceRoots={snap.workspaceRoots} onClose={closeDialog} opCaps={snap.opCapabilities} resend={{ state: resendState, handlers: resendHandlers }} chats={snap.chats} source={src} models={models}
          prefs={{ goal: dialog.type === "goal" ? goals[dialog.chatId] : undefined, saveGoal,
            settings: dialog.type === "status" && dialog.chatId ? bundle.modelSettings[dialog.chatId] : undefined,
            local: dialog.type === "status" && dialog.chatId ? snap.chatLocals.find((l) => l.chat.id === dialog.chatId) : undefined }}
          worktree={{ records: snap.worktrees, cwd: chat?.cwd.kind === "value" ? chat.cwd.value : null }}
          cloudTasks={snap.cloudTasks}
          compose={{
            chat: chat ?? undefined, live, insertText: insertIntoDraft, pickSkill, recentSkills: snap.settings.recentSkills ?? [], noteSkills,
            sides: chat ? snap.sideSessions.filter((x) => x.main.id === chat.key.id).slice().sort((a, b) => (b.openedAt ?? 0) - (a.openedAt ?? 0)) : [],
            onOpenSide: () => void openSide(),
            onShowSide: (id) => { setDialog(null); setSideId(id); },
            openPath: (p) => { if (chat) host.openInstructionFile(chat.key, p).catch((e) => sayErr("開けませんでした", e)); },
          }}
          threadOps={{ pendingOps: snap.pendingOps, queues: snap.queues, onOpenChat: (k) => { selectChat(k.id); } }}
          live={live} changesTick={changesTick} turnRecords={dialog.type === "changes" || dialog.type === "revert" ? bundle.turns[dialog.chatId] ?? [] : []} onRevertDone={(r) => { setChangesTick((n) => n + 1); if (r) say(r.failed.length > 0 ? `一部のみ戻しました（失敗 ${r.failed.length} 件）。` : `${r.reverted.length} 件のファイルを書き換えました。`); }}
          enterMode={enterMode} setEnterMode={setEnterMode}
          runningInput={snap.settings.runningInput ?? "queue"} setRunningInput={(m) => { void updateSettings((st) => ({ ...st, runningInput: m }), "実行中の入力の設定を保存できませんでした。次回の起動では保存されている値に戻ります"); }}
          notify={{ value: snap.settings.notifications, set: onNotifySettings }}
          top={{ main: mainTop, mini: miniTop, setMain: (b) => setTop("main", b), setMini: (b) => setTop("monitor", b) }}
          autostart={{
            value: snap.settings.autostart,
            set: (on) => updateSettings((s) => ({ ...s, autostart: on }), "自動起動の設定を変更できませんでした"),
          }}
          appearance={{
            value: snap.settings.appearance, view: snap.settings.chatList.view,
            set: (a) => updateSettings((s) => ({ ...s, appearance: a }), "表示の設定を保存できませんでした。次回の起動では保存されている値に戻ります"),
            setView: (v) => updateSettings((s) => ({ ...s, chatList: { ...s.chatList, view: v } }), "一覧の並びを保存できませんでした。次回の起動では保存されている値に戻ります"),
          }}
          quit={{ phase: quit, stops: snap.stops, failedSaves, live, decide, onForce: openForce, onRetrySave: () => void retrySave() }}
          force={{ ...force, run: () => void runForce() }}
          manage={{
            selectedId: selId,
            ackedWarnings: { list: snap.settings.acknowledgedWarnings, reset: () => void resetWarnings() },
            del: { ...del, local: snap.chatLocals.find((l) => l.chat.id === (dialog.type === "delete" ? dialog.chatId : "")), run: () => void runDelete(), archive: () => { if (dialog.type === "delete") { archiveOp(true, dialog.chatId); setDialog(null); } } },
            exp: { ...exp, setInclude: (b) => setExp((s) => ({ ...s, include: b })), run: () => void runExport() },
            usage: { ...usage, reload: loadUsage },
            baselines: {
              enabled: snap.settings.baselines.enabled,
              setEnabled: setBaselinesEnabled,
              repos: snap.settings.baselines.repos,
              setRepo: setRepoBaseline,
              deleteForSelected: deleteChatBaselines,
              deleteBlocked: !live ? "モックでは動きません。" : !chat ? "チャットを選ぶと削除できます。" : isWorkingForDock(snap, chat) ? "作業中のチャットの控えは削除できません。" : null,
            },
          }}
          exe={{ path: exePath, setPath: setExePath, placeholder: DEFAULT_EXE, connect: retryLaunch, browse: browseExe, openDiag: () => { host.openDiagDir().then((p) => say(`診断ログの場所: ${p}`)).catch((e) => sayErr("診断ログの場所を開けませんでした", e)); }, live: live && snap.sources.every((s) => s.connection.kind !== "connected"), sandboxWorkaround: { value: snap.settings.sandboxWorkaround, set: (on) => updateSettings((s) => ({ ...s, sandboxWorkaround: on }), "sandbox の競合回避の設定を保存できませんでした") } }}
          onCreateChat={createChat} onRename={renameChat}
          setTab={(t) => setDialog({ type: "settings", tab: t })} onAct={act} />
      ) : null}
      {dragging ? <div className="dropzone" role="status">ここにドロップすると、このチャット用にコピーして添付します（元のファイルは変更しません。フォルダは添付できません）</div> : null}
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
    </RefContext.Provider>
    </LinkContext.Provider>
    </SendKeyContext.Provider>
  );
}

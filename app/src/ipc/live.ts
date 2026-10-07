// ホストイベント → 表示状態の reducer（純粋関数）。状態判定はホストの責務で、ここは受け取った値を並べ替えて保持するだけ。
// 規則: 通信断・取得不能をdone/failedに変換しない。逐次本文（activityDelta）は描画専用。
import type { Bundle } from "../mock/data";
import type {
  ActivityKind, AppSettings, Capabilities, HostEvent, HostSnapshot, ItemKey, Known, SaveScope, TranscriptEntry, TurnKey, TurnRecord,
} from "./types";

/** ホスト（`AppSettings::default`）と同じ初期値。接続前の表示用で、保存済みの値ではない。 */
export const DEFAULT_SETTINGS: AppSettings = {
  executables: {}, autostart: false,
  notifications: { enabled: true, approvalAndQuestion: true, completed: true, failed: true, sound: true, showChatName: true },
  mainWindow: { alwaysOnTop: false, bounds: null }, monitorWindow: { alwaysOnTop: false, bounds: null },
  monitorScope: "selectedChat", defaultModel: null, sendKey: "ctrlEnter", acknowledgedWarnings: [], tools: { git: null }, cloudEnvByRepo: {},
};

const emptySnapshot = (): HostSnapshot => ({
  seq: 0, sources: [], chats: [], agents: [], requests: [], stops: [], queues: [], monitorScope: { kind: "selectedChat", chat: null },
  chatLocals: [], saveStatus: [], settings: DEFAULT_SETTINGS, modelSettings: [], startupWarnings: [], attachments: [], artifacts: [], quit: { kind: "idle" },
  opCapabilities: [], pendingOps: [], sideSessions: [], worktrees: [], cloudTasks: [],
});

export const emptyBundle = (): Bundle => ({
  snapshot: emptySnapshot(), turns: {}, modelSettings: {}, save: null, externalLabel: {},
});

/** 接続前の能力（すべて不明。非対応とは断定しない）。 */
export const UNKNOWN_CAPS: Capabilities = {
  descendantMonitoring: "unknown", descendantSearch: "unknown", historyReadWithoutResume: "unknown", resume: "unknown", listLoaded: "unknown",
  approvalKinds: [], steer: "unknown", interrupt: "unknown", modelSelection: "unknown", effortSelection: "unknown", rename: "unknown",
  archive: "unknown", delete: "unknown", externalHistory: "unknown", attachmentKinds: [], managedExecControl: "unknown", ops: [],
};

const upsert =<T,>(list: T[], item: T, same: (a: T) => boolean): T[] => {
  const i = list.findIndex(same);
  if (i < 0) return [...list, item];
  const next = list.slice();
  next[i] = item;
  return next;
};

/** スナップショットで置き換える（取り直し時）。会話本文は保持し、モデル設定はホストが持つ値（再起動後の復元分を含む）で更新する。 */
export function replaceSnapshot(b: Bundle, snapshot: HostSnapshot): Bundle {
  const modelSettings = { ...b.modelSettings };
  for (const e of snapshot.modelSettings) modelSettings[e.chat.id] = e.settings;
  return { ...b, snapshot, modelSettings, externalLabel: externalLabels(snapshot) };
}

function externalLabels(s: HostSnapshot): Record<string, string> {
  const out: Record<string, string> = {};
  for (const c of s.chats) if (c.origin === "external") out[c.key.id] = "外部";
  return out;
}

/** 保存の単位が同じか（kind＋対象チャット）。 */
export function sameScope(a: SaveScope, b: SaveScope): boolean {
  if (a.kind !== b.kind) return false;
  return "chat" in a && "chat" in b ? a.chat.id === b.chat.id : true;
}

const PENDING_TURN = "pending";

function turnFor(turns: TurnRecord[], agent: TurnKey["agent"], turnId: string): [TurnRecord[], number] {
  const i = turns.findIndex((t) => t.key.agent.id === agent.id && t.key.turnId === turnId);
  if (i >= 0) return [turns, i];
  const t: TurnRecord = {
    key: { agent, turnId }, end: null, startedAt: { kind: "notFetched" }, completedAt: { kind: "notFetched" }, entries: [], complete: false,
  };
  return [[...turns, t], turns.length];
}

function withEntry(b: Bundle, item: ItemKey, f: (old: TranscriptEntry | undefined) => TranscriptEntry): Bundle {
  const chatId = item.agent.id;
  // 会話本文はチャット本体（ルート）のturnだけ持つ。子孫の活動は右のドックに出る。
  if (!b.snapshot.chats.some((c) => c.key.id === chatId)) return b;
  const [turns, ti] = turnFor(b.turns[chatId] ?? [], item.agent, item.turnId ?? PENDING_TURN);
  const t = turns[ti];
  const ei = t.entries.findIndex((e) => e.key.itemId === item.itemId);
  const entry = f(ei >= 0 ? t.entries[ei] : undefined);
  const entries = ei >= 0 ? t.entries.map((e, i) => (i === ei ? entry : e)) : [...t.entries, entry];
  const nextTurns = turns.map((x, i) => (i === ti ? { ...x, entries } : x));
  return { ...b, turns: { ...b.turns, [chatId]: nextTurns } };
}

const textOf = (k: Known<string> | undefined): string => (k && k.kind === "value" ? k.value : "");
const val = (value: string): Known<string> => ({ kind: "value", value, basis: "direct" });

export function applyHostEvent(b: Bundle, e: HostEvent): Bundle {
  const s = b.snapshot;
  const set = (patch: Partial<HostSnapshot>): Bundle => ({ ...b, snapshot: { ...s, ...patch } });
  switch (e.kind) {
    case "sourceUpdated":
      return set({ sources: [e.source] });
    case "chatUpdated": {
      const next = set({ chats: upsert(s.chats, e.chat, (c) => c.key.id === e.chat.key.id) });
      return { ...next, externalLabel: externalLabels(next.snapshot) };
    }
    case "chatRemoved": {
      const { [e.chat.id]: _gone, ...turns } = b.turns;
      // 削除が完了したチャットのアプリ側の状態（補足情報・キュー・停止記録）も画面から外す。
      return {
        ...set({
          chats: s.chats.filter((c) => c.key.id !== e.chat.id), agents: s.agents.filter((a) => a.agent.chat.id !== e.chat.id), requests: s.requests.filter((r) => r.chat.id !== e.chat.id),
          chatLocals: s.chatLocals.filter((l) => l.chat.id !== e.chat.id), queues: s.queues.filter((q) => q.chat.id !== e.chat.id), stops: s.stops.filter((r) => r.chat.id !== e.chat.id),
        }),
        turns,
      };
    }
    case "agentUpdated":
      return set({ agents: upsert(s.agents, e.view, (a) => a.agent.key.id === e.view.agent.key.id) });
    case "turnUpdated": {
      const chatId = e.turn.agent.id;
      const list = b.turns[chatId];
      if (!list || e.end === null) return b;
      const turns = list.map((t) => (t.key.agent.id === e.turn.agent.id && t.key.turnId === e.turn.turnId ? { ...t, end: e.end } : t));
      return { ...b, turns: { ...b.turns, [chatId]: turns } };
    }
    case "activityUpdated": {
      const a = e.activity;
      return withEntry(b, a.key, (old) => {
        const next = textOf(a.summary);
        const prev = textOf(old?.text);
        // 要約は短縮されることがあるので、逐次本文で既に長く持っていれば維持する（全文は完了後の履歴取得で置き換わる）。
        const text = prev.length > next.length ? old!.text : a.summary;
        return { key: a.key, kind: a.kind as ActivityKind, text, phase: a.phase };
      });
    }
    case "activityDelta":
      return withEntry(b, e.item, (old) => ({
        key: e.item, kind: old?.kind ?? { kind: "agentMessage" }, text: val(textOf(old?.text) + e.delta), phase: "inProgress",
      }));
    case "requestUpdated":
      return set({ requests: upsert(s.requests, e.request, (r) => r.key.requestId === e.request.key.requestId && r.key.source === e.request.key.source) });
    case "chatQueueUpdated":
      return set({ queues: upsert(s.queues, e.queue, (q) => q.chat.id === e.queue.chat.id) });
    case "stopUpdated":
      return set({ stops: upsert(s.stops, e.record, (r) => r.id === e.record.id) });
    case "modelSettingsUpdated":
      return { ...b, modelSettings: { ...b.modelSettings, [e.chat.id]: e.settings } };
    case "chatLocalUpdated":
      return set({ chatLocals: upsert(s.chatLocals, e.local, (l) => l.chat.id === e.local.chat.id) });
    case "pendingOpsUpdated":
      // 結果が未確認の操作（レビュー・圧縮）。そのチャットの分を置き換える。
      return set({ pendingOps: [...s.pendingOps.filter((p) => p.chat.id !== e.chat.id), ...e.pendingOps.map((pending) => ({ chat: e.chat, pending }))] });
    case "saveStatusUpdated":
      return set({ saveStatus: upsert(s.saveStatus, e.status, (x) => sameScope(x.scope, e.status.scope)) });
    case "attachmentUpdated":
      return set({ attachments: upsert(s.attachments, e.entry, (a) => a.id === e.entry.id) });
    case "artifactUpdated":
      return set({ artifacts: upsert(s.artifacts, e.entry, (a) => a.id === e.entry.id) });
    case "sideUpdated": // side相談の記録の1件（開いた・閉じた・終了）
      return set({ sideSessions: upsert(s.sideSessions, e.side, (x) => x.id === e.side.id) });
    case "worktreeUpdated": // worktree台帳の1件（record=null は記録を外した）
      return set({ worktrees: e.record ? upsert(s.worktrees, e.record, (w) => w.id === e.id) : s.worktrees.filter((w) => w.id !== e.id) });
    case "cloudTaskUpdated": // クラウド委任の記録の1件（状態語は原文のまま。AgentDockの状態へ写像しない）
      return set({ cloudTasks: upsert(s.cloudTasks, e.record, (t) => t.id === e.record.id) });
    case "settingsUpdated":
      return set({ settings: e.settings });
    case "opCapabilitiesUpdated": // experimental の拒否などによる降格。能力を出し直す
      return set({ opCapabilities: e.ops });
    case "toolServersChanged": // 開いているMCP・Pluginsの表示が画面側で取り直す（状態は変えない）
    case "toolServerLoginUpdated":
    case "extensionOpUpdated":
    case "goalUpdated": // 画面側（App）が目標の表示を置き換える（状態は変えない）
    case "changesUpdated": // 開いている差分表示が画面側で取り直す（状態は変えない）
    case "sendUpdated":
    case "quitPrompt":
    case "quitUpdated": // 終了手順の進行は画面側（App）が保持する（表示用の一時状態）
    case "systemResumed": // 画面側で履歴を取り直す（状態は変えない。復帰の通知・再送はしない）
    case "navigateToChat": // 画面側で該当チャットを開く（状態は変えない）
    case "warning":
      return b; // 画面側で通知として扱う
  }
}

/** seqの扱い。連続なら適用、古いなら無視、飛んだら取り直し。 */
export type SeqAction = "apply" | "ignore" | "resync";
export function seqAction(last: number, next: number): SeqAction {
  if (next <= last) return "ignore";
  return next === last + 1 ? "apply" : "resync";
}

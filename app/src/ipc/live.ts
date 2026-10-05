// ホストイベント → 表示状態の reducer（純粋関数）。状態判定はホストの責務で、ここは受け取った値を並べ替えて保持するだけ。
// 規則: 通信断・取得不能をdone/failedに変換しない。逐次本文（activityDelta）は描画専用。
import type { Bundle } from "../mock/data";
import type {
  ActivityKind, Capabilities, HostEvent, HostSnapshot, ItemKey, Known, TranscriptEntry, TurnKey, TurnRecord,
} from "./types";

const emptySnapshot = (): HostSnapshot => ({
  seq: 0, sources: [], chats: [], agents: [], requests: [], stops: [], queue: [], monitorScope: { kind: "selectedChat", chat: null },
});

export const emptyBundle = (): Bundle => ({
  snapshot: emptySnapshot(), turns: {}, attachments: [], modelSettings: {}, save: null, externalLabel: {},
});

/** 接続前の能力（すべて不明。非対応とは断定しない）。 */
export const UNKNOWN_CAPS: Capabilities = {
  descendantMonitoring: "unknown", descendantSearch: "unknown", historyReadWithoutResume: "unknown", resume: "unknown", listLoaded: "unknown",
  approvalKinds: [], steer: "unknown", interrupt: "unknown", modelSelection: "unknown", effortSelection: "unknown", rename: "unknown",
  archive: "unknown", delete: "unknown", externalHistory: "unknown", attachmentKinds: [], managedExecControl: "unknown",
};

const upsert =<T,>(list: T[], item: T, same: (a: T) => boolean): T[] => {
  const i = list.findIndex(same);
  if (i < 0) return [...list, item];
  const next = list.slice();
  next[i] = item;
  return next;
};

/** スナップショットで置き換える（取り直し時）。会話本文・モデル設定は保持する。 */
export function replaceSnapshot(b: Bundle, snapshot: HostSnapshot): Bundle {
  return { ...b, snapshot, externalLabel: externalLabels(snapshot) };
}

function externalLabels(s: HostSnapshot): Record<string, string> {
  const out: Record<string, string> = {};
  for (const c of s.chats) if (c.origin === "external") out[c.key.id] = "外部";
  return out;
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
      return { ...set({ chats: s.chats.filter((c) => c.key.id !== e.chat.id), agents: s.agents.filter((a) => a.agent.chat.id !== e.chat.id), requests: s.requests.filter((r) => r.chat.id !== e.chat.id) }), turns };
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
    case "queueUpdated":
      return set({ queue: upsert(s.queue, e.item, (q) => q.id === e.item.id) });
    case "stopUpdated":
      return set({ stops: upsert(s.stops, e.record, (r) => r.id === e.record.id) });
    case "modelSettingsUpdated":
      return { ...b, modelSettings: { ...b.modelSettings, [e.chat.id]: e.settings } };
    case "sendUpdated":
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

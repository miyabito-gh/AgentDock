// スナップショットからの表示用導出。状態判定そのものはホストの責務で、ここは並べ替えと文言だけ。
import type { AgentView, Chat, HistoryOutcome, HostSnapshot, PendingRequest, StopRecord } from "../ipc/types";
import { STATE, keyStr, knownValue } from "./format";
import { CHAT_DISPLAY_TEXT, chatDisplayKind, isActiveState, unknownCount, unknownNote, validOutcome, type ChatDisplayKind } from "./chatState";

/** 最初の依頼の先頭30字（仮表示用）。 */
export const previewTitle = (c: Chat): string | null => {
  const p = (knownValue(c.preview) ?? "").replace(/\s+/g, " ").trim();
  if (!p) return null;
  const chars = Array.from(p);
  return chars.length > 30 ? `${chars.slice(0, 30).join("")}…` : p;
};
/** 表示名。確認済みの名前が無ければ最初の依頼の先頭を仮表示する（confirmed=false で薄く表示）。 */
export const chatTitle = (c: Chat): { text: string; confirmed: boolean } => {
  const n = knownValue(c.name);
  if (n) return { text: n, confirmed: true };
  const p = previewTitle(c);
  return { text: p ?? "（名前未確認）", confirmed: false };
};
/** 一覧の並び（§3.6）。ピン→最近利用時刻の降順（無ければ作成時刻）→時刻不明。安定ソートで、ホストの `sort_chats` と同じ規則。 */
export const sortChats = (chats: Chat[]): Chat[] => {
  const t = (c: Chat): number | null => c.lastUsedAt ?? knownValue(c.createdAt) ?? null;
  return chats.map((c, i) => ({ c, i, t: t(c) })).sort((a, b) =>
    Number(b.c.pinned) - Number(a.c.pinned)
    || (a.t === null ? (b.t === null ? 0 : 1) : b.t === null ? -1 : b.t - a.t)
    || a.i - b.i).map((x) => x.c);
};
export const chatName = (c: Chat) => chatTitle(c).text;
export const TENTATIVE_STYLE = { opacity: 0.65, fontStyle: "italic" } as const;
export const TENTATIVE_HINT = "確認済みの名前ではありません（最初の依頼から仮表示）";
export const chatAgents = (s: HostSnapshot, c: Chat): AgentView[] => s.agents.filter((a) => keyStr(a.agent.chat) === keyStr(c.key));
export const rootView = (s: HostSnapshot, c: Chat): AgentView | undefined => chatAgents(s, c).find((a) => a.agent.parent.kind === "root");
export const chatRequests = (s: HostSnapshot, c: Chat): PendingRequest[] =>
  s.requests.filter((r) => keyStr(r.chat) === keyStr(c.key) && r.state.kind === "pending");
export const chatStops = (s: HostSnapshot, c: Chat): StopRecord[] => s.stops.filter((r) => keyStr(r.chat) === keyStr(c.key));
export const isRunning = (s: HostSnapshot, c: Chat): boolean => {
  const st = rootView(s, c)?.status.state;
  return st === "running" || st === "waiting" || st === "initializing";
};
/** 停止未確認・所有不明を含む記録があるか（ホストが判定した summary をそのまま見る） */
export const stopOpen = (r: StopRecord): boolean => r.targets.some((t) => t.summary === "unconfirmed" || t.summary === "interruptRequested" || t.summary === "ownershipUnknown");

/** 状態不明のエージェントについて、履歴で終端を確認できた結果（無ければ null）。状態そのものは変えない。 */
export const historyOutcomeOf = (s: HostSnapshot, v: AgentView): HistoryOutcome | null =>
  validOutcome(v.status.state, v.agent.latestTurn, s.historyConfirmations.find((h) => h.agent.id === v.agent.key.id));
/** 親以外（子・孫など）の状態。表示用の導出にだけ使う（送信条件・通知・停止確認の判定には使わない）。 */
export const descendantStates = (s: HostSnapshot, c: Chat) =>
  chatAgents(s, c).filter((a) => a.agent.parent.kind !== "root").map((a) => a.status.state);
/** 履歴で終端を確認できた子孫を除いた、子孫の状態（「子N件の状態不明」の件数用）。 */
const unresolvedDescendantStates = (s: HostSnapshot, c: Chat) =>
  chatAgents(s, c).filter((a) => a.agent.parent.kind !== "root" && !historyOutcomeOf(s, a)).map((a) => a.status.state);
/** チャット単位の表示種別（子孫込み）。 */
export const chatDisplay = (s: HostSnapshot, c: Chat): ChatDisplayKind => {
  const st = rootView(s, c)?.status.state;
  return st ? chatDisplayKind(st, descendantStates(s, c)) : "root";
};
/** 「実行中」表示の対象: 対応待ち、親が作業中、親が完了・待機で子孫が作業中。状態不明の子孫だけでは出さない。アーカイブの有無は見ない。 */
export const isWorkingForDock = (s: HostSnapshot, c: Chat): boolean => {
  if (chatRequests(s, c).length) return true;
  const st = rootView(s, c)?.status.state;
  if (!st) return false;
  const d = chatDisplay(s, c);
  return isActiveState(st) || d === "childWorking";
};

export function chatStatusText(s: HostSnapshot, c: Chat): string {
  if (chatRequests(s, c).length) return "対応待ち";
  const root = rootView(s, c);
  if (c.noHistory) return "履歴なし（Codex に記録なし）";
  if (!root) return "状態不明";
  const kind = chatDisplayKind(root.status.state, descendantStates(s, c));
  if (kind === "childFail") return CHAT_DISPLAY_TEXT.childFail;
  if (c.origin === "external") return "外部・状態不明";
  if (kind !== "root") return CHAT_DISPLAY_TEXT[kind];
  // 状態不明の子孫は親の状態に注記を添えるだけ（表示のみ。通知・キュー・停止確認の判定には使わない）
  const n = unknownCount(unresolvedDescendantStates(s, c));
  return n > 0 ? `${STATE[root.status.state].t}（${unknownNote(n)}）` : STATE[root.status.state].t;
}

export const isDoneLike = (st: AgentView["status"]["state"]) => st === "done" || st === "closed" || st === "interrupted";

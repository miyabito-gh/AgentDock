// スナップショットからの表示用導出。状態判定そのものはホストの責務で、ここは並べ替えと文言だけ。
import type { AgentView, Chat, HostSnapshot, PendingRequest, StopRecord } from "../ipc/types";
import { STATE, keyStr, knownValue } from "./format";

export const chatName = (c: Chat) => knownValue(c.name) ?? "（名前未確認）";
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

export function chatStatusText(s: HostSnapshot, c: Chat): string {
  if (chatRequests(s, c).length) return "対応待ち";
  const root = rootView(s, c);
  if (!root) return "状態不明";
  const childFail = chatAgents(s, c).some((a) => a.agent.parent.kind !== "root" && a.status.state === "failed");
  if (root.status.state === "running" && childFail) return "作業中・子の失敗あり";
  if (c.origin === "external") return "外部・状態不明";
  return STATE[root.status.state].t;
}

export const isDoneLike = (st: AgentView["status"]["state"]) => st === "done" || st === "closed" || st === "interrupted";

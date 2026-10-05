// IPC呼び出しの薄い宣言。T4時点では仮データを返す。T5で各関数の本体を invokeCmd に差し替える。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { makeBundle, MODELS, type Bundle } from "../mock/data";
import {
  HOST_EVENT_CHANNEL,
  type CommandMap, type CommandName, type HostEventEnvelope, type HostSnapshot, type ModelInfo,
  type MonitorScope, type RespondOutcome, type RequestKey, type RequestAnswer, type ChatKey,
  type InterruptChatResult, type SendIntent, type SendAttempt, type LocalId, type ModelChoice, type ChatModelSettings,
} from "./types";

/** 型付きinvoke。引数は `args` 1個で渡す（types.ts CommandMap の規約）。T5で使う。 */
export function invokeCmd<K extends CommandName>(name: K, args: CommandMap[K]["args"]): Promise<CommandMap[K]["result"]> {
  return invoke(name, { args });
}

/** ホストイベントの購読。seqの欠番検出と再snapshotはT5で行う。 */
export function subscribeHostEvents(handler: (e: HostEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<HostEventEnvelope>(HOST_EVENT_CHANNEL, (ev) => handler(ev.payload));
}

// ───────────── 仮実装（T5で置換） ─────────────

export async function getBundle(scenario: string): Promise<Bundle> {
  return makeBundle(scenario);
}
export async function getSnapshot(scenario = "normal"): Promise<HostSnapshot> {
  return makeBundle(scenario).snapshot;
}
export async function listModels(): Promise<ModelInfo[]> {
  return MODELS;
}
export async function setMonitorScope(_scope: MonitorScope): Promise<null> {
  return null;
}
export async function sendMessage(_chat: ChatKey, _text: string, _intent: SendIntent): Promise<SendAttempt> {
  return { attemptId: `mock-${Date.now()}`, clientMessageId: `cm-${Date.now()}`, at: Date.now(), state: { kind: "sending" } };
}
export async function retrySend(_chat: ChatKey, _attempt: LocalId): Promise<null> {
  return null;
}
export async function respondRequest(_request: RequestKey, _answer: RequestAnswer): Promise<RespondOutcome> {
  return { kind: "delivered" };
}
export async function interruptChat(_chat: ChatKey): Promise<InterruptChatResult | null> {
  return null; // 中断の受付と停止確認は別。T5でホストの StopRecord を受け取る。
}
export async function setChatModel(_chat: ChatKey, choice: ModelChoice): Promise<ChatModelSettings> {
  return { selected: choice, accepted: { kind: "notFetched" }, effective: { kind: "notFetched" }, applies: "nextTurn" };
}

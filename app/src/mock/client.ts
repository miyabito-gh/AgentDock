// モック切替用の仮実装（実IPCの代わりに仮データを返す）。製品の挙動ではない。
import { makeBundle, MODELS, type Bundle } from "./data";
import type {
  ChatKey, ChatModelSettings, InterruptChatResult, LocalId, ModelChoice, ModelInfo, MonitorScope, RequestAnswer, RequestKey,
  RespondOutcome, SendAttempt, SendIntent,
} from "../ipc/types";

export async function getBundle(scenario: string): Promise<Bundle> {
  return makeBundle(scenario);
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
  return null;
}
export async function setChatModel(_chat: ChatKey, choice: ModelChoice): Promise<ChatModelSettings> {
  return { selected: choice, accepted: { kind: "notFetched" }, effective: { kind: "notFetched" }, applies: "nextTurn", acceptedWorkMode: { kind: "notFetched" } };
}

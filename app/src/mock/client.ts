// モック切替用の仮実装（実IPCの代わりに仮データを返す）。製品の挙動ではない。
import { makeBundle, MODELS, type Bundle } from "./data";
import type {
  AgentKey, ChatKey, ChatModelSettings, ExternalId, ForkPurpose, InterruptChatResult, LocalId, ModelChoice, ModelInfo, MonitorScope, RecheckAgentsResult,
  RequestAnswer, RequestKey, RespondOutcome, ResendAsForkResult, SendAttempt, SendIntent,
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
// 編集して再送・履歴で再確認（モックでは何も起きない。ホストの実装は Rust 側）。
export async function resendAsFork(_chat: ChatKey, _throughTurn: ExternalId, _targetTurn: ExternalId, _text: string, _purpose: ForkPurpose, _send: boolean): Promise<ResendAsForkResult | null> {
  return null;
}
export async function recheckUnknownAgents(_chat: ChatKey, agents: AgentKey[]): Promise<RecheckAgentsResult> {
  return { results: agents.map((agent) => ({ agent, outcome: { kind: "stillUnknown" } })) };
}
export async function setChatModel(_chat: ChatKey, choice: ModelChoice): Promise<ChatModelSettings> {
  return { selected: choice, accepted: { kind: "notFetched" }, effective: { kind: "notFetched" }, applies: "nextTurn", acceptedWorkMode: { kind: "notFetched" } };
}

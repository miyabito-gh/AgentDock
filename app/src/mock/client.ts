// モック切替用の仮実装（実IPCの代わりに仮データを返す）。製品の挙動ではない。
import { makeBundle, MODELS, type Bundle } from "./data";
import { mockOrgGraphs, mockOrgRoles, mockOrgView } from "./org";
import type {
  AgentKey, ChatKey, ChatModelSettings, ExternalId, ForkPurpose, InterruptChatResult, LocalId, ModelChoice, ModelInfo, MonitorScope, RecheckAgentsResult,
  OrgGraphsView, OrgRolesView, OrgSnapshotView, RequestAnswer, RequestKey, RespondOutcome, ResendAsForkResult, SendAttempt, SendIntent,
} from "../ipc/types";
import type { HandoffPreview } from "../ipc/types";

/** 引き継ぎ資料の仮の確認結果（読取りのみの表示確認用。送れない）。 */
export async function previewHandoff(_chat: ChatKey): Promise<HandoffPreview> {
  const envelope = "【AgentDock 引き継ぎ資料】（モックの仮データ）\n目的メモ: 再接続の二重送信を直す。\n直近3往復: …";
  return { blockers: [], notes: ["子2件の状態は未確認のまま送ります（モック）"], envelope, chars: envelope.length, roundsIncluded: 3, roundsAvailable: 5, truncated: false };
}

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

// 組織実行の仮データ（シナリオ別）。
export async function orgView(scenario: string): Promise<OrgSnapshotView> {
  return mockOrgView(scenario);
}
export async function orgRolesView(): Promise<OrgRolesView> {
  return mockOrgRoles();
}
export async function orgGraphsView(): Promise<OrgGraphsView> {
  return mockOrgGraphs();
}

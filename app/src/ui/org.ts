// 組織実行の表示用導出（P-ORG-7b）。状態の判定はホスト。ここは並べ替え・文言・色クラスの対応だけ。
// 状態は文字とアイコン名（ホストの label／icon）で必ず示し、色だけで区別させない。
import type { Chat, ChatKey, HostSnapshot, OrgConfirmCode, OrgEffect, OrgEstimateView, OrgNodeKind, OrgTaskPhase, OrgTaskView, OrgWorkflowView, PendingRequest, RespondGap } from "../ipc/types";
import { chatRequests } from "./derive";
import { keyStr } from "./format";
export { isOrgOwned, shownOrgIds } from "./orgHidden";

export type OrgTone = "run" | "wait" | "fail" | "int" | "done" | "idle" | "unk";

/** 業務状態の色クラス（`.st.*`。形も状態ごとに違う）。 */
export const PHASE_TONE: Record<OrgTaskPhase, OrgTone> = {
  pending: "idle", ready: "idle", running: "run", blocked: "wait", reviewing: "run", rework: "wait", completed: "done", failed: "fail", cancelled: "int",
};
/** 確認状態の色クラス。確定以外は「止まって見える」ものとして区別する。 */
export const CONFIRM_TONE: Record<OrgConfirmCode, OrgTone> = {
  confirmed: "done", sendUnknown: "unk", needsReconcile: "unk", interruptRequested: "int", stopUnconfirmed: "wait",
};

export const EFFECT_TEXT: Record<OrgEffect, string> = {
  userTreeWrite: "あなたの作業ツリー・ブランチへの書込み",
  pushMergeTag: "push・マージ・タグ",
  outsideWorkdir: "作業フォルダ外",
  networkOrInstall: "ネットワーク取得・パッケージ導入",
  secretsOrAuth: "秘密情報・認証",
  deletionPrimary: "削除が主目的",
};

/** 表の並び順＝対応が要るものを先に。 */
export const TASK_GROUPS: Array<{ key: string; title: string; phases: OrgTaskPhase[] }> = [
  { key: "attention", title: "止まっている・失敗", phases: ["blocked", "failed"] },
  { key: "active", title: "実行中・確認中・差し戻し", phases: ["running", "reviewing", "rework"] },
  { key: "waiting", title: "待機中", phases: ["ready", "pending"] },
  { key: "done", title: "完了・取消し", phases: ["completed", "cancelled"] },
];

export function groupTasks(tasks: OrgTaskView[]): Array<{ key: string; title: string; tasks: OrgTaskView[] }> {
  return TASK_GROUPS.map((g) => ({ key: g.key, title: g.title, tasks: tasks.filter((t) => g.phases.includes(t.status.phase)) })).filter((g) => g.tasks.length > 0);
}

export const waitText = (t: OrgTaskView): string => (t.waitingOn.length ? `${t.waitingOn.join(",")}待ち` : "—");
export const attemptText = (t: OrgTaskView): string => `${t.attempts}/${t.maxAttempts}`;
/** 成果物＝タスクのコミット。記録がなければ「なし」（取得できなかったのではなく、記録がない）。 */
export const artifactText = (t: OrgTaskView): string => (t.commit ? `あり（${t.commit.slice(0, 8)}）` : "なし");

/** 「確認状態」が確定でないタスク（自動では進めない）。 */
export const needsAttention = (t: OrgTaskView): boolean => t.confirm.state !== "confirmed" || t.status.phase === "blocked" || t.status.phase === "failed" || t.needsChatChoice;

export interface WorkflowSummary { label: string; icon: string; tone: OrgTone; /** 要対応のタスク数（計画の承認待ちは含めない） */ attention: number; planWaiting: boolean }

/** 一覧の行・ヘッダーに出す全体の状態。 */
export function workflowSummary(w: OrgWorkflowView): WorkflowSummary {
  const attention = w.tasks.filter(needsAttention).length;
  const planWaiting = !!w.planApproval;
  const failed = w.tasks.filter((t) => t.status.phase === "failed").length;
  const cancelled = w.tasks.filter((t) => t.status.phase === "cancelled").length;
  if (planWaiting) return { label: "計画の承認待ち", icon: "eye", tone: "wait", attention, planWaiting };
  if (w.finished) {
    if (failed > 0) return { label: `終了（失敗 ${failed}件）`, icon: "x", tone: "fail", attention, planWaiting };
    if (cancelled > 0) return { label: `終了（取消し ${cancelled}件）`, icon: "ban", tone: "int", attention, planWaiting };
    return { label: "完了", icon: "check", tone: "done", attention, planWaiting };
  }
  if (w.halt) return { label: w.halt.label, icon: w.halt.icon, tone: w.halt.kind === "sandboxUnhealthy" || w.halt.kind === "definitionUnavailable" ? "fail" : "wait", attention, planWaiting };
  if (w.dispatching) return { label: "実行中", icon: "play", tone: "run", attention, planWaiting };
  return { label: "新しい送信は止まっています", icon: "pause", tone: "idle", attention, planWaiting };
}

/** ワークフロー内の役割ごとのタスク数（計画承認カード用）。 */
export function roleCounts(tasks: OrgTaskView[]): Array<{ role: string; n: number }> {
  const m = new Map<string, number>();
  for (const t of tasks) m.set(t.role, (m.get(t.role) ?? 0) + 1);
  return Array.from(m, ([role, n]) => ({ role, n }));
}

/** このワークフローのチャットで、Codexの承認・質問が待っているもの（「→ 要求へ」の元。回答は各チャットで利用者が行う）。 */
export function orgPendingRequests(snap: HostSnapshot, workflowId: string): { count: number; first: Chat | null } {
  let count = 0;
  let first: Chat | null = null;
  for (const c of snap.chats) {
    const org = snap.chatLocals.find((l) => l.chat.id === c.key.id)?.org;
    if (!org || org.workflowId !== workflowId) continue;
    const r: PendingRequest[] = chatRequests(snap, c);
    if (r.length > 0) { count += r.length; first = first ?? c; }
  }
  return { count, first };
}

// ── 承認・質問の待ち（待機理由。ホストには保存せず、スナップショットから導く） ──

/** あるタスクのチャットで、承認・質問の待ちと、回答を届けられなかった要求の件数（DESIGN_ORG_R1 2.2）。 */
export interface RequestWait {
  /** 承認待ち（質問以外の回答待ち。種類不明の操作待ちを含む）。 */
  approvals: number;
  /** 質問（userInput／toolElicitation）。 */
  questions: number;
  /** 回答できなかった要求（unanswerable と、今の turn の expired）。 */
  unanswerable: number;
  /** 回答待ちの要求（カードを出す元）。 */
  pending: PendingRequest[];
  /** 回答できなかった要求（理由の1行を出す元）。 */
  gaps: PendingRequest[];
  first: PendingRequest | null;
}

const NO_WAIT: RequestWait = { approvals: 0, questions: 0, unanswerable: 0, pending: [], gaps: [], first: null };

/** チャットのルートの今の turn（取れなければ null）。 */
function currentTurnOf(snap: HostSnapshot, chat: ChatKey): string | null {
  const v = snap.agents.find((a) => keyStr(a.agent.chat) === keyStr(chat) && a.agent.parent.kind === "root");
  return v?.status.turn ?? null;
}

/** 回答を届けられなかった要求。unanswerable は解決されるまで残す。expired は今の turn のものだけ（過去の切断の分は出さない）。 */
export function chatGapRequests(snap: HostSnapshot, chat: ChatKey): PendingRequest[] {
  const cur = currentTurnOf(snap, chat);
  return snap.requests.filter((r) => {
    if (keyStr(r.chat) !== keyStr(chat)) return false;
    if (r.state.kind === "unanswerable") return true;
    if (r.state.kind !== "expired") return false;
    // どの turn の要求か分からないときは出す（隠さない）。分かるなら今の turn のものだけ。
    return r.turn === null || cur === null || r.turn === cur;
  });
}

export function taskRequestWait(snap: HostSnapshot, t: OrgTaskView): RequestWait {
  if (!t.chat) return NO_WAIT;
  const chat = t.chat;
  const pending = snap.requests.filter((r) => keyStr(r.chat) === keyStr(chat) && r.state.kind === "pending");
  const questions = pending.filter((r) => r.kind.kind === "userInput" || r.kind.kind === "toolElicitation").length;
  const gaps = chatGapRequests(snap, chat);
  return { approvals: pending.length - questions, questions, unanswerable: gaps.length, pending, gaps, first: pending[0] ?? null };
}

/** 回答待ちの表示（表の「待ち先」列。なければ null）。 */
export function requestWaitText(w: RequestWait): string | null {
  const parts: string[] = [];
  if (w.approvals > 0) parts.push(`承認${w.approvals}`);
  if (w.questions > 0) parts.push(`質問${w.questions}`);
  return parts.length ? `あなたの回答待ち（${parts.join("・")}）` : null;
}

/** 回答できなかった要求の理由（常設表示）。 */
export const GAP_TEXT: Record<RespondGap | "expired", string> = {
  connectionChanged: "この要求には回答できません（接続が切り替わったため、Codex へ届けられません）。回答は送っていません。",
  notHeld: "AgentDock がこの要求を保持していないため、回答を届けられません（原因は特定できません）。",
  expired: "この要求には回答できません（接続が切れたため、期限切れです）。回答は送っていません。",
};
export const GAP_GUIDE_PLAIN = "Codex 側で turn が待ち続けている場合は中断してください。";
export const GAP_GUIDE = `${GAP_GUIDE_PLAIN}組織のタスクは中断後に『新しい試行で再送』できます。`;
export const gapText = (r: PendingRequest): string => (r.state.kind === "unanswerable" ? GAP_TEXT[r.state.cause] : GAP_TEXT.expired);

// ── 「待ち・作業中・完了」の塊（組織画面の表の上の帯） ──

export type WaitCause = "answer" | "limit" | "failed" | "manual" | "judge";

/** 利用者の対応待ちの種類（1タスク1つ。回答待ち＞上限＞失敗＞手動＞判断の順）。対応待ちでなければ null。 */
export function waitCauseOf(snap: HostSnapshot, t: OrgTaskView): WaitCause | null {
  const w = taskRequestWait(snap, t);
  if (w.pending.length > 0 || w.unanswerable > 0) return "answer";
  if (t.status.phase !== "blocked") return null;
  switch (t.status.reason) {
    case "usageLimit": case "limit": case "roundLimit": case "reworkLimit": return "limit";
    case "turnFailed": case "needsAttention": case "manualStop": case "checkFailed": return "failed";
    case "manual": return "manual";
    case "awaitingApproval": return "judge";
    default: return null;
  }
}

export interface WaitBand {
  waiting: { total: number; answer: number; limit: number; failed: number; manual: number; judge: number; firstId: string | null };
  working: { total: number; firstId: string | null };
  done: { done: number; all: number };
}

export function waitBand(snap: HostSnapshot, wf: OrgWorkflowView): WaitBand {
  const waiting = { total: 0, answer: 0, limit: 0, failed: 0, manual: 0, judge: 0, firstId: null as string | null };
  const working = { total: 0, firstId: null as string | null };
  for (const t of wf.tasks) {
    const c = waitCauseOf(snap, t);
    if (c) {
      waiting.total += 1; waiting[c] += 1; waiting.firstId = waiting.firstId ?? t.id;
    } else if (["running", "reviewing", "rework"].includes(t.status.phase)) {
      working.total += 1; working.firstId = working.firstId ?? t.id;
    }
  }
  return { waiting, working, done: { done: wf.tasks.filter((t) => t.status.phase === "completed").length, all: wf.tasks.length } };
}

/** 帯の「あなたの対応待ち」の内訳（0のものは出さない）。 */
export function waitingDetail(b: WaitBand["waiting"]): string {
  const parts: string[] = [];
  if (b.answer) parts.push(`回答 ${b.answer}`);
  if (b.limit) parts.push(`上限 ${b.limit}`);
  if (b.failed) parts.push(`失敗 ${b.failed}`);
  if (b.manual) parts.push(`手動 ${b.manual}`);
  if (b.judge) parts.push(`判断 ${b.judge}`);
  return parts.join("・");
}

/** 左一覧の行・ドックに出す件数（回答待ち・回答不可）。ワークフローのチャットすべての合計。 */
export function workflowRequestCounts(snap: HostSnapshot, wf: OrgWorkflowView): { wait: number; fail: number } {
  let wait = 0, fail = 0;
  for (const t of wf.tasks) {
    const w = taskRequestWait(snap, t);
    wait += w.approvals + w.questions;
    fail += w.unanswerable;
  }
  return { wait, fail };
}

/** 表の行の移動先ID（スクロール・フォーカス用）。 */
export const taskRowId = (id: string): string => `org-task-${id}`;

// ── グラフ（流れ）の表示・開始前の見積り。計算はホストの値を並べ替えるだけ（状態の判定はしない） ──

export const NODE_KIND_TEXT: Record<OrgNodeKind, string> = { agent: "役割", gate: "あなたの判断", router: "分岐", plannedTasks: "計画のタスク" };

export { verdictJa, verdictText, reworkLimitText, roundText, edgeText } from "./orgText";

const man = (tokens: number): string => String(Math.round(tokens / 1000) / 10);

/** 開始前の見積り（定義から計算した上限。実績ではない）。ホストの値（最大 turn と 1 turn あたりの土台）を文にするだけ。 */
export function estimateLines(e: OrgEstimateView | null, hint: number, maxTurns: number): { lines: string[]; over: boolean } {
  if (!e) return { lines: ["見積り: 未取得（このテンプレートの定義を読み込めていません）"], over: false };
  const perTurn = man(hint);
  const lines = [
    `通常 ${e.turnsTypical} turn／最大 ${e.turnsMax} turn（形式修正込み）。1 turn あたり約${perTurn}万トークンの土台があるため、最大でおよそ ${e.turnsMax}×${perTurn}万 ≒ ${man(e.tokensMax)}万トークン（目安。実際の使用量ではありません）。`,
  ];
  const over = e.turnsMax > maxTurns;
  if (over) lines.push(`最大 ${e.turnsMax} turn は、turn 上限（${maxTurns}）を超えます。途中で turn 上限に達して止まる可能性があります。`);
  return { lines, over };
}

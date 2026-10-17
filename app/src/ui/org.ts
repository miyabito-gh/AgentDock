// 組織実行の表示用導出（P-ORG-7b）。状態の判定はホスト。ここは並べ替え・文言・色クラスの対応だけ。
// 状態は文字とアイコン名（ホストの label／icon）で必ず示し、色だけで区別させない。
import type { Chat, HostSnapshot, OrgConfirmCode, OrgEffect, OrgTaskPhase, OrgTaskView, OrgWorkflowView, PendingRequest } from "../ipc/types";
import { chatRequests } from "./derive";
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
  if (w.halt) return { label: w.halt.label, icon: w.halt.icon, tone: w.halt.kind === "sandboxUnhealthy" ? "fail" : "wait", attention, planWaiting };
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

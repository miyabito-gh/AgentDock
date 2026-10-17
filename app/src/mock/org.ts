// 組織実行の仮データ（モック表示用。製品の挙動ではない）。型は ipc/types.ts に適合させる。
import type { OrgConfirmView, OrgRolesView, OrgSnapshotView, OrgStatusView, OrgTaskView, OrgWorkflowView } from "../ipc/types";

const st = (phase: OrgStatusView["phase"], label: string, icon: string, reason: OrgStatusView["reason"] = null): OrgStatusView => ({ phase, reason, label, icon });
const ok: OrgConfirmView = { state: "confirmed", cause: null, label: "確認済み", icon: "shield-check" };

const task = (id: string, role: string, title: string, over: Partial<OrgTaskView>): OrgTaskView => ({
  id, role, title, deps: [], waitingOn: [], status: st("pending", "待機中", "circle"), confirm: ok, attempts: 1, maxAttempts: 3, chat: null, commit: null,
  effects: [], stallNotified: false, reconcile: null, canCancel: true, canAcceptManual: false, needsChatChoice: false, canRetry: false, ...over,
});

const base = (over: Partial<OrgWorkflowView>): OrgWorkflowView => ({
  workflowId: "wf-1", title: "再接続処理の見直し", request: "再接続処理を見直し、バックオフとテストを追加する。既存の挙動は変えない。",
  repo: "C:\Users\wmasa\Documents\Rust\Sample", halt: null,
  limits: { maxWriters: 2, maxTotal: 3, maxRework: 2, maxAutoReplans: 1, maxTurns: 30, maxTasks: 20, stagnationMinutes: 30 },
  usage: { turnsUsed: 11, turnsRemaining: 19, autoReplansUsed: 0, knownTokens: 184_300, rows: 6, rowsWithoutTokens: 1 },
  tasks: [], planApproval: null, sandboxNotice: null, trust: null, notices: [], dispatching: true, finished: false, ...over,
});

const RUN_TASKS: OrgTaskView[] = [
  task("T1", "worker", "再接続のバックオフを実装する", { status: st("completed", "完了", "check"), attempts: 1, commit: "9f3c2a1be07d", canCancel: false }),
  task("T2", "reviewer", "T1 の差分をレビューする", { deps: ["T1"], status: st("running", "実行中", "play"), chat: { backend: "codex", id: "org-t2" } }),
  task("T3", "worker", "バックオフのテストを追加する", { deps: ["T1"], status: st("rework", "差し戻し中", "rotate"), attempts: 2, chat: { backend: "codex", id: "org-t3" } }),
  task("T4", "worker", "設定の読み込みを更新する", { deps: ["T1"], status: st("blocked", "止まっています（要対応）", "pause", "needsAttention"), confirm: { state: "sendUnknown", cause: "restart", label: "受理不明", icon: "help-circle" }, chat: { backend: "codex", id: "org-t4" }, stallNotified: true,
    reconcile: { task: "T4", turnRead: true, turnEnd: null, worktree: { kind: "uncommitted", entries: 3, label: "未コミットの変更 3件" }, text: "履歴で終端を確認できていません" } }),
  task("T5", "reviewer", "全体を最終レビューする", { deps: ["T2", "T3", "T4"], waitingOn: ["T2", "T3", "T4"], status: st("pending", "待機中", "hourglass") }),
  task("T6", "worker", "古い設計メモを整理する", { status: st("failed", "失敗（実行エラー）", "x", "executionFailed"), attempts: 3, canCancel: false, confirm: { state: "needsReconcile", cause: "disconnect", label: "要照合", icon: "refresh" }, canAcceptManual: true }),
  task("T7", "worker", "不要な試作を取り消す", { status: st("cancelled", "取消し", "ban"), canCancel: false }),
];

const PLAN_TASKS: OrgTaskView[] = [
  task("T1", "worker", "依存パッケージを更新する", { effects: ["networkOrInstall"] }),
  task("T2", "worker", "ビルド設定を書き換える", { deps: ["T1"], waitingOn: ["T1"], effects: ["userTreeWrite"] }),
  task("T3", "reviewer", "差分をレビューする", { deps: ["T2"], waitingOn: ["T2"] }),
];

export function mockOrgView(scenario: string): OrgSnapshotView {
  if (scenario === "orgPlan") {
    return { stallMinutes: 30, workflow: base({ tasks: PLAN_TASKS, dispatching: false, halt: { kind: "awaitingPlanApproval", label: "計画の承認待ち", icon: "eye", resumable: false },
      usage: { turnsUsed: 1, turnsRemaining: 29, autoReplansUsed: 0, knownTokens: null, rows: 1, rowsWithoutTokens: 1 },
      planApproval: { reasonsKnown: true, reasons: [{ task: "T1", text: "ネットワーク取得・パッケージ導入を含みます" }, { task: "T2", text: "あなたの作業ツリー・ブランチへの書込みを含みます" }] } }) };
  }
  if (scenario === "orgRun") {
    return { stallMinutes: 30, workflow: base({ tasks: RUN_TASKS, dispatching: false,
      halt: { kind: "restart", label: "再起動後の照合済み（再開待ち）", icon: "pause", resumable: true },
      trust: { kind: "codexTrustAppended", severity: "info", text: "Codex が作業フォルダを信頼済みとして ~/.codex/config.toml に追記しました。", projects: ["C:\Users\wmasa\AppData\Local\com.agentdock.app\worktrees\wf-1-T1"] },
      notices: [{ serial: 1, kind: "taskBlocked", severity: "attention", task: "T4", text: "受理を確認できないため止まっています" }, { serial: 2, kind: "reconciled", severity: "info", task: null, text: "再起動後の照合を終えました" }] }) };
  }
  if (scenario === "orgDone") {
    return { stallMinutes: 30, workflow: base({ tasks: RUN_TASKS.filter((t) => t.status.phase === "completed"), dispatching: false, finished: true }) };
  }
  return { stallMinutes: 30, workflow: null };
}

export function mockOrgRoles(): OrgRolesView {
  return { loaded: true, created: [], problems: [], roles: [
    { id: "planner", displayName: "Planner", file: "planner.toml", enabled: true, widened: [], confirmed: true, fingerprint: "fp-planner", sandbox: "read-only", approval: "never", workplace: "読取り用 worktree", allowNetwork: false, extraWritableDirs: [], model: null, effort: null, maxConcurrent: 1 },
    { id: "worker", displayName: "Worker", file: "worker.toml", enabled: false, widened: ["ネットワークを許可しています"], confirmed: false, fingerprint: "fp-worker", sandbox: "workspace-write", approval: "never", workplace: "専用 worktree", allowNetwork: true, extraWritableDirs: [], model: null, effort: "high", maxConcurrent: 2 },
  ] };
}

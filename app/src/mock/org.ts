// 組織実行の仮データ（モック表示用。製品の挙動ではない）。型は ipc/types.ts に適合させる。
import type { OrgConfirmView, OrgGraphNodeView, OrgGraphView, OrgGraphsView, OrgRolesView, OrgSnapshotView, OrgStatusView, OrgTaskView, OrgWorkflowView } from "../ipc/types";

const st = (phase: OrgStatusView["phase"], label: string, icon: string, reason: OrgStatusView["reason"] = null): OrgStatusView => ({ phase, reason, label, icon });
const ok: OrgConfirmView = { state: "confirmed", cause: null, label: "確認済み", icon: "shield-check" };

const task = (id: string, role: string, title: string, over: Partial<OrgTaskView>): OrgTaskView => ({
  id, role, title, deps: [], waitingOn: [], status: st("pending", "待機中", "circle"), confirm: ok, attempts: 1, maxAttempts: 3, chat: null, commit: null,
  effects: [], stallNotified: false, reconcile: null, canCancel: true, canAcceptManual: false, needsChatChoice: false, canRetry: false, retry: null, retryRefusal: null, canFail: false, canAdoptManual: false, attentionNote: null,
  since: null, interject: { route: "nextTurn", label: "次にこのタスクへ送るとき（差し戻し・再送など）に添えます。このタスクにもう送信がなければ届きません" }, skip: null, skipRefusal: null, canSendBack: false, sendBackNeedsNote: true, sendBackRefusal: null, canAcceptAsIs: false, canHold: false, held: false, notesPending: 0, ...over,
});

const base = (over: Partial<OrgWorkflowView>): OrgWorkflowView => ({
  workflowId: "wf-1", title: "再接続処理の見直し", request: "再接続処理を見直し、バックオフとテストを追加する。既存の挙動は変えない。",
  repo: "C:\Users\wmasa\Documents\Rust\Sample", halt: null, graph: null,
  limits: { maxWriters: 2, maxTotal: 3, maxRework: 2, maxAutoReplans: 1, maxTurns: 30, maxTasks: 20, stagnationMinutes: 30, maxRetries: 3 },
  usage: { turnsUsed: 11, turnsRemaining: 19, autoReplansUsed: 0, knownTokens: 184_300, rows: 6, rowsWithoutTokens: 1 },
  tasks: [], planApproval: null, sandboxNotice: null, trust: null, notices: [], dispatching: true, finished: false, readonly: false, archivedAt: null, deletion: null, notes: [], interjects: [], notesAfterPlan: 0, artifactHint: null, ...over,
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
  const g = graphScenario(scenario);
  if (g) return g;
  if (scenario === "orgPlan") {
    return { stallMinutes: 30, history: [], archived: [], workflow: base({ tasks: PLAN_TASKS, dispatching: false, halt: { kind: "awaitingPlanApproval", label: "計画の承認待ち", icon: "eye", resumable: false },
      usage: { turnsUsed: 1, turnsRemaining: 29, autoReplansUsed: 0, knownTokens: null, rows: 1, rowsWithoutTokens: 1 },
      planApproval: { reasonsKnown: true, reasons: [{ task: "T1", text: "ネットワーク取得・パッケージ導入を含みます" }, { task: "T2", text: "あなたの作業ツリー・ブランチへの書込みを含みます" }] } }) };
  }
  if (scenario === "orgRun") {
    return { stallMinutes: 30, history: [], archived: [], workflow: base({ tasks: RUN_TASKS, dispatching: false,
      halt: { kind: "restart", label: "再起動後の照合済み（再開待ち）", icon: "pause", resumable: true },
      trust: { kind: "codexTrustAppended", severity: "info", text: "Codex が作業フォルダを信頼済みとして ~/.codex/config.toml に追記しました。", projects: ["C:\Users\wmasa\AppData\Local\com.agentdock.app\worktrees\wf-1-T1"] },
      notices: [{ serial: 1, kind: "taskBlocked", severity: "attention", task: "T4", text: "受理を確認できないため止まっています" }, { serial: 2, kind: "reconciled", severity: "info", task: null, text: "再起動後の照合を終えました" }] }) };
  }
  if (scenario === "orgDone") {
    return { stallMinutes: 30, history: [], archived: [], workflow: base({ tasks: RUN_TASKS.filter((t) => t.status.phase === "completed"), dispatching: false, finished: true }) };
  }
  return { stallMinutes: 30, history: [], archived: [], workflow: null };
}

// グラフで動くワークフロー（品質向上周回）。verify が rework を返し、build が2周目。
const qnode = (id: string, label: string, roleId: string | null, over: Partial<OrgGraphNodeView>): OrgGraphNodeView => ({
  id, label, kind: "agent", roleId, task: null, status: null, confirm: null, round: null, maxRound: null, lastVerdict: null, gate: null, roundLimit: null, ...over,
});
const gtask = (id: string, role: string, title: string, over: Partial<OrgTaskView>): OrgTaskView => task(id, role, title, { maxAttempts: 1, ...over });
const quality = (nodes: OrgGraphNodeView[], used: number, history: Array<"rework" | "pass">): OrgGraphView => ({
  graphId: "quality-loop", name: "計画→実装→検証を周回（品質向上）", nodes,
  edges: [
    { from: "plan", to: "build", kind: "forward", label: "常に", used: 1, allowed: null },
    { from: "build", to: "verify", kind: "forward", label: "常に", used: 1, allowed: null },
    { from: "verify", to: "build", kind: "feedback", label: "判定 = やり直し", used, allowed: 3 },
    { from: "verify", to: "plan", kind: "feedback", label: "判定 = 計画からやり直し", used: 0, allowed: 1 },
  ],
  rounds: [
    { edge: "verify->build", from: "verify", to: "build", used, maxRounds: 3, extra: 0, onExhausted: "ask", history: history.map((h, i) => ({ index: i + 1, verdict: { kind: "value", text: h } })) },
    { edge: "verify->plan", from: "verify", to: "plan", used: 0, maxRounds: 1, extra: 0, onExhausted: "ask", history: [] },
  ],
});
const QUALITY_TASKS: OrgTaskView[] = [
  gtask("plan-1", "planner", "plan", { status: st("completed", "完了", "check"), canCancel: false }),
  gtask("build-1", "worker", "build", { deps: ["plan-1"], status: st("running", "実行中", "play"), attempts: 2 }),
  gtask("verify-1", "verifier", "verify", { deps: ["build-1"], status: st("reviewing", "レビュー中", "eye") }),
];
const qn = (t: OrgTaskView, id: string, label: string, role: string | null, over: Partial<OrgGraphNodeView> = {}) => qnode(id, label, role, { task: t.id, status: t.status, confirm: t.confirm, ...over });

export function mockOrgGraphs(): OrgGraphsView {
  const est = (a: number) => ({ activations: a, turnsTypical: a, turnsMax: a * 2, tokensMax: a * 2 * 17000 });
  return {
    loaded: true, created: [], defaultMaxTurns: 30, maxTurnsLimit: 200, tokensPerTurnHint: 17000,
    templates: [
      { id: "standard", name: "計画して実行（標準）", description: "Planner が依存関係つきのタスク計画を作り、Worker と Reviewer が実行・検証する。", workspace: "git", roles: ["planner", "reviewer", "worker"], file: "standard.json", selectable: true, reasons: [], estimate: est(62), suggestedMaxTurns: null },
      { id: "quality-loop", name: "計画→実装→検証を周回（品質向上）", description: "検証で差し戻されたら実装をやり直し、方針の誤りなら計画から作り直す。", workspace: "git", roles: ["planner", "verifier", "worker"], file: "quality-loop.json", selectable: true, reasons: [], estimate: est(12), suggestedMaxTurns: 40 },
      { id: "folder-draft", name: "フォルダで下書き", description: "Git を使わない下書き用。", workspace: "folder", roles: ["writer"], file: "folder-draft.json", selectable: false, reasons: ["Git を使わない（folder）テンプレートには、まだ対応していません"], estimate: est(4), suggestedMaxTurns: null },
    ],
    problems: [{ file: "broken.json", reasons: ["V4: 辺 verify→build は後ろへ戻る辺ですが、feedback の指定がありません"] }],
  };
}

function graphScenario(scenario: string): OrgSnapshotView | null {
  if (scenario === "orgGraph") {
    const [p, b, v] = QUALITY_TASKS;
    const graph = quality([
      qn(p, "plan", "計画", "planner"),
      qn(b, "build", "実装", "worker", { round: 2, maxRound: 4 }),
      qn(v, "verify", "検証", "verifier", { round: 2, maxRound: 4, lastVerdict: { kind: "value", text: "rework" } }),
    ], 1, ["rework"]);
    return { stallMinutes: 30, history: [], archived: [], workflow: base({ title: "品質向上の依頼", tasks: QUALITY_TASKS, graph }) };
  }
  if (scenario === "orgLimit") {
    const tasks = [QUALITY_TASKS[0], { ...QUALITY_TASKS[1], status: st("blocked", "止まっています（やり直しの上限）", "pause", "roundLimit") }, QUALITY_TASKS[2]];
    const graph = quality([
      qn(tasks[0], "plan", "計画", "planner"),
      qn(tasks[1], "build", "実装", "worker", { round: 4, maxRound: 4, roundLimit: { edge: "verify->build", used: 3, allowed: 3, awaiting: true } }),
      qn(tasks[2], "verify", "検証", "verifier", { round: 4, maxRound: 4, lastVerdict: { kind: "value", text: "rework" } }),
    ], 3, ["rework", "rework", "rework"]);
    return { stallMinutes: 30, history: [], archived: [], workflow: base({ title: "品質向上の依頼", tasks, graph }) };
  }
  if (scenario === "orgGate") {
    const tasks = [
      gtask("draft-1", "planner", "draft", { status: st("completed", "完了", "check"), canCancel: false }),
      gtask("confirm-1", "gate", "confirm", { deps: ["draft-1"], status: st("blocked", "止まっています（承認待ち）", "pause", "awaitingApproval") }),
    ];
    const graph: OrgGraphView = {
      graphId: "gated", name: "下書きを確認して進める", nodes: [
        qn(tasks[0], "draft", "draft", "planner"),
        qn(tasks[1], "confirm", "confirm", null, {
          kind: "gate", lastVerdict: { kind: "pending" },
          gate: {
            prompt: "この下書きで進めてよいですか？", awaiting: true,
            choices: [{ id: "ok", label: "このまま進める", needsNote: false }, { id: "fix", label: "直してほしい（理由を書く）", needsNote: true }],
            shows: [{ slot: "spec", text: "{\n  \"goal\": \"再接続のバックオフ\"\n}", truncated: false }],
          },
        }),
      ],
      edges: [
        { from: "draft", to: "confirm", kind: "forward", label: "常に", used: 1, allowed: null },
        { from: "confirm", to: "draft", kind: "feedback", label: "choice = fix", used: 0, allowed: 2 },
      ],
      rounds: [{ edge: "confirm->draft", from: "confirm", to: "draft", used: 0, maxRounds: 2, extra: 0, onExhausted: "ask", history: [] }],
    };
    return { stallMinutes: 30, history: [], archived: [], workflow: base({ title: "下書きの確認", tasks, graph }) };
  }
  return null;
}

export function mockOrgRoles(): OrgRolesView {
  return { loaded: true, created: [], problems: [], roles: [
    { id: "planner", displayName: "Planner", file: "planner.toml", enabled: true, widened: [], confirmed: true, fingerprint: "fp-planner", sandbox: "read-only", approval: "never", workplace: "読取り用 worktree", allowNetwork: false, extraWritableDirs: [], model: null, effort: null, maxConcurrent: 1 },
    { id: "worker", displayName: "Worker", file: "worker.toml", enabled: false, widened: ["ネットワークを許可しています"], confirmed: false, fingerprint: "fp-worker", sandbox: "workspace-write", approval: "never", workplace: "専用 worktree", allowNetwork: true, extraWritableDirs: [], model: null, effort: "high", maxConcurrent: 2 },
  ] };
}

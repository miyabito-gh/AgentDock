// 同等性の操作の台帳（段階③、DESIGN_P3 §1 #16）。メニュー・主要ボタン・Ctrl+K コマンドメニューは、ここから作る（P3-8）。
// 規則: 能力が非対応・未実装の操作は使えない扱い（理由を出す）。Unverified は「未確認」を付け、結果は観測した事実だけを書く。
//       状態は snapshot から導く。成功を表示する文言はここに置かない。
import type { OpCapability, OpRoute, ParityOp, Support, Verification } from "../ipc/types";

export const OP_LABEL: Record<ParityOp, string> = {
  changeList: "変更ファイルと差分の表示",
  revertChanges: "変更を戻す",
  codeReview: "コードレビュー",
  reviewToNewChat: "レビュー結果を別チャットへ",
  workMode: "計画／実行の切替",
  fork: "会話の分岐",
  compact: "文脈の圧縮",
  referenceChat: "過去の会話を参考指定",
  goal: "Goal",
  sideChat: "side相談",
  skills: "Skill の指定",
  instructionFiles: "指示ファイル（AGENTS.md）",
  toolServers: "MCP",
  extensions: "Plugins",
  cloudDelegation: "クラウド委任",
  worktree: "Git worktree",
  backendStatus: "Codex の状態",
  speedTier: "速度（Fast）",
  personality: "personality",
  memory: "memories",
};

export const ROUTE_TEXT: Record<OpRoute, string> = {
  backendApi: "AI の API",
  appManaged: "AgentDock 管理",
  cliHelper: "CLI 補助",
};

export const SUPPORT_TEXT: Record<Support, string> = {
  supported: "対応",
  experimental: "experimental（接続時の有効化が必要）",
  unsupported: "非対応・未実装",
  unknown: "不明",
};

export const VERIFY_TEXT: Record<Verification, string> = {
  verified: "実機で確認済み",
  unverified: "未確認",
};

/** 操作の可否。無効のときは必ず理由を持つ。 */
export type Enabled = { kind: "enabled"; unverified: boolean } | { kind: "disabled"; reason: string };

/** 可否の判断材料（snapshot から導く）。 */
export interface CommandContext {
  /** 操作ごとの能力。取れていなければ undefined。 */
  cap: OpCapability | undefined;
  connected: boolean;
  /** 選択中のチャットがある。 */
  hasChat: boolean;
  /** 作業中・停止未確認。 */
  busy: boolean;
  deletePending: boolean;
  /** 外部で実行中（閲覧のみ）。 */
  externalRunning: boolean;
}

export interface ParityCommand {
  id: string;
  label: string;
  op: ParityOp;
  /** チャットを選んでいる必要があるか。 */
  needsChat: boolean;
  /** 実行中のチャットでは使えないか（状態を変える操作）。 */
  blockedWhileBusy: boolean;
  enabled: (ctx: CommandContext) => Enabled;
}

const disabled = (reason: string): Enabled => ({ kind: "disabled", reason });

/** 共通の可否。能力 → 接続 → チャット → 削除保留 → 外部実行中 → 作業中の順に、最初に当たった理由を返す。 */
export function availability(c: Pick<ParityCommand, "needsChat" | "blockedWhileBusy">, ctx: CommandContext): Enabled {
  const cap = ctx.cap;
  if (!cap) return disabled("この AI の対応状況を取得できていません");
  if (cap.deprecated) return disabled(cap.note ?? "非推奨のため使えません");
  if (cap.support === "unsupported") return disabled(cap.note ?? "このAIでは非対応です");
  if (cap.support === "unknown") return disabled("対応しているか確認できません");
  if (!ctx.connected) return disabled("接続されていません");
  if (c.needsChat && !ctx.hasChat) return disabled("チャットを選んでください");
  if (ctx.deletePending) return disabled("削除保留中のため使えません");
  if (ctx.externalRunning) return disabled("外部で実行中の会話は閲覧のみです");
  if (c.blockedWhileBusy && ctx.busy) return disabled("作業中のため使えません（完了または停止の確認後に実行できます）");
  return { kind: "enabled", unverified: cap.verification === "unverified" };
}

const cmd = (id: string, op: ParityOp, needsChat: boolean, blockedWhileBusy: boolean, label = OP_LABEL[op]): ParityCommand => {
  const base = { needsChat, blockedWhileBusy };
  return { id, label, op, ...base, enabled: (ctx) => availability(base, ctx) };
};

/** 台帳。順序はメニュー「作業」の並び。 */
export const PARITY_COMMANDS: ParityCommand[] = [
  cmd("changeList", "changeList", true, false, "差分を確認"),
  cmd("revertChanges", "revertChanges", true, true, "変更を戻す…"),
  cmd("codeReview", "codeReview", true, true, "コードレビュー…"),
  cmd("reviewToNewChat", "reviewToNewChat", true, true, "レビューを別チャットで…"),
  cmd("workMode", "workMode", true, false, "計画／実行の切替"),
  cmd("fork", "fork", true, false, "会話を分岐"),
  cmd("compact", "compact", true, true, "文脈を圧縮"),
  cmd("referenceChat", "referenceChat", true, false, "過去の会話を参考に…"),
  cmd("goal", "goal", true, false, "Goal を設定…"),
  cmd("sideChat", "sideChat", true, false, "side相談を開く"),
  cmd("skills", "skills", true, false, "Skill を指定…"),
  cmd("instructionFiles", "instructionFiles", true, false, "指示ファイルを確認"),
  cmd("toolServers", "toolServers", false, false, "MCP を確認…"),
  cmd("extensions", "extensions", false, false, "Plugins を管理…"),
  cmd("cloudDelegation", "cloudDelegation", true, false, "クラウドに委任…"),
  cmd("worktree", "worktree", false, false, "worktree で開始…"),
  cmd("backendStatus", "backendStatus", false, false, "Codex の状態"),
  cmd("speedTier", "speedTier", true, false, "速度（Fast）"),
  cmd("personality", "personality", false, false),
  cmd("memory", "memory", false, false, "memories"),
];

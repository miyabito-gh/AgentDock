// 同等性の操作の台帳（段階③、DESIGN_P3 §1 #16）。メニュー・主要ボタン・Ctrl+K コマンドメニューは、ここから作る（P3-8）。
// 規則: 能力が非対応・未実装の操作は使えない扱い（理由を出す）。Unverified は「未確認」を付け、結果は観測した事実だけを書く。
//       状態は snapshot から導く。成功を表示する文言はここに置かない。
import type { Chat, HostSnapshot, OpCapability, OpRoute, ParityOp, Support, Verification } from "../ipc/types";
import { chatStops, isRunning, rootView, stopOpen } from "./derive";

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
  /** Git が使えないと確認できている（確認できていないときは false。実行時にホストが拒否する）。 */
  gitMissing: boolean;
}

/** snapshot から可否の判断材料を作る（表示のために接続・再開・読取りを増やさない）。 */
export function commandContext(snap: HostSnapshot, chat: Chat | null, connected: boolean, op: ParityOp | null, gitMissing = false): CommandContext {
  const stopped = chat ? chatStops(snap, chat).some(stopOpen) : false;
  const local = chat ? snap.chatLocals.find((l) => l.chat.id === chat.key.id) : undefined;
  return {
    cap: op ? snap.opCapabilities.find((c) => c.op === op) : undefined,
    connected,
    hasChat: !!chat,
    busy: chat ? isRunning(snap, chat) || stopped : false,
    deletePending: !!local?.deletePending,
    externalRunning: !!chat && chat.origin === "external" && rootView(snap, chat)?.freshness !== "live",
    gitMissing,
  };
}

export interface ParityCommand {
  id: string;
  label: string;
  /** 対応する同等性の操作。能力に紐づかない操作（中断）は null。 */
  op: ParityOp | null;
  /** App の act() へ渡す操作名。 */
  act: string;
  /** 上部メニュー「作業」に出すか（出さないものは Ctrl+K だけ）。 */
  menu: boolean;
  /** 中央ヘッダーの主要ボタンの文字（なければ出さない）。 */
  header?: string;
  kbd?: string;
  /** チャットを選んでいる必要があるか。 */
  needsChat: boolean;
  /** 実行中のチャットでは使えないか（状態を変える操作）。 */
  blockedWhileBusy: boolean;
  /** Git が必要か（確認できているときだけ「Gitなし」で無効にする）。 */
  needsGit: boolean;
  enabled: (ctx: CommandContext) => Enabled;
}

const disabled = (reason: string): Enabled => ({ kind: "disabled", reason });

/** 共通の可否。能力 → 接続 → チャット → 削除保留 → 外部実行中 → 作業中 → Git の順に、最初に当たった理由を返す。 */
export function availability(c: Pick<ParityCommand, "needsChat" | "blockedWhileBusy" | "needsGit">, ctx: CommandContext): Enabled {
  const cap = ctx.cap;
  if (!cap) return disabled("この AI の対応状況を取得できていません");
  if (cap.deprecated) return disabled(cap.note ?? "非推奨のため使えません");
  if (cap.support === "unsupported") return disabled(cap.note ?? "このAIでは非対応です");
  if (cap.support === "experimental") return disabled(cap.note ?? "experimental API が無効のため使えません");
  if (cap.support === "unknown") return disabled("対応しているか確認できません");
  if (!ctx.connected) return disabled("接続されていません");
  if (c.needsChat && !ctx.hasChat) return disabled("チャットを選んでください");
  if (ctx.deletePending) return disabled("削除保留中のため使えません");
  if (ctx.externalRunning) return disabled("外部で実行中の会話は閲覧のみです");
  if (c.blockedWhileBusy && ctx.busy) return disabled("作業中のため使えません（完了または停止の確認後に実行できます）");
  // チャットを必要とする操作だけ（worktree の新規開始は別のフォルダを選べるので、選択中チャットのGitでは止めない）。
  if (c.needsGit && c.needsChat && ctx.gitMissing) return disabled("Gitリポジトリではありません");
  return { kind: "enabled", unverified: cap.verification === "unverified" };
}

interface Opt { label: string; menu?: boolean; header?: string; kbd?: string; git?: boolean }
const cmd = (id: string, op: ParityOp, act: string, needsChat: boolean, blockedWhileBusy: boolean, o: Opt): ParityCommand => {
  const base = { needsChat, blockedWhileBusy, needsGit: !!o.git };
  return { id, op, act, label: o.label, menu: !!o.menu, header: o.header, kbd: o.kbd, ...base, enabled: (ctx) => availability(base, ctx) };
};

/** 台帳。順序は上部メニュー「作業」の並び（モックの順）。メニュー・主要ボタン・Ctrl+K はすべてここから作る。 */
export const PARITY_COMMANDS: ParityCommand[] = [
  cmd("changeList", "changeList", "diff", true, false, { label: "差分を確認", menu: true, header: "差分", git: true }),
  cmd("revertChanges", "revertChanges", "revert", true, true, { label: "変更を戻す…", menu: true, header: "戻す", git: true }),
  cmd("codeReview", "codeReview", "review", true, true, { label: "コードレビュー…", menu: true, header: "レビュー", git: true }),
  cmd("reviewToNewChat", "reviewToNewChat", "review", true, true, { label: "レビューを別チャットで…", git: true }),
  cmd("workMode", "workMode", "workMode", true, false, { label: "計画／実行の切替", menu: true }),
  cmd("fork", "fork", "fork", true, false, { label: "会話を分岐", menu: true, header: "分岐" }),
  cmd("compact", "compact", "compact", true, true, { label: "文脈を圧縮…", menu: true }),
  cmd("goal", "goal", "goal", true, false, { label: "Goal を設定…", menu: true, header: "Goal" }),
  cmd("referenceChat", "referenceChat", "reference", true, false, { label: "過去の会話を参考に…", menu: true }),
  cmd("sideChat", "sideChat", "sideOpen", true, false, { label: "side相談を開く", menu: true }),
  cmd("skills", "skills", "skills", true, false, { label: "Skill を指定…", menu: true }),
  cmd("instructionFiles", "instructionFiles", "instructions", true, false, { label: "指示ファイル（AGENTS.md）を確認…", menu: true }),
  cmd("worktree", "worktree", "newChatWorktree", false, false, { label: "worktree で開始…", menu: true, git: true }),
  cmd("worktreeManage", "worktree", "worktrees", false, false, { label: "worktree を管理…", menu: true, git: true }),
  cmd("cloudDelegation", "cloudDelegation", "cloud", true, false, { label: "クラウドに委任…", menu: true }),
  // 以下は上部メニュー「作業」には出さず、Ctrl+K と各ダイアログから使う。
  cmd("toolServers", "toolServers", "settings:mcp", false, false, { label: "MCP を確認…" }),
  cmd("extensions", "extensions", "settings:mcp", false, false, { label: "Plugins を管理…" }),
  cmd("backendStatus", "backendStatus", "status", false, false, { label: "Codex の状態" }),
  cmd("speedTier", "speedTier", "status", true, false, { label: "速度（Fast）の設定を確認" }),
  cmd("memory", "memory", "status", false, false, { label: "memories の設定を確認" }),
  cmd("personality", "personality", "status", false, false, { label: "personality の状態を確認" }),
  {
    id: "interrupt", label: "中断", op: null, act: "interrupt", menu: true, kbd: "Esc", needsChat: true, blockedWhileBusy: false, needsGit: false,
    enabled: (ctx) => !ctx.connected ? disabled("接続されていません")
      : !ctx.hasChat ? disabled("チャットを選んでください")
        : ctx.externalRunning ? disabled("外部で実行中の会話は閲覧のみです")
          : !ctx.busy ? disabled("作業中ではありません") : { kind: "enabled", unverified: false },
  },
];

/** 画面に出す1操作（台帳の項目と、いまの可否）。 */
export interface CommandView { cmd: ParityCommand; state: Enabled }

/** 3つの入口（メニュー・ヘッダー・Ctrl+K）が使う可否の一覧。能力が非対応のものは「このAIでは非対応」の理由つきで無効になる。 */
export const commandViews = (snap: HostSnapshot, chat: Chat | null, connected: boolean, gitMissing = false): CommandView[] =>
  PARITY_COMMANDS.map((c) => ({ cmd: c, state: c.enabled(commandContext(snap, chat, connected, c.op, gitMissing)) }));

/** 検索語に合う操作（ラベル・操作名の部分一致、大文字小文字は区別しない。空なら全件）。 */
export const filterCommands = (views: CommandView[], q: string): CommandView[] => {
  const w = q.trim().toLowerCase();
  if (!w) return views;
  return views.filter((v) => v.cmd.label.toLowerCase().includes(w) || (v.cmd.op !== null && OP_LABEL[v.cmd.op].toLowerCase().includes(w)));
};

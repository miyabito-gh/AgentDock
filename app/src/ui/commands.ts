// 同等性の操作の台帳（段階③、DESIGN_P3 §1 #16）。メニュー・主要ボタン・Ctrl+K コマンドメニューは、ここから作る（P3-8）。
// 規則: 能力が非対応・未実装の操作は使えない扱い（理由を出す）。Unverified は「未確認」を付け、結果は観測した事実だけを書く。
//       状態は snapshot から導く。成功を表示する文言はここに置かない。
// 台帳は上部メニュー・ヘッダー「…」・Ctrl+K のすべての出典（DESIGN_MENUS §3.4）。メニューの並びは menuModel.ts、snapshot からの組立ては commandViews.ts。
// このファイルは型以外では shortcuts.ts だけに依存する（node --test でそのまま読めるように）。
import type { OpCapability, OpRoute, ParityOp, Support, Verification } from "../ipc/types";
import { kbdForAct } from "./shortcuts.ts";

export const OP_LABEL: Record<ParityOp, string> = {
  changeList: "変更の確認",
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
  sandboxCheck: "sandbox の確認",
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
  /** ピン留め中。 */
  pinned: boolean;
  /** アーカイブ済み（アプリの一覧から隠している場合を含む）。 */
  archived: boolean;
  /** 一般チャット（作業フォルダなし）。 */
  isGeneral: boolean;
  /** 選択中チャットの親が live。 */
  isLive: boolean;
  /** 作業フォルダを開けない理由（ヘッダーのボタンと同じ判定の結果。開けるなら null）。呼び出し側が渡す。 */
  cwdWhy: string | null;
  /** 次の送信から使う作業フォルダの数（取れていなければ 0。2以上のとき項目名に件数を添える）。呼び出し側が渡す。 */
  folderCount: number;
  /** オン／オフ表示の状態（act 名をキーにする。例: toggleLeft）。呼び出し側が渡す。 */
  checked: Record<string, boolean>;
  /** 設定「継続チャット」がオフ（目的メモ・引き継ぎは使えない）。 */
  continuationOff?: boolean;
  /** 目的メモの更新案を作れない理由（画面で分かる分。作れるなら null／省略）。 */
  draftBlock?: string | null;
}

/** 上部メニューの5つ（Ctrl+K の所属表示にも使う）。 */
export type MenuGroup = "チャット" | "作業" | "表示" | "ツール" | "ヘルプ";

export interface Command {
  id: string;
  /** 同じ操作はどこでも同じ文字列（状態で変わるものは dynamicLabel）。 */
  label: string;
  /** 対応する同等性の操作。能力に紐づかない操作は null。 */
  op: ParityOp | null;
  /** App の act() へ渡す操作名。 */
  act: string;
  /** 所属メニュー。 */
  group: MenuGroup;
  /** ショートカット表示。shortcuts.ts から引く（ハンドラーがあるものだけ）。 */
  kbd?: string;
  /** チャットを選んでいる必要があるか。 */
  needsChat: boolean;
  /** 実行中のチャットでは使えないか（状態を変える操作）。 */
  blockedWhileBusy: boolean;
  /** Git が必要か（確認できているときだけ「Gitなし」で無効にする）。 */
  needsGit: boolean;
  /** 危険操作（赤。メニューの最下段に置く）。 */
  danger?: boolean;
  /** Ctrl+K の検索用の別名。 */
  keywords?: string[];
  /** 状態に応じた文言（ピン留め／ピン留めを外す など）。 */
  dynamicLabel?: (ctx: CommandContext) => string;
  /** 状態に応じた操作名（アーカイブ／アーカイブを解除）。 */
  dynamicAct?: (ctx: CommandContext) => string;
  /** オン／オフ（✓）。 */
  checked?: (ctx: CommandContext) => boolean;
  enabled: (ctx: CommandContext) => Enabled;
}
/** 旧名（段階③）。 */
export type ParityCommand = Command;

const disabled = (reason: string): Enabled => ({ kind: "disabled", reason });
const ON: Enabled = { kind: "enabled", unverified: false };

/** 共通の可否。能力 → 接続 → チャット → 削除保留 → 外部実行中 → 作業中 → Git の順に、最初に当たった理由を返す。 */
export function availability(c: Pick<Command, "needsChat" | "blockedWhileBusy" | "needsGit">, ctx: CommandContext): Enabled {
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

interface Opt { label: string; group: MenuGroup; /** 接続・能力で止めず常に押せる（可否は開いたダイアログが自分で示す） */ always?: boolean; git?: boolean; keywords?: string[]; checked?: (ctx: CommandContext) => boolean }
const cmd = (id: string, op: ParityOp, act: string, needsChat: boolean, blockedWhileBusy: boolean, o: Opt): Command => {
  const base = { needsChat, blockedWhileBusy, needsGit: !!o.git };
  return { id, op, act, label: o.label, group: o.group, kbd: kbdForAct(act), keywords: o.keywords, checked: o.checked, ...base, enabled: o.always ? () => ON : (ctx) => availability(base, ctx) };
};

/** 台帳（同等性の操作）。順序は現行の上部メニュー「作業」の並び。 */
export const PARITY_COMMANDS: Command[] = [
  cmd("changeList", "changeList", "diff", true, false, { label: "変更を確認…", group: "作業", git: true }),
  cmd("revertChanges", "revertChanges", "revert", true, true, { label: "変更を戻す…", group: "作業", git: true }),
  cmd("codeReview", "codeReview", "review", true, true, { label: "コードレビュー…", group: "作業", git: true }),
  cmd("reviewToNewChat", "reviewToNewChat", "review", true, true, { label: "レビューを別チャットで…", group: "作業", git: true }),
  cmd("workMode", "workMode", "workMode", true, false, { label: "計画／実行を切替", group: "作業", checked: (ctx) => !!ctx.checked.workMode }),
  cmd("fork", "fork", "fork", true, false, { label: "会話を分岐…", group: "作業" }),
  cmd("compact", "compact", "compact", true, true, { label: "文脈を圧縮…", group: "作業" }),
  cmd("goal", "goal", "goal", true, false, { label: "Goal を設定…", group: "作業" }),
  cmd("referenceChat", "referenceChat", "reference", true, false, { label: "過去の会話を参考に…", group: "作業" }),
  cmd("sideChat", "sideChat", "sideOpen", true, false, { label: "side相談を開く", group: "作業" }),
  cmd("skills", "skills", "skills", true, false, { label: "Skill を指定…", group: "作業" }),
  cmd("instructionFiles", "instructionFiles", "instructions", true, false, { label: "指示ファイル（AGENTS.md）を確認…", group: "作業" }),
  cmd("worktree", "worktree", "newChatWorktree", false, false, { label: "worktree で新しいチャット…", group: "チャット", git: true }),
  cmd("worktreeManage", "worktree", "worktrees", false, false, { label: "worktree を管理…", group: "ツール", git: true }),
  cmd("cloudDelegation", "cloudDelegation", "cloud", true, false, { label: "クラウドに委任…", group: "ツール" }),
  // 以下は Ctrl+K と各ダイアログから使う（上部メニューは menuModel の MENUBAR で並べる）。
  // 「MCP を確認…」「Plugins を管理…」は1項目に統合（行き先は同じ設定の MCP・Plugins）。別名は keywords で検索できる。
  cmd("toolServers", "toolServers", "settings:mcp", false, false, { label: "MCP・Plugins…", group: "ツール", always: true, keywords: ["MCP", "Plugins", "プラグイン", "拡張"] }),
  // 速度（Fast）・memories・personality の3件も行き先は同じ「Codex の状態」。能力表の op（OP_LABEL）は残す。
  cmd("backendStatus", "backendStatus", "status", false, false, { label: "Codex の状態…", group: "ツール", always: true, keywords: ["状態", "速度", "Fast", "memories", "メモリ", "personality", "性格"] }),
  cmd("usage", "backendStatus", "usage", false, false, { label: "利用状況…", group: "ツール", always: true, keywords: ["使用量", "残り", "usage"] }),
  {
    id: "interrupt", label: "中断", op: null, act: "interrupt", group: "作業", needsChat: true, blockedWhileBusy: false, needsGit: false,
    enabled: (ctx) => !ctx.connected ? disabled("接続されていません")
      : !ctx.hasChat ? disabled("チャットを選んでください")
        : ctx.externalRunning ? disabled("外部で実行中の会話は閲覧のみです")
          : !ctx.busy ? disabled("作業中ではありません") : ON,
  },
];

interface AppOpt {
  label: string; group: MenuGroup; needsChat?: boolean; danger?: boolean; keywords?: string[];
  /** チャット選択済みの上で、使えない理由があれば返す。 */
  why?: (ctx: CommandContext) => string | null;
  dynamicLabel?: (ctx: CommandContext) => string;
  dynamicAct?: (ctx: CommandContext) => string;
  checked?: (ctx: CommandContext) => boolean;
}
/** 能力表に載らない操作（チャット管理・表示・ツール・ヘルプ）。 */
const app = (id: string, act: string, o: AppOpt): Command => {
  const needsChat = !!o.needsChat;
  return {
    id, op: null, act, label: o.label, group: o.group, kbd: kbdForAct(act), needsChat, blockedWhileBusy: false, needsGit: false,
    danger: o.danger, keywords: o.keywords, dynamicLabel: o.dynamicLabel, dynamicAct: o.dynamicAct, checked: o.checked,
    enabled: (ctx) => {
      if (needsChat && !ctx.hasChat) return disabled("チャットを選んでください");
      const w = o.why?.(ctx);
      return w ? disabled(w) : ON;
    },
  };
};
const notPending = (ctx: CommandContext) => (ctx.deletePending ? "削除保留中のため使えません" : null);
const contOn = (ctx: CommandContext) => (ctx.continuationOff ? "設定「継続チャット」がオフです" : notPending(ctx));

/** チャット管理・表示・ツール・ヘルプの台帳。act 名は既存の App の switch に合わせる（新規は find・cwdChange・toggleMonitor・shortcuts・howto・about）。 */
export const APP_COMMANDS: Command[] = [
  app("newChatQuick", "newChatQuick", { label: "新しいチャット（すぐ始める）", group: "チャット", keywords: ["作成", "新規", "クイック"] }),
  app("newChat", "newChat", { label: "新しいチャット（詳細を指定）…", group: "チャット", keywords: ["作成", "新規", "フォルダ"] }),
  app("subagentRuns", "subagentRuns", { label: "サブエージェントで依頼", group: "チャット", keywords: ["子エージェント", "worktree", "並列", "履歴"] }),
  app("renameChat", "renameChat", { label: "名前を変更…", group: "チャット", needsChat: true, why: notPending }),
  app("togglePin", "togglePin", {
    label: "ピン留め", group: "チャット", needsChat: true,
    dynamicLabel: (ctx) => (ctx.pinned ? "ピン留めを外す" : "ピン留め"), keywords: ["ピン留めを外す", "固定"],
  }),
  // 作業中アーカイブは従来どおり許可（App 側で確認を通す）。
  app("archive", "archiveChat", {
    label: "アーカイブ", group: "チャット", needsChat: true,
    dynamicLabel: (ctx) => (ctx.archived ? "アーカイブを解除" : "アーカイブ"), dynamicAct: (ctx) => (ctx.archived ? "unarchiveChat" : "archiveChat"), keywords: ["アーカイブを解除", "隠す"],
  }),
  app("exportMd", "exportMd", { label: "Markdown にエクスポート…", group: "チャット", needsChat: true, keywords: ["書き出し", "保存"] }),
  app("newChatSameFolder", "newChatSameFolder", {
    label: "同じ作業フォルダで新しいチャット…", group: "チャット", needsChat: true, keywords: ["複製", "同じフォルダ", "新規", "追加フォルダ"],
    why: (ctx) => (ctx.isGeneral ? "一般チャット（作業フォルダなし）です" : null),
  }),
  app("openCwd", "openCwd", {
    label: "作業フォルダを開く", group: "チャット", needsChat: true,
    why: (ctx) => (ctx.isGeneral ? "一般チャット（作業フォルダなし）です" : ctx.cwdWhy),
  }),
  app("cwdChange", "cwdChange", {
    label: "作業フォルダの確認と変更…", group: "チャット", needsChat: true,
    dynamicLabel: (ctx) => (ctx.folderCount > 1 ? `作業フォルダの確認と変更…（${ctx.folderCount}）` : "作業フォルダの確認と変更…"),
    why: (ctx) => (ctx.isGeneral ? "開発チャットだけで使えます" : null),
  }),
  app("refreshList", "refreshList", { label: "一覧を更新", group: "チャット", keywords: ["再読み込み"] }),
  app("deleteChat", "deleteChat", { label: "削除…", group: "チャット", needsChat: true, danger: true, why: (ctx) => (ctx.deletePending ? "削除保留中です" : null) }),
  app("quit", "quit", { label: "AgentDock を終了…", group: "チャット", danger: true, keywords: ["終了", "閉じる"] }),

  // 継続チャット（DESIGN_CONT §7）。引き継げない理由（組織実行・worktree 等）は押したあとの案内行に出す。
  app("purposeNote", "purpose", { label: "目的メモ…", group: "作業", needsChat: true, why: contOn, keywords: ["メモ", "目的", "継続"] }),
  // 下書きダイアログを開くだけ（モデルは呼ばない。作成はダイアログの「作成する」を押したとき）。作業中でも開ける。
  app("purposeDraft", "purposeDraft", {
    label: "目的メモの更新案を作る…", group: "作業", needsChat: true, keywords: ["メモ", "目的", "更新案", "下書き", "継続"],
    why: (ctx) => contOn(ctx) ?? (!ctx.connected ? "接続されていません" : (ctx.draftBlock ?? null)),
  }),
  app("orgIntakeFromChat", "orgIntakeFromChat", {
    label: "この会話から組織実行の依頼を作る…", group: "作業", needsChat: true, keywords: ["組織実行", "依頼整理", "ワークフロー"],
    why: (ctx) => (ctx.isGeneral ? "作業フォルダのあるチャットだけで使えます" : notPending(ctx)),
  }),
  app("handoffThread", "handoff", { label: "新しい thread へ引き継ぐ…", group: "作業", needsChat: true, why: contOn, keywords: ["引き継ぎ", "文脈", "継続", "新しいthread"] }),

  app("reloadHistory", "reloadHistory", {
    label: "状態を再照合", group: "作業", needsChat: true, keywords: ["再確認", "更新"],
    why: (ctx) => (ctx.isLive ? "最新の状態です" : null),
  }),

  app("toggleLeft", "toggleLeft", { label: "チャット一覧", group: "表示", checked: (ctx) => !!ctx.checked.toggleLeft }),
  app("toggleRight", "toggleRight", { label: "エージェントのドック", group: "表示", checked: (ctx) => !!ctx.checked.toggleRight }),
  app("toggleMonitor", "toggleMonitor", { label: "監視窓", group: "表示", checked: (ctx) => !!ctx.checked.toggleMonitor, keywords: ["コンパクト", "ミニ"] }),
  app("toggleMainTop", "toggleMainTop", { label: "通常画面を最前面に", group: "表示", checked: (ctx) => !!ctx.checked.toggleMainTop }),
  app("toggleMiniTop", "toggleMiniTop", { label: "監視窓を最前面に", group: "表示", checked: (ctx) => !!ctx.checked.toggleMiniTop }),
  app("toggleDone", "toggleDone", { label: "終了済みも表示（完了・停止・確認済みの失敗）", group: "表示", checked: (ctx) => !!ctx.checked.toggleDone }),
  app("find", "find", { label: "会話内を検索…", group: "表示", needsChat: true, keywords: ["検索", "find"] }),
  app("palette", "palette", { label: "すべての操作…", group: "表示", keywords: ["コマンド", "メニュー"] }),

  app("skillsHub", "settings:skills", { label: "Skills・AGENTS.md…", group: "ツール", keywords: ["Skill", "AGENTS"] }),
  app("modeMenu", "modeMenu", { label: "接続先の切替…", group: "ツール", keywords: ["モック", "実接続"] }),
  app("settings", "settings:general", { label: "設定…", group: "ツール", keywords: ["codex.exe", "環境設定"] }),

  app("shortcuts", "shortcuts", { label: "キーボードショートカット…", group: "ヘルプ", keywords: ["キー", "ショートカット"] }),
  app("howto", "howto", { label: "状態の見方…", group: "ヘルプ", keywords: ["凡例", "鮮度"] }),
  app("parity", "parity", { label: "Codex との対応状況…", group: "ヘルプ", keywords: ["同等性", "確認状況"] }),
  app("about", "about", { label: "バージョン情報…", group: "ヘルプ", keywords: ["版", "バージョン"] }),
];

/** 全台帳（メニュー・ヘッダー・Ctrl+K はこれを引く）。 */
export const ALL_COMMANDS: Command[] = [...PARITY_COMMANDS, ...APP_COMMANDS];

/** 画面に出す1操作（台帳の項目と、いまの可否）。状態で変わる文言・操作名・✓ は解決済み。 */
/** `mark` は入口の小さな中立の印（例: 「記録できません」）。状態の根拠ではなく、押すと対処の案内へ行ける目印。 */
export interface CommandView { cmd: Command; state: Enabled; mark?: string; label: string; act: string; checked: boolean | undefined }

/** 検索語に合う操作（ラベル・別名・同等性の操作名・所属メニューの部分一致、大文字小文字は区別しない。空なら全件）。 */
export const filterCommands = (views: CommandView[], q: string): CommandView[] => {
  const w = q.trim().toLowerCase();
  if (!w) return views;
  const hit = (s: string) => s.toLowerCase().includes(w);
  return views.filter((v) => hit(v.label) || hit(v.cmd.label) || (v.cmd.keywords ?? []).some(hit) || (v.cmd.op !== null && hit(OP_LABEL[v.cmd.op])) || hit(v.cmd.group));
};

/** 作業フォルダを開けない理由（ヘッダーの作業フォルダボタンと同じ判定。開けるなら null）。 */
export function cwdOpenWhy(cwd: string | null, kind: string | undefined, live: boolean): string | null {
  if (!live) return "モックでは開きません";
  if (!cwd) return "作業フォルダを取得できていません";
  if (/^[\\/]{2}[^?.]/.test(cwd.trim())) return "ネットワークパスは開きません";
  if (kind === "missing") return "フォルダが見つかりません";
  if (kind === "unconfirmed") return "フォルダの有無を確認できていません";
  return null;
}

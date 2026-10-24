// 所有者違い（dubious ownership）のリポジトリを「AgentDockのgit操作だけに許可」する案内の純粋ロジック（組織実行の開始前・Gitの準備）。
import type { BlockedReason, RepoStatusView } from "../ipc/types";
import type { ConfirmSpec } from "./confirmTexts.ts";

/** Gitの状態が所有者違いなら、許可の対象にするリポジトリの場所。そうでなければ null。 */
export function ownerRepoOf(s: RepoStatusView | null | undefined): string | null {
  return s?.kind === "ownerMismatch" ? s.repo.trim() || null : null;
}

/** 開始前チェックの拒否理由（Blocked）が所有者違いなら、許可の対象にするリポジトリの場所。 */
export function blockedOwnerRepo(b: BlockedReason | null | undefined): string | null {
  return b?.kind === "repoNotPrepared" ? ownerRepoOf(b.repo) : null;
}


/** 許可すると記録設定も変わること（組織実行の開始前・変更の確認のどちらの確認にも入れる共通の一文）。 */
export const OWNER_ALLOW_RECORD_NOTE = "このリポジトリの変更の記録も「記録する」になります（以前「記録しない」を選んでいても上書きします）。";

/** 変更の記録の案内（RepoRecordPanel）の、選ぶ前の説明。 */
export const RECORD_CHOICE_NOTE = "「記録する」を選ぶと、AgentDock がこのリポジトリの変更を控えとして残し、「変更を戻す」で戻せるようになります。所有者が違う場合は、先に下の許可が必要です。";
export const RECORD_SKIP_NOTE = "「記録しない」を選ぶと、このリポジトリでは AgentDock が変更を戻せなくなります。後から、設定の「保存と容量」で「記録する」に変えられます。";

/** 許可を選んだ後に押す確認の本文（許可の範囲と、設定を変えないことを明示する）。 */
export const ownerAllowConfirmText = (repo: string): string =>
  `次のリポジトリを、AgentDockのgit操作だけに許可します。\n\n${repo}\n\nAgentDockが起動するgit（開始前の確認・worktree・コミット・取り込みなど）にだけ safe.directory として渡します。あなたのgit設定（.gitconfig）は変更しません。\n\n${OWNER_ALLOW_RECORD_NOTE}\n\nこのフォルダのGit管理は別のユーザーの所有です。内容を信頼できる場合だけ許可してください。許可はいつでも解除できます。`;

/** 許可の確認ダイアログ（appConfirm）の内容。 */
export const ownerAllowSpec = (repo: string): ConfirmSpec => ({ title: "AgentDockのgit操作だけに許可しますか", body: ownerAllowConfirmText(repo), okLabel: "許可する", cancelLabel: "やめる" });

// 所有者違い（dubious ownership）のリポジトリを「AgentDockのgit操作だけに許可」する案内の純粋ロジック（組織実行の開始前・Gitの準備）。
import type { BlockedReason, RepoStatusView } from "../ipc/types";

/** Gitの状態が所有者違いなら、許可の対象にするリポジトリの場所。そうでなければ null。 */
export function ownerRepoOf(s: RepoStatusView | null | undefined): string | null {
  return s?.kind === "ownerMismatch" ? s.repo.trim() || null : null;
}

/** 開始前チェックの拒否理由（Blocked）が所有者違いなら、許可の対象にするリポジトリの場所。 */
export function blockedOwnerRepo(b: BlockedReason | null | undefined): string | null {
  return b?.kind === "repoNotPrepared" ? ownerRepoOf(b.repo) : null;
}

/** 許可を選んだ後に押す確認の本文（許可の範囲と、設定を変えないことを明示する）。 */
export const ownerAllowConfirmText = (repo: string): string =>
  `次のリポジトリを、AgentDockのgit操作だけに許可します。\n\n${repo}\n\nAgentDockが起動するgit（開始前の確認・worktree・コミット・取り込みなど）にだけ safe.directory として渡します。あなたのgit設定（.gitconfig）は変更しません。\n\nこのリポジトリの変更の記録も「記録する」になります（以前「記録しない」を選んでいても上書きします）。\n\nこのフォルダのGit管理は別のユーザーの所有です。内容を信頼できる場合だけ許可してください。許可はいつでも解除できます。`;

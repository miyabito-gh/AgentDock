// 組織実行の開始前の注意: 作業フォルダに未コミットの変更があるとき（Workerの作業場所はHEADから作られるため反映されない）。
import type { DirtySummaryView, RepoStatusView } from "../ipc/types";

/** 注意を出す対象の状態か（コミットのあるリポジトリだけ。ほかはGitの状態行が案内する）。 */
export function dirtyCheckApplies(s: RepoStatusView | null): boolean {
  return s?.kind === "ready" || s?.kind === "nested";
}

/** 未コミットの変更がないとき null。自動ではコミットしない（選ぶのは利用者）。 */
export function dirtyNoticeText(d: DirtySummaryView | null): string | null {
  if (!d || d.total === 0) return null;
  const shown = d.sample.slice(0, 5);
  const rest = d.total - shown.length;
  const names = shown.join("、") + (rest > 0 ? ` ほか${rest}件` : "");
  return `作業フォルダに未コミットの変更が${d.total}件あります（${names}）。Workerの作業場所はHEADから作られるため、これらは反映されません。先にコミットするか、このまま始めるかを選んでください。`;
}

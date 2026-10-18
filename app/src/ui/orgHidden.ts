// 組織実行の役割チャットを通常の一覧から隠すかの判断（純粋関数）。
import type { ChatLocalView, OrgSnapshotView } from "../ipc/types";

/**
 * 組織実行が作って、まだ手動に切り替えていないチャット（通常の一覧から隠し、ワークフローから辿る）。
 * shown＝画面に出せるワークフローのID（実行中＋保存済みの過去分＋アーカイブ分）。そのワークフローが出せない（読めない・件数の外）ときは
 * 辿り道がないので隠さない。null＝まだ取得していない（従来どおり隠す）。
 */
export const isOrgOwned = (l: ChatLocalView | undefined, shown: ReadonlySet<string> | null = null): boolean =>
  !!l?.org && !l.org.released && (shown === null || shown.has(l.org.workflowId));

/** 画面に出せるワークフローのID。取得に失敗していて何も無いときは空集合（隠さない）、まだ取得していなければ null。 */
export function shownOrgIds(org: OrgSnapshotView | null, failed: boolean): Set<string> | null {
  if (!org) return failed ? new Set() : null;
  // アーカイブしたワークフローも辿れる（アーカイブ一覧から開く）ので、役割チャットは通常の一覧へ漏らさない。
  const ids = new Set<string>([...org.history, ...(org.archived ?? [])].map((w) => w.workflowId));
  if (org.workflow) ids.add(org.workflow.workflowId);
  return ids;
}

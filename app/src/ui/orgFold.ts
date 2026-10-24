// 左一覧の『過去のワークフロー』の畳み・件数・まとめてアーカイブの対象選別（純粋関数。表示だけで、状態・保存は変えない）。
// node --test で単体実行できるよう、他の ui モジュールには依存しない。

export interface FoldWf {
  workflowId: string;
  finished: boolean;
  archivedAt: number | null;
  deletion: unknown | null;
  artifactHint: { ready: number } | null;
  tasks: Array<{ confirm: { state: string }; status: { phase: string }; needsChatChoice: boolean }>;
}

/** 取り込み待ちの成果物があるか（ready は取得できた件数。印が無ければ無しと断定せず false＝数えない）。 */
export const hasPendingImport = (w: Pick<FoldWf, "artifactHint">): boolean => !!w.artifactHint && w.artifactHint.ready > 0;

/** あなたの対応が要るか（確認状態が確定でない・止まっている・失敗・チャット選択待ち。org.ts の needsAttention と同じ条件）。 */
export const needsYou = (w: Pick<FoldWf, "tasks">): boolean =>
  w.tasks.some((t) => t.confirm.state !== "confirmed" || t.status.phase === "blocked" || t.status.phase === "failed" || t.needsChatChoice);

/**
 * 過去のワークフロー（新しい順）を、常に見せる行と畳む行に分ける。
 * 見せる: 終了していないもの・開いているもの・削除途中（先頭側）・直近に終わった1件（現在のワークフローが終了済みならそれが該当するので出さない）。
 * searching のときは畳まない。
 */
export function splitHistory<T extends FoldWf>(history: readonly T[], selectedId: string | null, currentFinished: boolean, searching: boolean): { visible: T[]; folded: T[] } {
  if (searching) return { visible: [...history], folded: [] };
  const recent = currentFinished ? null : history.find((w) => w.finished && !w.deletion)?.workflowId ?? null;
  const keep = (w: T) => !w.finished || !!w.deletion || w.workflowId === selectedId || w.workflowId === recent;
  const deleting = history.filter((w) => !!w.deletion);
  const others = history.filter((w) => !w.deletion && keep(w));
  return { visible: [...deleting, ...others], folded: history.filter((w) => !keep(w)) };
}

export interface FoldCounts { total: number; pendingImport: number; needAction: number }
export function foldCounts(folded: readonly FoldWf[]): FoldCounts {
  return { total: folded.length, pendingImport: folded.filter(hasPendingImport).length, needAction: folded.filter(needsYou).length };
}

/** 畳み行の見出し。見落とさないよう、取り込み待ち・要対応は数を文字で添える（0 のものは出さない）。 */
export function foldLabel(c: FoldCounts): string {
  const extra = [c.pendingImport ? `取り込み待ち ${c.pendingImport}` : "", c.needAction ? `要対応 ${c.needAction}` : ""].filter((x) => x);
  return `過去のワークフロー（${c.total}件${extra.length ? `・うち${extra.join("・")}` : ""}）`;
}

/** まとめてアーカイブの候補（終了済み・確定済み・削除途中でない・未アーカイブ）。最終判定はホスト。 */
export const archiveEligible = <T extends FoldWf>(folded: readonly T[]): T[] =>
  folded.filter((w) => w.finished && w.archivedAt === null && !w.deletion && !w.tasks.some((t) => t.confirm.state !== "confirmed"));

/** 取り込み待ちのものを含めるかで対象を選ぶ（既定は外す）。 */
export function pickArchiveTargets<T extends FoldWf>(eligible: readonly T[], includePending: boolean): T[] {
  return includePending ? [...eligible] : eligible.filter((w) => !hasPendingImport(w));
}

/** 確認に並べる対象の表示（先頭 max 件＋ほか N 件）。 */
export function clipList<T>(items: readonly T[], max = 5): { head: T[]; more: number } {
  return { head: items.slice(0, max), more: Math.max(0, items.length - max) };
}

export interface ArchiveResult { id: string; ok: boolean; reason?: string }
/** 結果の文。1件ごとの失敗で全体は止めない。 */
export function summarizeArchive(rs: readonly ArchiveResult[]): string {
  const ok = rs.filter((r) => r.ok).length;
  const ng = rs.filter((r) => !r.ok);
  if (ng.length === 0) return `${ok}件をアーカイブしました（アーカイブ一覧から戻せます）。`;
  const why = Array.from(new Set(ng.map((r) => r.reason ?? "理由を取得できません"))).slice(0, 2).join(" / ");
  return `${ok}件をアーカイブしました／${ng.length}件はできませんでした: ${why}`;
}

// 組織実行の周回・判定・辺の文言（純粋関数。node --test で単体テストできるよう org.ts から分けている）。
import type { OrgGraphEdgeView, OrgGraphNodeView, OrgVerdict } from "../ipc/types";

/** 判定値の日本語訳。未知の値は元の値をそのまま出す（推測で訳さない）。ホスト側の辺の条件の訳（view.rs の verdict_ja）と同じ対応。 */
const VERDICT_JA: Record<string, string> = { rework: "やり直し", pass: "合格", replan: "計画からやり直し" };
export const verdictJa = (v: string): string => VERDICT_JA[v] ?? v;

/** 判定の文言。出力がまだなければ「—」、値を読めなければ「未取得」（空や既定値にしない）。値は訳して出す。 */
export const verdictText = (v: OrgVerdict | null | undefined): string => (!v ? "" : v.kind === "pending" ? "—" : v.kind === "notObtained" ? "未取得" : verdictJa(v.text));

/**
 * 周回の回数の文言。単位は「実行」（最初の実行を含む）と「やり直し」（差し戻して戻る回数）に分ける。
 * 実行 = やり直し + 1。`used` は使ったやり直し、`allowed` はやり直しの許容回数（利用者が足した分を含む）。
 */
export function reworkLimitText(used: number, allowed: number): string {
  const left = Math.max(0, allowed - used);
  return `実行 ${used + 1}回目／最大 ${allowed + 1}回（やり直しは最大${allowed}回、残り${left}回）`;
}

/** 周回の中の工程の回数文言。周回の外・まだ動いていない工程は null。`round`＝実行回数、`maxRound`＝最大の実行回数。 */
export const roundText = (n: OrgGraphNodeView): string | null => (n.round !== null && n.maxRound !== null ? reworkLimitText(n.round - 1, n.maxRound - 1) : null);

/** 辺の一行。通っていない辺は「未実行」を付ける（薄く表示する側の目印）。`nameOf` は工程IDから表示名（ないときはIDのまま）。 */
export function edgeText(e: OrgGraphEdgeView, nameOf: (id: string) => string = (id) => id): { head: string; detail: string; untaken: boolean; title: string } {
  const title = `${e.from}->${e.to}`;
  if (e.kind === "feedback") {
    const allowed = e.allowed ?? 0;
    return { head: `↺ ${nameOf(e.to)}`, detail: `${e.label}、やり直し 最大${allowed}回・使用${e.used}回・残り${Math.max(0, allowed - e.used)}回`, untaken: e.used === 0, title };
  }
  return { head: `→ ${nameOf(e.to)}`, detail: e.label, untaken: e.used === 0, title };
}

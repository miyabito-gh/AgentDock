// 依頼整理の会話の削除（左一覧の「まとめて削除」と、ワークフロー削除との選択式）の純粋ロジック。
// 実行: node --test src/ui/intakeDelete.test.ts （tscの対象外）。最終の判定はホスト（ここは候補を選ぶだけ）。
import type { Chat, ChatKey, ChatLocalView } from "../ipc/types";
import { listTitle, shortStamp } from "./leftNest.ts";

export type IntakeStateText = "開始済み" | "やめたもの";

export interface IntakeDeleteRow {
  chat: ChatKey;
  title: string;
  state: IntakeStateText;
  /** 最近使った時刻（無ければ null）。 */
  stamp: number | null;
}

/** 紐づくワークフローの判定に使う最小の情報（実行中・未終了かどうか）。 */
export interface WfLite { finished: boolean }

const knownValue = <T>(k: { kind: string; value?: T } | undefined): T | undefined => (k && k.kind === "value" ? k.value : undefined);

/**
 * まとめて削除の候補。終了した依頼整理（開始済み・やめたもの）で、ピン留めでなく、紐づくワークフローが実行中・未終了でないもの。
 * 未開始（下書き・要約中・確認待ち）は対象外。紐づけ先が画面に出せない（削除済み等）ものは候補に入れる（ホストが最終判定）。
 * アーカイブ・一覧から外した・引き継ぎ済み・削除保留中は候補にしない。
 */
export function intakeDeleteTargets(
  chats: readonly Chat[],
  localOf: (c: Chat) => ChatLocalView | undefined,
  wfs: ReadonlyMap<string, WfLite>,
): IntakeDeleteRow[] {
  const out: IntakeDeleteRow[] = [];
  for (const c of chats) {
    const l = localOf(c);
    const it = l?.conversation?.intake;
    if (!l || !it || (it.kind !== "started" && it.kind !== "abandoned")) continue;
    if (c.pinned) continue;
    if (l.org && !l.org.released) continue;
    if (l.visibility.kind !== "visible" || knownValue(c.archived) === true || l.supersededBy || l.deletePending) continue;
    if (it.kind === "started") {
      const wf = wfs.get(it.workflowId);
      if (wf && !wf.finished) continue; // 紐づくワークフローが実行中・未終了
    }
    out.push({ chat: c.key, title: listTitle(c, l).text, state: it.kind === "started" ? "開始済み" : "やめたもの", stamp: c.lastUsedAt ?? knownValue(c.createdAt) ?? null });
  }
  return out;
}

/** 開始済み・やめたものの件数（畳み行の表示用）。 */
export function stateCounts(rows: readonly IntakeDeleteRow[]): { started: number; abandoned: number } {
  const started = rows.filter((r) => r.state === "開始済み").length;
  return { started, abandoned: rows.length - started };
}

export const BULK_HEADLINE = (n: number): string => `依頼整理の会話 ${n} 件を削除します`;

export const BULK_IRREVERSIBLE = "削除すると会話の記録は元に戻せません。残したいときは削除せず、そのままにしてください（左一覧からは隠れています）。";

export const BULK_HOST_NOTE = "実行中・停止を確認できないもの・保存に失敗しているもの・引き継ぎ済みのものは、削除せずに残します。";

/** 一覧の1行（名前・状態・日時）。 */
export function rowText(r: IntakeDeleteRow): string {
  return `${r.title}（${r.state}${r.stamp === null ? "" : `・${shortStamp(r.stamp)}`}）`;
}

/** 先頭5件＋「ほかN件」。 */
export function previewLines(rows: readonly IntakeDeleteRow[], max = 5): string[] {
  const lines = rows.slice(0, max).map(rowText);
  if (rows.length > max) lines.push(`ほか ${rows.length - max} 件`);
  return lines;
}

export interface BulkFailure { title: string; reason: string }

/** 結果の集計文。「N件を削除しました／M件はできませんでした: 理由」。 */
export function resultText(deleted: number, failed: readonly BulkFailure[]): string {
  const head = `${deleted}件を削除しました`;
  if (failed.length === 0) return head;
  return `${head}／${failed.length}件はできませんでした: ${failed.map((f) => `${f.title}（${f.reason}）`).join("、")}`;
}

/** ホストの拒否・失敗メッセージの先頭1行（長い案内文を一覧に並べない）。 */
export const reasonOf = (message: string): string => message.split("\n")[0].trim() || "理由を取得できませんでした";

/** ワークフロー削除の確認での、依頼整理の会話の扱い。 */
export type IntakeChoice =
  | { kind: "none"; note: string }
  | { kind: "gone"; note: string }
  | { kind: "offer"; defaultOn: true; blocked: string | null };

export const INTAKE_UNLINKED_NOTE = "依頼整理の会話は、紐づけを確認できないため対象外です。";

/** 削除ダイアログの依頼整理チェックの出し分け（既定オン。消せない理由があってもワークフローの削除は止めない）。 */
export function intakeChoice(pv: { intake: { gone: boolean; blocked: string | null } | null; intakeUnlinked: boolean }): IntakeChoice {
  if (!pv.intake) return { kind: "none", note: INTAKE_UNLINKED_NOTE };
  if (pv.intake.gone) return { kind: "gone", note: "依頼整理の会話は、すでに削除されています。" };
  return { kind: "offer", defaultOn: true, blocked: pv.intake.blocked };
}

/** ワークフロー削除の結果から、依頼整理の会話についての一言（無ければ null）。 */
export function intakeOutcomeText(r: { deleted: boolean; reason: string | null } | null | undefined): string | null {
  if (!r) return null;
  return r.deleted ? "依頼整理の会話も削除しました。" : `依頼整理の会話は削除しませんでした: ${r.reason ? reasonOf(r.reason) : "理由を取得できませんでした"}`;
}

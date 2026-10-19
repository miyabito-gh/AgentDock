// 組織実行の依頼整理（DESIGN_ORG_INTAKE §1）の純粋な補助。要約カードの入力と、ホストの型の相互変換。
import type { IntakeStartPrefs, IntakeSummary } from "../ipc/types";

/** 1行1項目の入力を配列にする（空行は捨てる）。 */
export const linesToList = (s: string): string[] => s.split(/\r?\n/).map((x) => x.trim()).filter((x) => x !== "");
export const listToLines = (a: string[]): string => a.join("\n");

/** 要約カードの入力（文字列のまま持つ）。 */
export interface SummaryForm {
  goal: string;
  deliverable: string;
  inputs: Array<{ path: string; note: string }>;
  constraints: string;
  doneCriteria: string;
  openQuestions: string;
  graphId: string;
  maxWriters: string;
  maxTotal: string;
  maxTurns: string;
}

export const emptyForm = (): SummaryForm => ({ goal: "", deliverable: "", inputs: [], constraints: "", doneCriteria: "", openQuestions: "", graphId: "", maxWriters: "", maxTotal: "", maxTurns: "" });

const numText = (n: number | null): string => (n === null ? "" : String(n));

export function formOf(s: IntakeSummary | null, p: IntakeStartPrefs): SummaryForm {
  return {
    goal: s?.goal ?? "", deliverable: s?.deliverable ?? "",
    inputs: s ? s.inputs.map((i) => ({ path: i.path, note: i.note })) : [],
    constraints: listToLines(s?.constraints ?? []), doneCriteria: listToLines(s?.doneCriteria ?? []), openQuestions: listToLines(s?.openQuestions ?? []),
    graphId: p.graphId ?? "", maxWriters: numText(p.maxWriters), maxTotal: numText(p.maxTotal), maxTurns: numText(p.maxTurns),
  };
}

export const sameForm = (a: SummaryForm, b: SummaryForm): boolean => JSON.stringify(a) === JSON.stringify(b);

/** 数の入力（空欄は既定＝null、範囲外・数でないものは undefined）。 */
export function parseCount(s: string, min: number, max: number): number | null | undefined {
  const t = s.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return undefined;
  const n = Number(t);
  return n >= min && n <= max ? n : undefined;
}

/** 入力の誤り（保存できない理由）。なければ null。 */
export function formProblem(f: SummaryForm, turnLimit: number): string | null {
  if (parseCount(f.maxWriters, 1, 4) === undefined || parseCount(f.maxTotal, 1, 4) === undefined) return "並列数は 1〜4 の整数で入れてください（空欄なら既定）";
  if (parseCount(f.maxTurns, 1, turnLimit) === undefined) return `turn 上限は 1〜${turnLimit} の整数で入れてください（空欄なら既定）`;
  return null;
}

/** ホストへ渡す形にする。誤りがあるときは呼ばない（`formProblem` で確認してから）。 */
export function summaryOf(f: SummaryForm): IntakeSummary {
  return {
    goal: f.goal.trim(), deliverable: f.deliverable.trim(),
    inputs: f.inputs.map((i) => ({ path: i.path.trim(), note: i.note.trim() })).filter((i) => i.path !== ""),
    constraints: linesToList(f.constraints), doneCriteria: linesToList(f.doneCriteria), openQuestions: linesToList(f.openQuestions),
  };
}

export function prefsOf(f: SummaryForm, folder: string, turnLimit: number): IntakeStartPrefs {
  return {
    folder,
    graphId: f.graphId === "" ? null : f.graphId,
    maxWriters: parseCount(f.maxWriters, 1, 4) ?? null,
    maxTotal: parseCount(f.maxTotal, 1, 4) ?? null,
    maxTurns: parseCount(f.maxTurns, 1, turnLimit) ?? null,
  };
}

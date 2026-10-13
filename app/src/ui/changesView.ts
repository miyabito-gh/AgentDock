// 「変更を確認」画面（ChangesDialog.tsx）の表示用の純粋ロジック（依頼の選択肢・行の状態・選択・確認の要約・状態スロット）。
// 規則: 判定そのものはホスト（rules/baseline.rs）が行う。ここは受け取った判定を並べて印にするだけで、判定を足したり変えたりしない。
//       取れていない値は「未確認」とし、0で代用しない。チェックの既定はすべてオフ（戻すのは利用者が押したときだけ）。
import type { ChangedFile, Known, RevertItem, RevertPlan, RevertResult, SegmentStatus, TurnRecord } from "../ipc/types";
import { forcedPaths, pickedPaths, reasonLines, type ReasonLine } from "./revertView.ts";

// ───────────────────────────── 依頼の選択肢 ─────────────────────────────

export interface RequestOption {
  turn: string;
  /** 1始まりの通し番号（古い順）。 */
  n: number;
  /** 画面の表示名（`#n 発言の先頭 時刻` ＋ 印）。 */
  label: string;
  /** `#n 発言の先頭`（確認の見出し・title用。印・時刻なし）。 */
  name: string;
  /** 控えを取れなかった・送信されなかった依頼（選べない）。 */
  disabled: boolean;
}

const MAX_HEAD = 30;
export function clip(text: string, max = MAX_HEAD): string {
  const t = text.replace(/\s+/g, " ").trim();
  const chars = Array.from(t);
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : t;
}

/** turn の依頼（ユーザーの発言）の先頭。手元になければ null。 */
export function requestHead(turn: string, records: readonly TurnRecord[]): string | null {
  const rec = records.find((r) => r.key.turnId === turn);
  const e = rec?.entries.find((x) => x.kind.kind === "userMessage");
  if (!e || e.text.kind !== "value") return null;
  const c = clip(e.text.value);
  return c === "" ? null : c;
}

const pad2 = (v: number) => String(v).padStart(2, "0");
export const clockOf = (ms: number): string => { const d = new Date(ms); return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`; };

/** 依頼セレクタの選択肢（新しい順）。turn に対応づいた区間だけ（受理不明の区間は対応づくまで出さない）。 */
export function requestOptions(segs: readonly SegmentStatus[], records: readonly TurnRecord[]): RequestOption[] {
  const out: RequestOption[] = [];
  for (const s of segs) {
    if (!s.turn || out.some((o) => o.turn === s.turn)) continue;
    const n = out.length + 1;
    const head = requestHead(s.turn, records);
    const name = head ? `#${n}「${head}」` : `#${n}`;
    const marks: string[] = [];
    if (s.state.kind === "running") marks.push("●作業中");
    if (s.state.kind === "endUnknown") marks.push("…");
    if (s.concurrent) marks.push("⇆");
    const disabled = s.state.kind === "failed" || s.state.kind === "abandoned";
    if (disabled) marks.push("記録なし");
    out.push({ turn: s.turn, n, name, disabled, label: `${name} ${clockOf(s.startedAt)}${marks.length ? ` ${marks.join(" ")}` : ""}` });
  }
  return out.reverse();
}

/** 既定の依頼＝選べる最新のもの。なければ null（「すべての依頼」）。 */
export const defaultRequest = (opts: readonly RequestOption[]): string | null => opts.find((o) => !o.disabled)?.turn ?? null;

export const requestName = (turn: string, opts: readonly RequestOption[]): string => opts.find((o) => o.turn === turn)?.name ?? turn;

// ───────────────────────────── 行の状態 ─────────────────────────────

export type RowState = "revertible" | "needsOverride" | "blocked" | "reverted" | "failed" | "unknown";

export interface Row {
  key: string;
  file: ChangedFile;
  state: RowState;
  /** 計画の項目（戻せるか確認できていない行では null）。 */
  item: RevertItem | null;
  /** 一覧のチェックを押せる（戻せるだけ。要確認は右ペインの「それでも戻す」）。 */
  selectable: boolean;
  checked: boolean;
  /** 要確認・戻せない・失敗の理由（要確認は固定順で全件）。 */
  reasons: ReasonLine[];
  includes: string[];
}

export const fileKey = (f: ChangedFile): string => `${f.path}|${f.moveTo ?? ""}`;

/** 一覧と計画と選択と直前の結果から、行ごとの状態を作る。plan が null なら「未確認」（戻せるかの判定がない）。 */
export function mergeRows(files: readonly ChangedFile[], plan: RevertPlan | null, chosen: Readonly<Record<string, boolean>>, last: RevertResult | null): Row[] {
  return files.map((file) => {
    const key = fileKey(file);
    const item = plan?.items.find((i) => i.path === file.path) ?? null;
    const failed = last?.failed.find((f) => f.path === file.path);
    const base = { key, file, item, includes: item?.includesTurns ?? [] };
    if (file.reverted || last?.reverted.includes(file.path)) return { ...base, state: "reverted" as const, selectable: false, checked: false, reasons: [] };
    if (failed) return { ...base, state: "failed" as const, selectable: false, checked: false, reasons: [{ code: "readFailed" as const, label: "失敗", message: failed.reason }] };
    if (!item) return { ...base, state: "unknown" as const, selectable: false, checked: false, reasons: [] };
    const v = item.verdict;
    if (v.kind === "blocked" && v.code === "alreadyReverted") return { ...base, state: "reverted" as const, selectable: false, checked: false, reasons: [] };
    const reasons = reasonLines(v);
    if (v.kind === "revertible") return { ...base, state: "revertible" as const, selectable: true, checked: !!chosen[file.path], reasons };
    if (v.kind === "needsOverride") return { ...base, state: "needsOverride" as const, selectable: false, checked: !!chosen[file.path], reasons };
    return { ...base, state: "blocked" as const, selectable: false, checked: false, reasons };
  });
}

export const STATE_LABEL: Record<RowState, string> = { revertible: "", needsOverride: "要確認", blocked: "戻せない", reverted: "戻し済み", failed: "失敗", unknown: "未確認" };
export const STATE_MARK: Record<RowState, string> = { revertible: "", needsOverride: "◆", blocked: "■", reverted: "✓", failed: "✕", unknown: "？" };

/** 「すべて」の状態。戻せる行だけが対象（要確認は含めない）。 */
export function allState(rows: readonly Row[]): "all" | "none" | "some" | "empty" {
  const sel = rows.filter((r) => r.selectable);
  if (sel.length === 0) return "empty";
  const on = sel.filter((r) => r.checked).length;
  return on === 0 ? "none" : on === sel.length ? "all" : "some";
}

/** 「すべて」を押したときの新しい選択。戻せる行だけを変え、要確認の選択はそのまま。 */
export function toggleAll(rows: readonly Row[], chosen: Readonly<Record<string, boolean>>): Record<string, boolean> {
  const turnOn = allState(rows) !== "all";
  const next = { ...chosen };
  for (const r of rows) if (r.selectable) { if (turnOn) next[r.file.path] = true; else delete next[r.file.path]; }
  return next;
}

/** チェックを1件切り替える（戻せる・要確認だけ。それ以外は変えない）。 */
export function setChecked(rows: readonly Row[], chosen: Readonly<Record<string, boolean>>, path: string, on: boolean): Record<string, boolean> {
  const r = rows.find((x) => x.file.path === path);
  if (!r || (r.state !== "revertible" && r.state !== "needsOverride")) return { ...chosen };
  const next = { ...chosen };
  if (on) next[path] = true; else delete next[path];
  return next;
}

/** 判定の署名（区分と、止める・要確認の理由コード）。変わったかの比較に使う。 */
function signature(it: RevertItem | undefined): string {
  if (!it) return "none";
  const v = it.verdict;
  switch (v.kind) {
    case "revertible": return "r";
    case "blocked": return `b:${v.code}`;
    case "needsOverride": return `n:${v.reasons.map((r) => r.code).sort().join(",")}`;
  }
}

/** 選んだファイルのうち、2つの計画で判定が変わった（または消えた）もの。1件でもあれば実行しない。 */
export function planChanged(oldPlan: RevertPlan | null, newPlan: RevertPlan, paths: readonly string[]): string[] {
  return paths.filter((p) => signature(oldPlan?.items.find((i) => i.path === p)) !== signature(newPlan.items.find((i) => i.path === p)));
}

/** 計画を取り直したときの選択。判定が変わらなかったファイルのチェックだけ残す（変わったものは外す）。 */
export function keepChosen(chosen: Readonly<Record<string, boolean>>, oldPlan: RevertPlan | null, newPlan: RevertPlan | null): Record<string, boolean> {
  if (!newPlan) return {};
  const keep = Object.keys(chosen).filter((p) => chosen[p] && planChanged(oldPlan, newPlan, [p]).length === 0);
  return Object.fromEntries(keep.map((p) => [p, true]));
}

// ───────────────────────────── 見出し・確認 ─────────────────────────────

export interface Headline { count: number; additions: number | null; deletions: number | null }

const knownSum = (vals: Known<number>[]): number | null => {
  let t = 0;
  for (const v of vals) { if (v.kind !== "value") return null; t += v.value; }
  return t;
};

/** 「N ファイル +a −d」。行数が1つでも取れていない（バイナリを除く）ときは null（0で代用しない）。 */
export function headline(files: readonly ChangedFile[]): Headline {
  const counted = files.filter((f) => !f.binary);
  return { count: files.length, additions: knownSum(counted.map((f) => f.additions)), deletions: knownSum(counted.map((f) => f.deletions)) };
}

export interface ConfirmSummary {
  /** 戻す順の全パス（通常→要確認）。 */
  paths: string[];
  forced: string[];
  total: number;
  /** 通常の対象（表示用: パス・一緒に戻る後の依頼の数・削除されるか）。 */
  normal: Array<{ path: string; includes: number; deletes: boolean }>;
  /** 要確認で入れた対象（理由つき）。 */
  override: Array<{ path: string; reasons: ReasonLine[] }>;
  /** 主ボタン・危険ボタンの名前。要確認を含むときは別の変更も含めることを明示する。 */
  actionLabel: string;
}

export function confirmSummary(items: readonly RevertItem[], chosen: Readonly<Record<string, boolean>>): ConfirmSummary {
  const list = [...items];
  const picked = pickedPaths(list, chosen);
  const forced = forcedPaths(list, chosen);
  const total = picked.length + forced.length;
  const find = (p: string) => list.find((i) => i.path === p)!;
  return {
    paths: [...picked, ...forced], forced, total,
    normal: picked.map((p) => ({ path: p, includes: find(p).includesTurns.length, deletes: find(p).kind === "added" })),
    override: forced.map((p) => ({ path: p, reasons: reasonLines(find(p).verdict) })),
    actionLabel: forced.length > 0 ? `別の変更も含めて ${total} 件を戻す` : `${total} 件を戻す`,
  };
}

// ───────────────────────────── 状態スロット ─────────────────────────────

export type StatusSlot =
  | { kind: "listError"; tone: "err"; text: string }
  | { kind: "changed"; tone: "warn"; paths: string[] }
  | { kind: "noBaseline"; tone: "warn"; text: string }
  | { kind: "owner"; tone: "warn" }
  | { kind: "planError"; tone: "warn"; text: string }
  | { kind: "result"; tone: "info" | "err"; result: RevertResult }
  | { kind: "resultError"; tone: "err"; text: string }
  | { kind: "notes"; tone: "muted"; text: string };

export interface SlotInput {
  /** 一覧の取得失敗（通信の例外・取得できなかった旨）。 */
  listError: string | null;
  /** 実行前の取り直しで判定が変わったファイル（実行しなかった）。 */
  changedPaths: string[];
  /** 控えがない理由（重複を除いた帯に出す文。出さないときは null）。 */
  baseText: string | null;
  /** 所有者違い・記録しない・許可済みの表示がある。 */
  ownerPanel: boolean;
  /** 戻せるかの判定の取得失敗。 */
  planError: string | null;
  result: RevertResult | null;
  /** 戻す操作の例外（成功とは書かない）。 */
  resultError: string | null;
  notes: readonly string[];
}

/** 上から最初に当たったものを1つだけ返す（帯は1本）。 */
export function statusSlot(i: SlotInput): StatusSlot | null {
  if (i.listError) return { kind: "listError", tone: "err", text: i.listError };
  if (i.changedPaths.length > 0) return { kind: "changed", tone: "warn", paths: i.changedPaths };
  if (i.baseText) return { kind: "noBaseline", tone: "warn", text: i.baseText };
  if (i.ownerPanel) return { kind: "owner", tone: "warn" };
  if (i.planError) return { kind: "planError", tone: "warn", text: i.planError };
  if (i.resultError) return { kind: "resultError", tone: "err", text: i.resultError };
  if (i.result) return { kind: "result", tone: i.result.failed.length > 0 ? "err" : "info", result: i.result };
  const notes = i.notes.filter((n) => n.trim() !== "");
  if (notes.length > 0) return { kind: "notes", tone: "muted", text: notes.join("・") };
  return null;
}

/** 結果の1行（一部のみは完了と書かない）。 */
export function resultText(r: RevertResult): string {
  if (r.failed.length > 0) return `一部のみ戻しました（戻せた ${r.reverted.length}・戻せなかった ${r.failed.length}）`;
  return r.reverted.length > 0 ? `${r.reverted.length} 件を戻しました` : "書き換えたファイルはありません";
}

// ───────────────────────────── 差分・パスの表示 ─────────────────────────────

/** パスを「ファイル名」と「フォルダ部」に分ける（区切りは / と \）。フォルダ部がなければ空文字。 */
export function splitPath(p: string): { name: string; dir: string } {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i < 0 ? { name: p, dir: "" } : { name: p.slice(i + 1), dir: p.slice(0, i) };
}

export type DiffKind = "add" | "del" | "ctx" | "hunk" | "meta";
export interface DiffRow { kind: DiffKind; old: number | null; new: number | null; text: string }

/** 統一diffを行に分ける（行番号つき）。本文は加工しない（先頭の +/-/空白だけ記号として分ける）。ハンクの前の行・`#` 行・`\` 行は meta。 */
export function diffRows(text: string): DiffRow[] {
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  const out: DiffRow[] = [];
  let o = 0, n = 0, inHunk = false;
  for (const l of lines) {
    const h = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(l);
    if (h) { o = Number(h[1]); n = Number(h[2]); inHunk = true; out.push({ kind: "hunk", old: null, new: null, text: l }); continue; }
    if (!inHunk || l.startsWith("#") || l.startsWith("\\")) { out.push({ kind: "meta", old: null, new: null, text: l }); continue; }
    if (l.startsWith("+")) out.push({ kind: "add", old: null, new: n++, text: l.slice(1) });
    else if (l.startsWith("-")) out.push({ kind: "del", old: o++, new: null, text: l.slice(1) });
    else out.push({ kind: "ctx", old: o++, new: n++, text: l.startsWith(" ") ? l.slice(1) : l });
  }
  return out;
}

/** 巨大な差分は先頭だけ描画する（超えた分は件数を示す）。 */
export const DIFF_ROW_LIMIT = 4000;

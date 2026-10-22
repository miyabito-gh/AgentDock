// 目的メモの下書き支援（DESIGN_CONT_DRAFT §4）の純関数: 採用した提案からメモを組み立てる、行差分、文言。
// 規則: 現在のメモの行は、利用者が置き換え・削除を選んだ行以外は一字も変えない（DA7）。node --test で直接実行できるよう、型の import だけにする。
import type { DraftAction, DraftKind, DraftMode, DraftProposal, DraftState, ProposalIssue } from "../ipc/types";

export const KIND_HEADINGS: Record<DraftKind, string> = {
  decision: "■ 決まったこと",
  constraint: "■ 前提・制約",
  openIssue: "■ 未解決の課題",
  reference: "■ 重要なファイル・数値・用語",
};
export const KIND_LABELS: Record<DraftKind, string> = {
  decision: "決まったこと",
  constraint: "前提・制約",
  openIssue: "未解決の課題",
  reference: "重要なファイル・数値・用語",
};
export const KIND_ORDER: DraftKind[] = ["decision", "constraint", "openIssue", "reference"];

/** 出典の表記（台帳の通し番号は #n、今の thread だけのときは 今#n）。 */
export const cite = (n: number, mode: DraftMode): string => `[${mode === "headOnly" ? "今#" : "#"}${n}]`;

/** メモに足す・置き換える行。出典があればその表記、なければ「（根拠なし）」。 */
export function memoLine(p: Pick<DraftProposal, "text" | "sources">, mode: DraftMode): string {
  const tail = p.sources.length > 0 ? ` ${p.sources.map((n) => cite(n, mode)).join("")}` : "（根拠なし）";
  return `- ${p.text.trim()}${tail}`;
}

const isHeading = (line: string): boolean => line.trim().startsWith("■ ");

/** 節の見出しの位置と、節の最後の内容行の位置（見出しだけの節は見出しの位置）。見出しがなければ null。 */
function sectionEnd(lines: string[], heading: string): number | null {
  const h = lines.findIndex((l) => l.trim() === heading);
  if (h < 0) return null;
  let end = h;
  for (let i = h + 1; i < lines.length && !isHeading(lines[i]); i++) if (lines[i].trim() !== "") end = i;
  return end;
}

export interface Assembled {
  text: string;
  /** 適用できなかった提案の index（置換対象が一意に見つからない・同じ行を二重に選んだ）。 */
  skipped: number[];
}

/** 採用した提案（picks の index）から、新しい目的メモを組み立てる。置き換え・削除は一致する1行だけ。追加は種類の節の末尾（なければメモの末尾に節を作る）。 */
export function assembleMemo(memo: string, proposals: DraftProposal[], picks: ReadonlySet<number>, mode: DraftMode): Assembled {
  const orig = memo === "" ? [] : memo.split("\n");
  const cells: (string | null)[] = [...orig];
  const touched = new Set<number>();
  const skipped: number[] = [];
  const chosen = proposals.filter((p) => picks.has(p.index) && p.action !== null && p.kind !== null);
  for (const p of chosen) {
    if (p.action !== "replace" && p.action !== "remove") continue;
    const target = p.replaces.trim();
    const hits = target === "" ? [] : orig.flatMap((l, i) => (l.trim() === target ? [i] : []));
    if (hits.length !== 1 || touched.has(hits[0])) { skipped.push(p.index); continue; }
    const i = hits[0];
    touched.add(i);
    if (p.action === "remove") { cells[i] = null; continue; }
    const indent = /^\s*/.exec(orig[i])?.[0] ?? "";
    cells[i] = indent + memoLine(p, mode);
  }
  const lines = cells.filter((l): l is string => l !== null);
  for (const p of chosen) {
    if (p.action !== "add" || p.kind === null) continue;
    const heading = KIND_HEADINGS[p.kind];
    const end = sectionEnd(lines, heading);
    if (end !== null) { lines.splice(end + 1, 0, memoLine(p, mode)); continue; }
    if (lines.length > 0 && lines[lines.length - 1] !== "") lines.push("");
    lines.push(heading, memoLine(p, mode));
  }
  return { text: lines.join("\n"), skipped };
}

export interface DiffLine { kind: "same" | "add" | "del"; text: string }

/** 行単位の差分（最長共通部分列）。大きすぎるときは全行を削除＋追加として返す。 */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before === "" ? [] : before.split("\n");
  const b = after === "" ? [] : after.split("\n");
  if (a.length * b.length > 250_000) return [...a.map((text): DiffLine => ({ kind: "del", text })), ...b.map((text): DiffLine => ({ kind: "add", text }))];
  const w = b.length + 1;
  const t = new Uint32Array((a.length + 1) * w);
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) t[i * w + j] = a[i] === b[j] ? t[(i + 1) * w + j + 1] + 1 : Math.max(t[(i + 1) * w + j], t[i * w + j + 1]);
  const out: DiffLine[] = [];
  let i = 0, j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) { out.push({ kind: "same", text: a[i] }); i++; j++; }
    else if (t[(i + 1) * w + j] >= t[i * w + j + 1]) out.push({ kind: "del", text: a[i++] });
    else out.push({ kind: "add", text: b[j++] });
  }
  while (i < a.length) out.push({ kind: "del", text: a[i++] });
  while (j < b.length) out.push({ kind: "add", text: b[j++] });
  return out;
}

export const diffMark = (k: DiffLine["kind"]): string => (k === "add" ? "＋" : k === "del" ? "－" : "　");

/** 印の文言。要確認かどうかは needsReview。 */
export function issueText(i: ProposalIssue): string {
  switch (i) {
    case "citationMissing": return "出典番号が存在しません";
    case "citationUnverifiable": return "原文を確かめられない往復です";
    case "citationInconsistent": return "出典の記載が矛盾しています";
    case "noEvidence": return "根拠なし";
    case "replaceTargetMissing": return "対象の行が見つかりません";
    case "malformed": return "形式が崩れています";
  }
}
export const needsReview = (i: ProposalIssue): boolean => i !== "noEvidence";

export const actionText = (a: DraftAction | null): string => (a === "add" ? "追加" : a === "replace" ? "置き換え" : a === "remove" ? "削除" : "（操作不明）");

/** 状態の見出し。完了・成功とは言わず、結果の中身は「確認待ち／要確認」で示す。 */
export function stateHeadline(s: DraftState): { text: string; tone: "run" | "ok" | "warn" | "fail" } {
  switch (s.kind) {
    case "preparing": return { text: "材料を集めています", tone: "run" };
    case "running": return { text: "更新案を作っています", tone: "run" };
    case "ready": return s.reviewCount > 0 ? { text: `確認待ち（要確認 ${s.reviewCount} 件）`, tone: "warn" } : { text: "確認待ち", tone: "ok" };
    case "unreadable": return { text: `結果を読み取れませんでした（${s.reason}）`, tone: "fail" };
    case "rejected": return { text: s.message, tone: "fail" };
    case "acceptanceUnknown": return { text: "作成を始めたか確認できません。結果が届けばここに出ます", tone: "warn" };
    case "lost": return { text: "接続が切れたため、更新案は得られませんでした（作成の結果は不明）", tone: "warn" };
    case "interrupted": return { text: "中止しました", tone: "warn" };
    case "failed": return { text: `作成に失敗しました（${s.message}）`, tone: "fail" };
  }
}

/** 経過時間 m:ss。 */
export function elapsedText(startedAt: number, now: number): string {
  const sec = Math.max(0, Math.floor((now - startedAt) / 1000));
  return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}`;
}

/** 入力の見積り文（トークンは目安であり、推定値として断る）。 */
export function estimateText(e: { roundsIncluded: number; roundsAvailable: number; tocRounds: number; inputChars: number; purposeRevision: number | null }): string {
  const rev = e.purposeRevision === null ? "目的メモ未設定" : `目的メモ版${e.purposeRevision}`;
  const lo = Math.round(e.inputChars * 0.7), hi = e.inputChars;
  return `直近${e.roundsIncluded}往復の原文・目次${e.tocRounds}往復（全${e.roundsAvailable}往復）・${rev}。送る文は約${e.inputChars.toLocaleString("en-US")}字（目安 ${lo.toLocaleString("en-US")}〜${hi.toLocaleString("en-US")} トークン＋基本指示。実際の値ではありません）`;
}

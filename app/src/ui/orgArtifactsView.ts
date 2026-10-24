// 組織実行の成果物パネル・取り込みの表示用の純粋関数（DESIGN_ORG_R3）。React に依存しない（`node --test src/ui/orgArtifactsView.test.ts`）。
// 規則: 取れていない値は「未確認」「未取得」と書き、0や空で代用しない。取り込み・戻すの成功は、読み戻しで確かめられた結果のときだけ書く。
import type { Known, OrgArtifactFile, OrgArtifactItem, OrgArtifactStatus, OrgArtifactsView, OrgCheckOutcome, OrgImportOutcome, OrgImportRecordView, OrgImportResult, OrgInvestigationView, OrgReviewView, OrgUndoOutcome } from "../ipc/types";

export type ChipTone = "run" | "wait" | "fail" | "int" | "done" | "idle" | "unk";

/** 成果物の状態の印（アイコン名は `Icon` のもの。色だけにせず文字も出す）。 */
export const STATUS_CHIP: Record<OrgArtifactStatus, { tone: ChipTone; icon: string }> = {
  working: { tone: "run", icon: "play" },
  aiReviewing: { tone: "run", icon: "eye" },
  readyForReview: { tone: "wait", icon: "eye" },
  notPassed: { tone: "fail", icon: "alert" },
  superseded: { tone: "idle", icon: "circle" },
  imported: { tone: "done", icon: "check" },
  includedElsewhere: { tone: "done", icon: "check" },
  noChanges: { tone: "idle", icon: "circle" },
  unknown: { tone: "unk", icon: "help" },
};

export const sha7 = (s: string | null | undefined): string => (s ? s.slice(0, 7) : "—");

export const isCandidate = (s: OrgArtifactStatus): boolean => s === "readyForReview" || s === "notPassed";

/** 取り込める成果（状態が候補で、前提も満たしている）。計画の順。 */
export const importable = (v: OrgArtifactsView): OrgArtifactItem[] => v.items.filter((i) => i.importable);

/** 見出しの件数。レビュー待ち＝あなたの確認と取り込みを待つ成果（AIのレビュー中とは別）。 */
export function headingCounts(items: OrgArtifactItem[]): { ready: number; imported: number } {
  return { ready: items.filter((i) => i.status === "readyForReview").length, imported: items.filter((i) => i.status === "imported").length };
}

export const headingText = (items: OrgArtifactItem[]): string => {
  const c = headingCounts(items);
  return `成果物　レビュー待ち ${c.ready}・取り込み済み ${c.imported}`;
};

const val = <T,>(k: Known<T>): T | null => (k.kind === "value" ? k.value : null);

/** `N ファイル　+a −d`。数えられない行数があれば、そのとおり書く（0にしない）。 */
export function totalsText(item: Pick<OrgArtifactItem, "totals">): string {
  const t = val(item.totals);
  if (!t) return "未確認";
  const parts = [`${t.files} ファイル`];
  if (t.files > 0) parts.push(`+${t.additions} −${t.deletions}`);
  if (t.binary > 0) parts.push(`バイナリ ${t.binary}`);
  if (t.unknown > 0) parts.push(`行数が未確認のファイル ${t.unknown}`);
  return parts.join("　");
}

const VERDICT_TEXT: Record<OrgReviewView["verdict"], string> = { pass: "合格", rework: "差し戻し", replan: "計画の作り直しを要望", unknown: "未取得" };
export const verdictLabel = (v: OrgReviewView["verdict"]): string => VERDICT_TEXT[v];

const CHECK_OUTCOME_TEXT: Record<OrgCheckOutcome, string> = { passed: "成功", failed: "失敗", notRun: "実行せず" };
export const checkOutcomeLabel = (o: OrgCheckOutcome): string => CHECK_OUTCOME_TEXT[o];

/** 判定の表示名。合格としたが確認のコマンドが失敗したものは、合格とは書かない（エンジンは合格を保留している）。 */
export const reviewVerdictLabel = (r: Pick<OrgReviewView, "verdict" | "heldBack">): string => (r.heldBack ? "合格を保留（確認のコマンドが失敗）" : VERDICT_TEXT[r.verdict]);

/** 実行した確認の要約1行。記録がなければ「記録なし」（0件とは書かない）。 */
export function checksSummary(r: Pick<OrgReviewView, "checks" | "noCheckReason">): string {
  if (r.checks === null) return "確認: 記録なし";
  if (r.checks.length === 0) return `確認: 実行していない${r.noCheckReason && r.noCheckReason.trim() !== "" ? `（理由: ${r.noCheckReason.trim()}）` : "（理由の記録なし）"}`;
  const n = (o: OrgCheckOutcome) => r.checks!.filter((c) => c.outcome === o).length;
  return `確認: 成功 ${n("passed")}・失敗 ${n("failed")}・実行せず ${n("notRun")}`;
}

/** 調査報告をコピー用の文にする（参照は文字のまま。リンクにしない）。 */
export function investigationText(v: OrgInvestigationView): string {
  const out = [`要約: ${v.summary}`];
  v.findings.forEach((f, i) => {
    out.push(`\n結論${i + 1}: ${f.point}`, `  根拠: ${f.basis}`);
    if (f.refs.length > 0) out.push(`  参照: ${f.refs.join(" / ")}`);
  });
  if (v.references.length > 0) out.push(`\n調べた対象: ${v.references.join(" / ")}`);
  if (v.openQuestions.length > 0) out.push("\n分からなかったこと:", ...v.openQuestions.map((q) => `- ${q}`));
  return out.join("\n");
}

/**
 * Reviewer の判定の周回数（初回を含む）。タスク表の「差し戻し n/m」（初回を数えない）とは別の数なので、
 * ここでは「差し戻しN回」と書かず「レビューN周」と書く（2周＝差し戻し1回に相当しうるが、判定の記録の数であり推定しない）。
 */
export const reviewRoundCount = (reviews: OrgReviewView[]): number => reviews.length;
export const REVIEW_ROUNDS_TIP = "実際に実行された Reviewer の判定の回数です。差し戻し後に Worker が再対応しただけで、まだ判定されていない回は数えません。タスク表の「差し戻し n/m」は判定が差し戻しだった回数なので、最後の判定が合格なら周回数は差し戻し＋1です";

/** 最新の判定（ワークフローの最後の回）。 */
export function latestReview(reviews: OrgReviewView[]): OrgReviewView | null {
  let best: OrgReviewView | null = null;
  for (const r of reviews) if (!best || r.round >= best.round) best = r;
  return best;
}

/** 行の所見の要約1行。「Reviewer: 合格（所見2件）」「Reviewer: 未取得」。 */
export function reviewLine(item: Pick<OrgArtifactItem, "reviews">): string {
  const rs = val(item.reviews);
  if (rs === null) return "Reviewer: 未取得";
  if (rs.length === 0) return "Reviewer: なし";
  const last = latestReview(rs)!;
  const extra: string[] = [];
  if (last.verdict !== "unknown" && last.findings.length > 0) extra.push(`所見${last.findings.length}件`);
  const rounds = reviewRoundCount(rs);
  if (rounds > 1) extra.push(`レビュー${rounds}周`);
  return `Reviewer: ${reviewVerdictLabel(last)}${extra.length > 0 ? `（${extra.join("・")}）` : ""}`;
}

const DOC_EXT = new Set(["md", "markdown", "txt", "csv", "tsv"]);
const extOf = (p: string): string => { const n = p.split(/[\\/]/).pop() ?? p; const i = n.lastIndexOf("."); return i > 0 ? n.slice(i + 1).toLowerCase() : ""; };

export type FileTab = "preview" | "diff";

/** 開いたときのタブ。文書（md・txt・csv）と画像はプレビュー、それ以外のテキストは差分。削除されたファイルは差分（プレビューするものが残っていない）。 */
export function defaultTab(f: Pick<OrgArtifactFile, "path" | "moveTo" | "kind" | "preview">): FileTab {
  if (f.kind === "deleted" || f.preview === "none") return "diff";
  if (f.preview === "image") return "preview";
  return DOC_EXT.has(extOf(f.moveTo ?? f.path)) ? "preview" : "diff";
}

/** 出せるタブ。プレビューできない形式・削除されたファイルは差分だけ。 */
export function tabsOf(f: Pick<OrgArtifactFile, "kind" | "preview">): FileTab[] {
  return f.kind === "deleted" || f.preview === "none" ? ["diff"] : ["preview", "diff"];
}

/** 取り込みの結果の文。`Applied` は読み戻しで確かめたときだけ（ホストがそうしか返さない）。 */
export function importResultText(r: OrgImportResult): { tone: "ok" | "warn"; text: string } {
  const o: OrgImportOutcome = r.outcome;
  switch (o.kind) {
    case "applied": return { tone: "ok", text: `取り込みました${r.files == null ? "" : `（${r.files} ファイル）`}。ブランチ ${r.branch} の先頭を ${sha7(o.head)} に確認しました。` };
    case "notApplied": return { tone: "warn", text: `取り込めませんでした（${o.reason}）。作業フォルダは変わっていません。` };
    case "partial": return { tone: "warn", text: `取り込みの途中で止まり、一部のファイルだけが書き換わりました（${o.files.join("、")}）。コミットは進んでいません。「取り込み前に戻す」で、その分を元に戻せます。` };
    case "unknown": return { tone: "warn", text: `取り込めたか確認できません${o.headNow.kind === "value" ? `（今のコミット ${sha7(o.headNow.value)}）` : ""}。再実行はしていません。「状態を確認」で読み取って確かめてください。` };
  }
}

export function undoResultText(o: OrgUndoOutcome): { tone: "ok" | "warn"; text: string } {
  switch (o.kind) {
    case "undone": return { tone: "ok", text: `取り込み前（${sha7(o.head)}）に戻しました。取り込んだ成果は agentdock ブランチに残っているので、あとでまた取り込めます。` };
    case "notUndone": return { tone: "warn", text: `戻せませんでした（${o.reason}）。作業フォルダは変わっていません。` };
    case "unknown": return { tone: "warn", text: `戻せたか確認できません${o.headNow.kind === "value" ? `（今のコミット ${sha7(o.headNow.value)}）` : ""}。再実行はしていません。「状態を確認」してください。` };
  }
}

const RECORD_STATE_TEXT: Record<OrgImportRecordView["state"], string> = {
  applied: "取り込み済み", notApplied: "取り込まれていません", partial: "一部だけ書き換わっています", unknown: "取り込めたか確認できません", undone: "取り込み前に戻しました",
};
export const recordStateText = (s: OrgImportRecordView["state"]): string => RECORD_STATE_TEXT[s];

/** 取り込み記録のうち、画面で操作を出すもの（確認できないもの・戻せるもの）。新しい順で渡される。 */
export const actionableRecords = (v: Pick<OrgArtifactsView, "records">): OrgImportRecordView[] => v.records.filter((r) => r.state === "applied" || r.state === "partial" || r.state === "unknown");

/** 取り込めるものを選んだ順（計画の順）に並べる。選ばれていないタスクIDは捨てる。 */
export function orderSelection(v: Pick<OrgArtifactsView, "items">, picked: Iterable<string>): string[] {
  const set = new Set(picked);
  return v.items.filter((i) => set.has(i.task) && i.importable).map((i) => i.task);
}

/** ファイル一覧の安定したキー。 */
export const fileKey = (f: Pick<OrgArtifactFile, "path" | "moveTo">): string => `${f.path}\u0000${f.moveTo ?? ""}`;

/** CSV／TSV を行と列に分ける（引用符・引用符内の区切り／改行／`""` に対応）。表のプレビュー用。読めない形でも文字は落とさない。 */
export function parseTable(text: string, delimiter: "," | "\t"): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  let any = false;
  const endCell = () => { row.push(cell); cell = ""; any = true; };
  const endRow = () => { endCell(); rows.push(row); row = []; any = false; };
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"') { if (text[i + 1] === '"') { cell += '"'; i++; } else quoted = false; } else cell += c;
    } else if (c === '"' && cell === "") { quoted = true; any = true; }
    else if (c === delimiter) endCell();
    else if (c === "\n") endRow();
    else if (c === "\r") { if (text[i + 1] !== "\n") endRow(); }
    else { cell += c; any = true; }
  }
  if (any || cell !== "") endRow();
  return rows;
}

/** 表の区切り文字（拡張子から。`.tsv` だけタブ）。 */
export const tableDelimiter = (path: string): "," | "\t" => (extOf(path) === "tsv" ? "\t" : ",");

/** 別のワークフローの取得結果が残っていても使わない（切り替え直後の取り違え防止）。 */
export const viewOf = <V extends { workflowId: string }>(v: V | null, workflowId: string | null): V | null => (v && workflowId !== null && v.workflowId === workflowId ? v : null);

// 実行: node --test src/ui/orgArtifactsView.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { actionableRecords, checkOutcomeLabel, checksSummary, investigationText, reviewVerdictLabel, defaultTab, parseTable, tableDelimiter, headingText, importResultText, isCandidate, latestReview, orderSelection, reviewLine, reviewRoundCount, sha7, tabsOf, totalsText, undoResultText } from "./orgArtifactsView.ts";

const known = (value: any) => ({ kind: "value", value, basis: "direct" }) as any;
const NF = { kind: "notFetched" } as any;
const review = (round: number, verdict: string, findings: string[] = []) => ({ reviewerTask: "r", verdict, findings, round, commit: null, fromHistory: false }) as any;
const item = (task: string, status: string, over: any = {}) => ({ task, status, importable: status === "readyForReview" || status === "notPassed", totals: NF, reviews: NF, ...over }) as any;
const file = (path: string, over: any = {}) => ({ path, kind: "modified", moveTo: null, additions: known(1), deletions: known(0), binary: false, preview: "text", ...over }) as any;

test("見出しの件数: レビュー待ちとAIのレビュー中を混ぜない", () => {
  const items = [item("a", "readyForReview"), item("b", "aiReviewing"), item("c", "imported"), item("d", "notPassed"), item("e", "readyForReview")];
  assert.equal(headingText(items), "成果物　レビュー待ち 2・取り込み済み 1");
  assert.equal(headingText([]), "成果物　レビュー待ち 0・取り込み済み 0");
  assert.ok(isCandidate("notPassed") && isCandidate("readyForReview") && !isCandidate("imported") && !isCandidate("superseded"));
});

test("行数: 未確認は0にせず書く。バイナリ・数えられないファイルを併記", () => {
  assert.equal(totalsText({ totals: NF }), "未確認");
  assert.equal(totalsText({ totals: known({ files: 3, additions: 12, deletions: 4, binary: 1, unknown: 0 }) }), "3 ファイル　+12 −4　バイナリ 1");
  assert.equal(totalsText({ totals: known({ files: 2, additions: 5, deletions: 0, binary: 0, unknown: 1 }) }), "2 ファイル　+5 −0　行数が未確認のファイル 1");
  assert.equal(totalsText({ totals: known({ files: 0, additions: 0, deletions: 0, binary: 0, unknown: 0 }) }), "0 ファイル");
});

test("所見の1行: 最新の判定・所見の件数・差し戻しの回数。取れなければ未取得", () => {
  assert.equal(reviewLine({ reviews: NF }), "Reviewer: 未取得");
  assert.equal(reviewLine({ reviews: known([]) }), "Reviewer: なし");
  assert.equal(reviewLine({ reviews: known([review(1, "pass", ["a", "b"])]) }), "Reviewer: 合格（所見2件）");
  assert.equal(reviewLine({ reviews: known([review(1, "rework", ["x"]), review(2, "pass")]) }), "Reviewer: 合格（レビュー2周）");
  assert.equal(reviewLine({ reviews: known([review(2, "unknown"), review(1, "rework", ["x"])]) }), "Reviewer: 未取得（レビュー2周）");
  assert.equal(reviewRoundCount([review(1, "rework"), review(2, "rework"), review(3, "pass")]), 3);
  assert.equal(latestReview([review(2, "pass"), review(1, "rework")])!.round, 2);
  assert.equal(latestReview([]), null);
});

test("開くタブ: 文書と画像はプレビュー、コード類は差分、削除・プレビュー不可は差分だけ", () => {
  assert.equal(defaultTab(file("a.md")), "preview");
  assert.equal(defaultTab(file("data.csv", { preview: "table" })), "preview");
  assert.equal(defaultTab(file("notes.txt")), "preview");
  assert.equal(defaultTab(file("config.json")), "diff");
  assert.equal(defaultTab(file("logo.png", { preview: "image" })), "preview");
  assert.equal(defaultTab(file("report.docx", { preview: "none" })), "diff");
  assert.equal(defaultTab(file("old.md", { kind: "deleted" })), "diff");
  assert.equal(defaultTab(file("old.md", { moveTo: "docs/new.md" })), "preview");
  assert.deepEqual(tabsOf(file("a.md")), ["preview", "diff"]);
  assert.deepEqual(tabsOf(file("a.docx", { preview: "none" })), ["diff"]);
  assert.deepEqual(tabsOf(file("a.md", { kind: "deleted" })), ["diff"]);
});

test("取り込みの結果: 成功は applied のときだけ。不明は再実行していないと書く", () => {
  const r = (outcome: any, over: any = {}) => ({ recordId: "i", outcome, files: null, folder: "C:/r", branch: "main", ...over });
  const a = importResultText(r({ kind: "applied", head: "0123456789abcdef" }, { files: 3 }));
  assert.equal(a.tone, "ok");
  assert.match(a.text, /取り込みました（3 ファイル）/);
  assert.match(a.text, /0123456/);
  assert.equal(importResultText(r({ kind: "notApplied", reason: "x" })).tone, "warn");
  const p = importResultText(r({ kind: "partial", files: ["a.txt"] }));
  assert.equal(p.tone, "warn");
  assert.doesNotMatch(p.text, /取り込みました/);
  const u = importResultText(r({ kind: "unknown", headNow: NF }));
  assert.equal(u.tone, "warn");
  assert.match(u.text, /再実行はしていません/);
  assert.doesNotMatch(u.text, /取り込みました/);
});

test("戻すの結果: 戻せたのは undone のときだけ", () => {
  assert.equal(undoResultText({ kind: "undone", head: "abcdef0123" }).tone, "ok");
  assert.equal(undoResultText({ kind: "notUndone", reason: "r" }).tone, "warn");
  assert.match(undoResultText({ kind: "unknown", headNow: NF }).text, /再実行はしていません/);
});

test("記録の操作: 確認できないもの・戻せるものだけ。選択は計画の順で取り込めるものだけ", () => {
  const recs = ["applied", "undone", "unknown", "notApplied", "partial"].map((state) => ({ state })) as any;
  assert.deepEqual(actionableRecords({ records: recs }).map((r) => r.state), ["applied", "unknown", "partial"]);
  const v = { items: [item("t1", "readyForReview"), item("t2", "imported"), item("t3", "notPassed"), item("t4", "readyForReview", { importable: false })] } as any;
  assert.deepEqual(orderSelection(v, ["t3", "t1", "t2", "t4", "zz"]), ["t1", "t3"]);
  assert.equal(sha7(null), "—");
  assert.equal(sha7("0123456789"), "0123456");
});

test("表のプレビュー: 引用符・改行・空欄を読み、区切りは拡張子で決める", () => {
  assert.deepEqual(parseTable("a,b\n1,2\n", ","), [["a", "b"], ["1", "2"]]);
  assert.deepEqual(parseTable('名前,メモ\r\n"山田, 太郎","1行目\n2行目"\r\n"say ""hi""",', ","), [["名前", "メモ"], ["山田, 太郎", "1行目\n2行目"], ['say "hi"', ""]]);
  assert.deepEqual(parseTable("x\ty\n\t\n", "\t"), [["x", "y"], ["", ""]]);
  assert.deepEqual(parseTable("", ","), []);
  assert.deepEqual(parseTable("only", ","), [["only"]]);
  assert.equal(tableDelimiter("a/b.TSV"), "\t");
  assert.equal(tableDelimiter("a.csv"), ",");
});

test("確認の記録: 記録なしと「実行していない」を区別し、保留した合格は合格と書かない", () => {
  const r = (checks: any, reason: string | null = null, extra: any = {}) => ({ ...review(1, "pass"), checks, noCheckReason: reason, heldBack: false, ...extra }) as any;
  assert.equal(checksSummary(r(null)), "確認: 記録なし");
  assert.equal(checksSummary(r([], "文書だけの変更")), "確認: 実行していない（理由: 文書だけの変更）");
  assert.equal(checksSummary(r([], "  ")), "確認: 実行していない（理由の記録なし）");
  const cs = [{ command: "pytest", outcome: "passed", summary: "" }, { command: "ruff", outcome: "failed", summary: "" }, { command: "tsc", outcome: "notRun", summary: "" }];
  assert.equal(checksSummary(r(cs)), "確認: 成功 1・失敗 1・実行せず 1");
  assert.equal(reviewVerdictLabel(r(cs, null, { heldBack: true })), "合格を保留（確認のコマンドが失敗）");
  assert.equal(reviewVerdictLabel(r([])), "合格");
  assert.equal(reviewLine({ reviews: known([r(cs, null, { heldBack: true })]) }), "Reviewer: 合格を保留（確認のコマンドが失敗）");
  assert.equal(checkOutcomeLabel("notRun"), "実行せず");
});

test("調査報告のコピー文: 結論ごとの根拠と参照・分からなかったこと", () => {
  const t = investigationText({ summary: "s", findings: [{ point: "p", basis: "b", refs: ["a.py:3", "c1"] }], references: ["a.py"], openQuestions: ["q"] });
  assert.equal(t, "要約: s\n\n結論1: p\n  根拠: b\n  参照: a.py:3 / c1\n\n調べた対象: a.py\n\n分からなかったこと:\n- q");
  assert.equal(investigationText({ summary: "s", findings: [], references: [], openQuestions: [] }), "要約: s");
});

// 実行: node --test src/ui/orgText.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { completedNoticeText, finishedBannerText, isInvestigationOnly, noReportHeadNote, reworkCellText, edgeText, reworkLimitText, verdictJa, verdictText } from "./orgText.ts";

test("周回の文言: 実行回数＝やり直し＋1。単位を分けて同じ数え方", () => {
  assert.equal(reworkLimitText(1, 3), "実行 2回目／最大 4回（やり直しは最大3回、残り2回）");
  assert.equal(reworkLimitText(3, 3), "実行 4回目／最大 4回（やり直しは最大3回、残り0回）");
  assert.equal(reworkLimitText(5, 3), "実行 6回目／最大 4回（やり直しは最大3回、残り0回）");
});

test("判定値の訳: 既知は日本語、未知はそのまま", () => {
  assert.equal(verdictJa("rework"), "やり直し");
  assert.equal(verdictJa("pass"), "合格");
  assert.equal(verdictJa("replan"), "計画からやり直し");
  assert.equal(verdictJa("custom"), "custom");
  assert.equal(verdictText({ kind: "value", text: "pass" }), "合格");
  assert.equal(verdictText({ kind: "pending" }), "—");
});

test("辺: 表示名で出し、IDは title へ", () => {
  const e = { from: "verify", to: "build", kind: "feedback", label: "判定 = やり直し", used: 1, allowed: 3 } as never;
  const x = edgeText(e, (id) => ({ build: "実装" })[id] ?? id);
  assert.equal(x.head, "↺ 実装");
  assert.equal(x.title, "verify->build");
  assert.match(x.detail, /やり直し 最大3回・使用1回・残り2回/);
});

test("差し戻し列: 初回を数えず、差し戻し回数／上限", () => {
  assert.equal(reworkCellText(1, 3), "0/2");
  assert.equal(reworkCellText(2, 3), "1/2");
  assert.equal(reworkCellText(3, 3), "2/2");
  assert.equal(reworkCellText(0, 3), "0/2");
});

test("停止理由: 変更なしで要旨が無いときだけ補足", () => {
  assert.equal(noReportHeadNote("Workerが変更を書きませんでした。必要なら"), "報告の要旨は記録されていません");
  assert.equal(noReportHeadNote("Workerが変更を書きませんでした（報告の要旨: 調べた）。"), null);
  assert.equal(noReportHeadNote("コミットできませんでした"), null);
  assert.equal(noReportHeadNote(null), null);
});

test("調査のみ: 変更タスクが無いときだけ取り込みの案内を出さない", () => {
  assert.equal(isInvestigationOnly([{ kind: "investigate" }, { kind: "investigate" }]), true);
  // 役割のタスク（Reviewer 等）が変更種別でも、コミットが無ければ調査のみ
  assert.equal(isInvestigationOnly([{ kind: "investigate" }, { kind: "change" }]), true);
  assert.equal(isInvestigationOnly([{ kind: "investigate", commit: null }, { kind: "change", commit: "abc" }]), false);
  assert.equal(isInvestigationOnly([{ kind: "change", commit: null }]), false, "調査報告が無ければ従来文言");
  assert.equal(isInvestigationOnly([]), false);
  assert.ok(finishedBannerText([{ kind: "investigate" }]).includes("調査報告"));
  assert.ok(finishedBannerText([{ kind: "change" }]).includes("取り込んで"));
});

test("完了通知の文も調査のみで切り替わる", () => {
  assert.ok(completedNoticeText([{ kind: "investigate" }]).includes("取り込む変更はありません"));
  assert.ok(completedNoticeText([{ kind: "change", commit: "a" }]).includes("取り込んでください"));
});

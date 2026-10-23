// 実行: node --test src/ui/orgDeleteUi.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { elapsedLabel, isSlow, manageOffReason, progressText, SLOW_MS } from "./orgDeleteUi.ts";

test("N/M件目: 済んだ段の次を処理中と数え、全体が不明なら出さない", () => {
  assert.equal(progressText({ done: 0, total: 3 }), "1/3 件目");
  assert.equal(progressText({ done: 2, total: 3 }), "3/3 件目");
  assert.equal(progressText({ done: 3, total: 3 }), "3/3 件目");
  assert.equal(progressText({ done: 0, total: 0 }), null);
  assert.equal(progressText(null), null);
});

test("経過と、90秒での案内", () => {
  assert.equal(elapsedLabel(5_400), "5秒");
  assert.equal(elapsedLabel(65_000), "1分05秒");
  assert.equal(isSlow(SLOW_MS - 1), false);
  assert.equal(isSlow(SLOW_MS), true);
});

test("無効の理由: 接続→未完了→未確定の順。有効なら null", () => {
  assert.match(manageOffReason({ connected: false, finished: false, allConfirmed: false })!, /接続していません/);
  assert.match(manageOffReason({ connected: true, finished: false, allConfirmed: true })!, /終わっていないタスク/);
  assert.match(manageOffReason({ connected: true, finished: true, allConfirmed: false })!, /確定していない/);
  assert.equal(manageOffReason({ connected: true, finished: true, allConfirmed: true }), null);
});

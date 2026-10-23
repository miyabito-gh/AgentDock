// 実行: node --test src/ui/historyLoad.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { histStart, histOk, histFail, failReason, failText, histView } from "./historyLoad.ts";

test("遷移: 読み込み中→失敗→再試行→成功", () => {
  let s = histStart(undefined);
  assert.deepEqual(s, { kind: "loading", retry: false });
  s = histFail("timeout");
  assert.equal(s.kind, "failed");
  s = histStart(s);
  assert.deepEqual(s, { kind: "loading", retry: true });
  s = histOk();
  assert.equal(s.kind, "ok");
});
test("失敗理由: 空・空白・無しは原因不明", () => {
  assert.equal(failReason(undefined), "原因不明");
  assert.equal(failReason(null), "原因不明");
  assert.equal(failReason("  "), "原因不明");
  assert.equal(failReason(" 応答なし "), "応答なし");
  assert.equal(failText("原因不明"), "会話の履歴を取得できませんでした: 原因不明（タイムアウトの可能性）");
});
test("表示: 失敗は常に出す。空チャット（成功・未取得）とは区別する", () => {
  assert.equal(histView(histFail("x"), false).kind, "failed");
  assert.equal(histView(histFail("x"), true).kind, "failed");
  assert.equal(histView(histOk(), false).kind, "none");
  assert.equal(histView(undefined, false).kind, "none");
});
test("表示: 読み込み中は初回だけ。取り直しで表示済みなら出さない", () => {
  assert.equal(histView(histStart(undefined), false).kind, "loading");
  assert.equal(histView(histStart(histFail("x")), false).kind, "loading");
  assert.equal(histView(histStart(histFail("x")), true).kind, "none");
});

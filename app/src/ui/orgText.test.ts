// 実行: node --test src/ui/orgText.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { edgeText, reworkLimitText, verdictJa, verdictText } from "./orgText.ts";

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

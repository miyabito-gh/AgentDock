// 実行: node --test src/ui/visual/markdown/draftStash.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { dropDraft, stashDraft, takeDraft } from "./draftStash.ts";

test("同じ印なら戻せて、取り出したら消える", () => {
  stashDraft("a", "text", "s1");
  assert.equal(takeDraft("a", "s1"), "text");
  assert.equal(takeDraft("a", "s1"), null);
});
test("ファイルが変わっていたら（印が違う）捨てる", () => {
  stashDraft("a", "text", "s1");
  assert.equal(takeDraft("a", "s2"), null);
  assert.equal(takeDraft("a", "s1"), null);
});
test("保存したら退避は消える・古いものから溢れる", () => {
  stashDraft("b", "x", "s");
  dropDraft("b");
  assert.equal(takeDraft("b", "s"), null);
  for (let i = 0; i < 10; i++) stashDraft("k" + i, "x", "s");
  assert.equal(takeDraft("k0", "s"), null);
  assert.equal(takeDraft("k9", "s"), "x");
});

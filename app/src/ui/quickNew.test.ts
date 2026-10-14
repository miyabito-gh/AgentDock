// 実行: node --test src/ui/quickNew.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { canQuickSend, pickQuickModel } from "./quickNew.ts";

const ms = [{ id: "a", isDefault: false, defaultEffort: null }, { id: "b", isDefault: true, defaultEffort: "medium" }];

test("記録なしは既定のモデル、既定もなければ先頭、空なら空", () => {
  assert.equal(pickQuickModel(ms, null), "b");
  assert.equal(pickQuickModel([ms[0]], null), "a");
  assert.equal(pickQuickModel([], null), "");
});
test("前回のモデルが一覧にあればそれ、なければ既定", () => {
  assert.equal(pickQuickModel(ms, { model: { model: "a", effort: null } }), "a");
  assert.equal(pickQuickModel(ms, { model: { model: "zzz", effort: null } }), "b");
  assert.equal(pickQuickModel(ms, { model: null }), "b");
});
test("本文が空白だけ・作成中は送れない", () => {
  assert.ok(canQuickSend("x", false));
  assert.ok(!canQuickSend("  \n", false));
  assert.ok(!canQuickSend("x", true));
});

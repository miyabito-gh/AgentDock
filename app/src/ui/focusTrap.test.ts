// 実行: node --test src/ui/focusTrap.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { cycleIndex, isTabbable, needsTrap, tabbableIndexes } from "./focusTrap.ts";

const c = (o: Partial<{ disabled: boolean; hidden: boolean; tabIndex: number; hiddenInput: boolean }> = {}) => ({ disabled: false, hidden: false, tabIndex: 0, ...o });

test("無効・非表示・tabindex負・hidden入力は Tab の対象にしない", () => {
  assert.equal(isTabbable(c()), true);
  assert.equal(isTabbable(c({ disabled: true })), false);
  assert.equal(isTabbable(c({ hidden: true })), false);
  assert.equal(isTabbable(c({ tabIndex: -1 })), false);
  assert.equal(isTabbable(c({ hiddenInput: true })), false);
  assert.deepEqual(tabbableIndexes([c(), c({ disabled: true }), c({ tabIndex: -1 }), c()]), [0, 3]);
});

test("循環: 末尾の次は先頭、先頭の Shift+Tab は末尾。枠の外からは端へ入れる", () => {
  assert.equal(cycleIndex(3, 2, false), 0);
  assert.equal(cycleIndex(3, 0, true), 2);
  assert.equal(cycleIndex(3, 1, false), 2);
  assert.equal(cycleIndex(3, 1, true), 0);
  assert.equal(cycleIndex(3, -1, false), 0);
  assert.equal(cycleIndex(3, -1, true), 2);
  assert.equal(cycleIndex(0, -1, false), null);
});

test("端と枠の外のときだけ自前で処理する", () => {
  assert.equal(needsTrap(3, 1, false), false);
  assert.equal(needsTrap(3, 2, false), true);
  assert.equal(needsTrap(3, 0, true), true);
  assert.equal(needsTrap(3, -1, false), true);
  assert.equal(needsTrap(0, -1, true), true);
});

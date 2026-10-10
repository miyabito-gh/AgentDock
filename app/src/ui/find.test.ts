// 実行: node --test src/ui/find.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { findInSegments } from "./find.ts";

test("単一区間の一致位置", () => {
  const h = findInSegments(["hello world"], "world", 10);
  assert.deepEqual(h, [{ start: { seg: 0, offset: 6 }, end: { seg: 0, offset: 11 } }]);
});

test("区間をまたぐ一致（太字の境目）", () => {
  const h = findInSegments(["abc ", "de", "f ghi"], "c def g", 10);
  assert.equal(h.length, 1);
  assert.deepEqual(h[0].start, { seg: 0, offset: 2 });
  assert.deepEqual(h[0].end, { seg: 2, offset: 3 });
});

test("英字の大文字小文字は同一視、全角半角は同一視しない", () => {
  assert.equal(findInSegments(["Hello HELLO hello"], "hElLo", 10).length, 3);
  assert.equal(findInSegments(["ＡＢＣ"], "abc", 10).length, 0);
  assert.equal(findInSegments(["日本語"], "本", 10).length, 1);
});

test("重なる一致は左から非重複で数える", () => {
  assert.equal(findInSegments(["aaaa"], "aa", 10).length, 2);
  assert.equal(findInSegments(["aaa"], "aa", 10).length, 1);
});

test("上限と空語", () => {
  assert.equal(findInSegments(["a a a a a"], "a", 3).length, 3);
  assert.deepEqual(findInSegments(["abc"], "", 10), []);
  assert.deepEqual(findInSegments([], "a", 10), []);
});

test("空の区間があっても位置が合う", () => {
  const h = findInSegments(["ab", "", "cd"], "bc", 10);
  assert.deepEqual(h[0].start, { seg: 0, offset: 1 });
  assert.deepEqual(h[0].end, { seg: 2, offset: 1 });
});

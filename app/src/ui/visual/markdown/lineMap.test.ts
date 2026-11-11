// 実行: node --test src/ui/visual/markdown/lineMap.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { lineCol, lineOfOffset, offsetOfLine } from "./lineMap.ts";

const t = "ab\ncd\n\nef";
test("位置から行を求める", () => {
  assert.equal(lineOfOffset(t, 0), 1);
  assert.equal(lineOfOffset(t, 2), 1);
  assert.equal(lineOfOffset(t, 3), 2);
  assert.equal(lineOfOffset(t, 6), 3);
  assert.equal(lineOfOffset(t, 7), 4);
  assert.equal(lineOfOffset(t, 999), 4);
});
test("行の先頭の位置を求める（行数を超えたら末尾）", () => {
  assert.equal(offsetOfLine(t, 1), 0);
  assert.equal(offsetOfLine(t, 2), 3);
  assert.equal(offsetOfLine(t, 3), 6);
  assert.equal(offsetOfLine(t, 4), 7);
  assert.equal(offsetOfLine(t, 9), t.length);
});
test("行と列（文字数で数える）", () => {
  assert.deepEqual(lineCol(t, 4), { line: 2, col: 2 });
  assert.deepEqual(lineCol("あい\n𠮷x", 5), { line: 2, col: 2 });
  assert.deepEqual(lineCol("", 0), { line: 1, col: 1 });
});

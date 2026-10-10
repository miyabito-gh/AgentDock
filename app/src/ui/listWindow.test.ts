// 実行: node --test src/ui/listWindow.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { cumulativeTops, windowRange, windowRangeVar, scrollTopToRevealVar, rowHeight, ROW_HEIGHT } from "./listWindow.ts";

test("同じ高さなら固定高さ版と先頭が同じで、見える範囲を必ず含む", () => {
  const h = new Array(500).fill(40);
  for (const st of [0, 35, 400, 5000, 19900]) {
    const v = windowRangeVar(h, st, 600), f = windowRange(500, st, 600, 40);
    assert.equal(v.start, f.start);
    assert.ok(v.end <= f.end && v.end >= Math.min(500, Math.ceil((st + 600) / 40)));
  }
});

test("見出し行（28px）混在でも見える行を含む", () => {
  // [見出し28, 行40 x3, 見出し28, 行40 x3]
  const h = [28, 40, 40, 40, 28, 40, 40, 40];
  assert.deepEqual(cumulativeTops(h), [0, 28, 68, 108, 148, 176, 216, 256, 296]);
  // scrollTop=150, view=30 → 行4(148-176)と行5(上端176 < 180)
  assert.deepEqual(windowRangeVar(h, 150, 30, 0), { start: 4, end: 6 });
  assert.deepEqual(windowRangeVar(h, 0, 28, 0), { start: 0, end: 1 });
  assert.deepEqual(windowRangeVar(h, 0, 29, 0), { start: 0, end: 2 });
});

test("overscan・範囲外・空", () => {
  const h = [28, 40, 40, 40, 28, 40, 40, 40];
  assert.deepEqual(windowRangeVar(h, 150, 30, 2), { start: 2, end: 8 });
  assert.deepEqual(windowRangeVar(h, 99999, 100, 1), { start: 6, end: 8 });
  assert.deepEqual(windowRangeVar(h, -50, 10, 0), { start: 0, end: 1 });
  assert.deepEqual(windowRangeVar([], 0, 100), { start: 0, end: 0 });
});

test("可変高さの reveal", () => {
  const h = [28, 40, 40, 40, 28, 40];
  assert.equal(scrollTopToRevealVar(h, 4, 0, 100), 176 - 100);
  assert.equal(scrollTopToRevealVar(h, 1, 100, 100), 28);
  assert.equal(scrollTopToRevealVar(h, 2, 40, 100), 40);
  assert.equal(scrollTopToRevealVar(h, 9, 7, 100), 7);
});

test("行高: 小28／標準30／大34、ROW_HEIGHT は標準", () => {
  assert.deepEqual([rowHeight("Small"), rowHeight("Normal"), rowHeight("Large")], [28, 30, 34]);
  assert.equal(rowHeight("Normal"), ROW_HEIGHT);
});

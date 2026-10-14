// 実行: node --test src/ui/paneWidth.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { clampPane, loadSaved, displayWidths, paneModes, CENTER_MIN } from "./paneWidth.ts";

test("丸めと範囲", () => {
  assert.equal(clampPane("left", 200.6), 201);
  assert.equal(clampPane("left", 10), 180);
  assert.equal(clampPane("left", 9999), 400);
  assert.equal(clampPane("dock", 100), 260);
  assert.equal(clampPane("dock", 9999), 520);
  assert.equal(clampPane("dock", null), 320);
  assert.equal(clampPane("left", NaN), 240);
  assert.equal(loadSaved("left", null), null);
  assert.equal(loadSaved("dock", 700), 520);
});

test("広い窓では保存値どおり", () => {
  const w = displayWidths({ winW: 1920, left: 300, dock: 400, leftShown: true, dockShown: true });
  assert.equal(w.left, 300);
  assert.equal(w.dock, 400);
  assert.equal(w.leftMax, 400);
  assert.equal(w.dockMax, 520);
});

test("中央480pxを確保する", () => {
  for (const winW of [1280, 1400, 1100, 960]) {
    const w = displayWidths({ winW, left: 400, dock: 520, leftShown: true, dockShown: true });
    assert.ok(winW - w.left - w.dock >= CENTER_MIN || (w.left === 180 && w.dock === 260), `winW=${winW}`);
    assert.ok(w.left >= 180 && w.dock >= 260);
  }
  const w = displayWidths({ winW: 1280, left: 400, dock: 520, leftShown: true, dockShown: true });
  assert.equal(w.left + w.dock, 800);
});

test("窓を狭めても保存値は変わらず、戻せば元の幅", () => {
  const saved = { left: 350, dock: 450 };
  const narrow = displayWidths({ winW: 1250, ...saved, leftShown: true, dockShown: true });
  assert.ok(narrow.left < 350 || narrow.dock < 450);
  assert.deepEqual(saved, { left: 350, dock: 450 });
  const wide = displayWidths({ winW: 1600, ...saved, leftShown: true, dockShown: true });
  assert.equal(wide.left, 350);
  assert.equal(wide.dock, 450);
});

test("閉じた・一時パネルのペインは0、もう一方に余裕が出る", () => {
  const w = displayWidths({ winW: 1100, left: 240, dock: 520, leftShown: true, dockShown: false });
  assert.equal(w.dock, 0);
  assert.equal(w.left, 240);
  assert.equal(w.leftMax, 400);
  const none = displayWidths({ winW: 800, left: null, dock: null, leftShown: false, dockShown: false });
  assert.equal(none.left, 0);
  assert.equal(none.dock, 0);
});

test("既定は240/320", () => {
  const w = displayWidths({ winW: 1920, left: null, dock: null, leftShown: true, dockShown: true });
  assert.equal(w.left, 240);
  assert.equal(w.dock, 320);
});

test("paneModes: 境界", () => {
  const m = (winW: number, leftOpen: boolean, rightOpen: boolean, left: number | null = null) => paneModes({ winW, leftOpen, rightOpen, left });
  // 左折りたたみ・右開
  assert.equal(m(740, false, true).rightTemp, false);
  assert.equal(m(739, false, true).rightTemp, true);
  // 左開（既定240）
  assert.equal(m(980, true, true).rightTemp, false);
  assert.equal(m(979, true, true).rightTemp, true);
  // 左保存幅300
  assert.equal(m(1040, true, true, 300).rightTemp, false);
  assert.equal(m(1039, true, true, 300).rightTemp, true);
  // 左開: 660で左並置、659で両方一時
  assert.deepEqual(m(660, true, true), { leftTemp: false, rightTemp: true });
  assert.deepEqual(m(659, true, true), { leftTemp: true, rightTemp: true });
  // 左折りたたみ＋右並置: 920未満は左を開くと一時パネル
  assert.equal(m(920, false, true).leftTemp, false);
  assert.equal(m(919, false, true).leftTemp, true);
  // 左折りたたみ＋右閉: 660で左は並置
  assert.equal(m(660, false, false).leftTemp, false);
  assert.equal(m(659, false, false).leftTemp, true);
  // 右閉・左開: 左の表示幅＋740で判定
  assert.equal(m(980, true, false).rightTemp, false);
  assert.equal(m(979, true, false).rightTemp, true);
});

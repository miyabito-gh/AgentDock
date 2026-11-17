import { test } from "node:test";
import assert from "node:assert/strict";
import { ZOOM_MAX, ZOOM_MIN, rememberZoom, rememberedZoom, stepZoom, zoomKey, zoomKeyOf, zoomPercent } from "./zoom.ts";

// 実行: node --test src/ui/visual/zoom.test.ts （tscの対象外）
test("stepZoom moves one preset at a time and clamps at the ends", () => {
  assert.equal(stepZoom(1, "in"), 1.1);
  assert.equal(stepZoom(1, "out"), 0.9);
  assert.equal(stepZoom(1.3, "in"), 1.5);
  assert.equal(stepZoom(1.3, "out"), 1.25);
  assert.equal(stepZoom(ZOOM_MAX, "in"), ZOOM_MAX);
  assert.equal(stepZoom(ZOOM_MIN, "out"), ZOOM_MIN);
  assert.equal(stepZoom(2.5, "reset"), 1);
  assert.equal(zoomPercent(0.67), "67%");
});

test("zoomKeyOf reads Ctrl +/-/0 including numpad and JIS ; key", () => {
  const k = (key: string, extra: Partial<{ code: string; ctrlKey: boolean; altKey: boolean }> = {}) => zoomKeyOf({ key, ctrlKey: true, altKey: false, ...extra });
  assert.equal(k("+"), "in"); assert.equal(k("="), "in"); assert.equal(k(";"), "in");
  assert.equal(k("-"), "out"); assert.equal(k("0"), "reset");
  assert.equal(k("x", { code: "NumpadAdd" }), "in");
  assert.equal(k("+", { ctrlKey: false }), null);
  assert.equal(k("+", { altKey: true }), null);
  assert.equal(k("s"), null);
});

test("zoom is remembered per file and 100% is forgotten", () => {
  const key = zoomKey("C:\\Site", "C:\\Site\\Index.html");
  assert.equal(key, zoomKey("c:\\site", "c:\\site\\index.html"));
  assert.equal(rememberedZoom(key), 1);
  rememberZoom(key, 1.5); assert.equal(rememberedZoom(key), 1.5);
  rememberZoom(key, 1); assert.equal(rememberedZoom(key), 1);
});

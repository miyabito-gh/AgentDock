import { test } from "node:test";
import assert from "node:assert/strict";
import { NO_STAMP, afterStep, canSkipReload, pollStep } from "./stampSync.ts";

test("初回は基準にするだけで再読み込みしない。基準では省略扱いにならない", () => {
  assert.equal(pollStep(NO_STAMP, "a"), "baseline");
  const s = afterStep(NO_STAMP, "a", "baseline", true);
  assert.equal(s.seen, "a");
  assert.equal(canSkipReload(s, "a"), false);
  assert.equal(pollStep(s, "a"), "none");
});
test("印が変われば再読み込み。成功したら更新し、同じ印の確認後の再読み込みは省ける", () => {
  const s0 = afterStep(NO_STAMP, "a", "baseline", true);
  assert.equal(pollStep(s0, "b"), "reload");
  const s1 = afterStep(s0, "b", "reload", true);
  assert.deepEqual(s1, { seen: "b", reloaded: "b" });
  assert.equal(canSkipReload(s1, "b"), true);
  assert.equal(canSkipReload(s1, "c"), false);
});
test("再読み込みに失敗したら更新せず、次の周期でも再試行になる", () => {
  const s0 = afterStep(NO_STAMP, "a", "baseline", true);
  const s1 = afterStep(s0, "b", "reload", false);
  assert.equal(s1, s0);
  assert.equal(pollStep(s1, "b"), "reload");
  assert.equal(canSkipReload(s1, "b"), false);
});

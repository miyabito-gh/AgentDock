// 実行: node --test src/ui/modelLabel.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { effortChar, modelBadge, modelBadgeText, modelTooltip, shortModel } from "./modelLabel.ts";

const v = (value: string) => ({ kind: "value", value, basis: "direct" }) as never;
const nf = { kind: "notFetched" } as never;

test("shortModel takes the trailing word or the first 8 chars", () => {
  assert.equal(shortModel("gpt-6.1-sol"), "Sol");
  assert.equal(shortModel("gpt-5.2-codex"), "Codex");
  assert.equal(shortModel("gpt-5.2"), "gpt-5.2");
  assert.equal(shortModel("averyverylongmodelname"), "averyver");
});

test("effortChar maps known efforts and truncates unknown ones", () => {
  assert.equal(effortChar("low"), "低");
  assert.equal(effortChar("medium"), "中");
  assert.equal(effortChar("high"), "高");
  assert.equal(effortChar("xhigh"), "最高");
  assert.equal(effortChar("turbo-plus"), "turb");
});

test("modelBadge is null unless the model is known", () => {
  assert.equal(modelBadge(v("gpt-6.1-sol"), v("high")), "Sol·高");
  assert.equal(modelBadge(v("gpt-6.1-sol"), nf), "Sol");
  assert.equal(modelBadge(nf, v("high")), null);
  assert.equal(modelBadge({ kind: "missing" } as never, nf), null);
});

test("modelTooltip says recorded value and never invents unknown ones", () => {
  assert.equal(modelTooltip(v("gpt-6.1-sol"), v("high")), "モデル: gpt-6.1-sol・推論強度: high（記録値。いま使用中とは限りません）");
  assert.equal(modelTooltip(nf, nf), "モデル: 未確認");
  assert.equal(modelTooltip(v("m"), { kind: "unsupported" } as never), "モデル: m・推論強度: 非対応（記録値。いま使用中とは限りません）");
});

test("modelBadgeText shows an explicit not-fetched label instead of vanishing", () => {
  assert.equal(modelBadgeText(v("gpt-6.1-sol"), v("high")), "Sol·高");
  assert.equal(modelBadgeText(nf, v("high")), "モデル未取得");
});

test("modelTooltip marks the root's recorded model", () => {
  assert.equal(modelTooltip(nf, nf, undefined, true), "親（メイン）のモデル（記録値）\nモデル: 未確認");
  assert.equal(modelTooltip(nf, nf), "モデル: 未確認");
});

import { effortLabel } from "./modelLabel.ts";
test("推論の強さの表示名: 既知は日本語、未知はそのまま", () => {
  assert.equal(effortLabel("medium"), "中");
  assert.equal(effortLabel("xhigh"), "最高");
  assert.equal(effortLabel("ultra-future"), "ultra-future");
});

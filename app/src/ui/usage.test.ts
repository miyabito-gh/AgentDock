// 実行: node --test src/ui/usage.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { fetchedText, formatReset, resetText, unavailableText, usageLine, windowLine } from "./usage.ts";

const val = (value: unknown) => ({ kind: "value", value, basis: "direct" }) as never;

test("formatReset shows time only on the same day", () => {
  const now = new Date(2026, 9, 11, 12, 0).getTime();
  assert.equal(formatReset(new Date(2026, 9, 11, 20, 5).getTime(), now), "20:05");
  assert.equal(formatReset(new Date(2026, 9, 13, 3, 7).getTime(), now), "10/13 03:07");
  assert.equal(fetchedText(new Date(2026, 9, 11, 20, 17).getTime()), "取得 20:17");
});

test("resetText distinguishes unknown reset times", () => {
  const now = new Date(2026, 9, 11, 12, 0).getTime();
  assert.equal(resetText(val(new Date(2026, 9, 11, 20, 5).getTime()), now), "リセット 20:05");
  assert.equal(resetText({ kind: "missing" }, now), "リセット時刻は示されていません");
  assert.equal(resetText({ kind: "notFetched" }, now), "リセット時刻は未確認");
});

test("resetText never prints broken times", () => {
  const now = new Date(2026, 9, 11, 12, 0).getTime();
  for (const bad of [0, -1000, 8.64e15 + 1, 1e300, NaN, Infinity]) assert.equal(resetText(val(bad), now), "リセット時刻は示されていません");
  assert.equal(resetText(val(new Date(2026, 9, 11, 20, 5).getTime() + 0.4), now), "リセット 20:05");
});

test("windowLine keeps the percentage as returned", () => {
  const now = new Date(2026, 9, 11, 12, 0).getTime();
  const w = { role: "primary", usedPercent: 12.5, windowMinutes: val(300), resetsAt: val(new Date(2026, 9, 11, 17, 0).getTime()) } as never;
  assert.equal(windowLine(w, now), "短い期間の枠: 使用率 12.5%（期間 300分）／リセット 17:00");
  const w2 = { role: "secondary", usedPercent: 0, windowMinutes: { kind: "missing" }, resetsAt: { kind: "missing" } } as never;
  assert.equal(windowLine(w2, now), "長い期間の枠: 使用率 0%（期間 欠損）／リセット時刻は示されていません");
});

test("unavailableText separates failure, unsupported and unconfirmed", () => {
  assert.equal(unavailableText(val(1), null, false), null);
  assert.equal(unavailableText({ kind: "notFetched" }, "timeout", false), "取得できませんでした（timeout）");
  assert.equal(unavailableText({ kind: "unsupported" }, null, false), "このアカウントでは非対応です");
  assert.match(unavailableText({ kind: "missing" }, null, true) ?? "", /ログインしていない可能性/);
  assert.match(unavailableText({ kind: "missing" }, null, false) ?? "", /^未確認です/);
});

test("usageLine prints values as returned", () => {
  assert.equal(usageLine({ lifetimeTokens: val(100), peakDailyTokens: { kind: "notFetched" } } as never), "累計 100 トークン／1日の最大 未確認 トークン");
});

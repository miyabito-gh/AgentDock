// 実行: node --test src/pet/composeLogic.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { canAdjust, canCopy, canCreate, charCount, originLabel, primaryAction, QUICK_ADJUSTS, statusLine } from "./composeLogic.ts";

const view = (over: Record<string, unknown> = {}) => ({
  open: true, mode: "reply", source: "本文", origin: "clipboard", notice: null, state: { kind: "idle" }, result: null, canAdjust: false, round: 0, capture: 1, ...over,
}) as any;

test("状態の1行は、取り込み・作成中・完了・失敗・結果不明を平易な日本語で示す", () => {
  assert.match(statusLine(view(), "本文", null).text, /クリップボードから取り込みました/);
  assert.match(statusLine(view({ origin: "selection" }), "本文", null).text, /選択した文章を取り込みました/);
  assert.match(statusLine(view({ origin: "none" }), "", null).text, /文章がありません/);
  assert.equal(statusLine(view({ state: { kind: "generating" }, round: 1 }), "本文", null).tone, "busy");
  assert.match(statusLine(view({ state: { kind: "generating" }, round: 2 }), "本文", null).text, /調整/);
  assert.equal(statusLine(view({ state: { kind: "done" }, result: "x" }), "本文", null).tone, "ok");
  const failed = statusLine(view({ state: { kind: "failed", message: "上限" } }), "本文", null);
  assert.equal(failed.tone, "warn");
  assert.match(failed.text, /上限/);
  assert.equal(statusLine(view({ state: { kind: "unknown", message: "結果は不明です" } }), "本文", null).text, "結果は不明です");
});

test("補足（元のクリップボードを戻せなかったなど）は取り込みの行に添える", () => {
  const t = statusLine(view({ origin: "selection", notice: "元のクリップボードは文字以外だったため戻せませんでした" }), "本文", null).text;
  assert.match(t, /選択した文章を取り込みました。元のクリップボード/);
});

test("画面側のエラーが最優先で出る", () => {
  assert.equal(statusLine(view({ state: { kind: "done" } }), "本文", "コピーできませんでした").text, "コピーできませんでした");
});

test("作成は、文章があり作成中でないときだけ押せる", () => {
  assert.equal(canCreate(view(), "本文", false), true);
  assert.equal(canCreate(view(), "  \n", false), false);
  assert.equal(canCreate(view({ state: { kind: "generating" } }), "本文", false), false);
  assert.equal(canCreate(view(), "本文", true), false);
});

test("再調整は、会話が生きていて指示があるときだけ押せる", () => {
  assert.equal(canAdjust(view({ canAdjust: true, state: { kind: "done" } }), "短く", false), true);
  assert.equal(canAdjust(view({ canAdjust: false, state: { kind: "done" } }), "短く", false), false);
  assert.equal(canAdjust(view({ canAdjust: true, state: { kind: "done" } }), " ", false), false);
  assert.equal(canAdjust(view({ canAdjust: true, state: { kind: "generating" } }), "短く", false), false);
});

test("コピーは結果があり、新しい結果を待っていないときだけ押せる", () => {
  assert.equal(canCopy(view({ state: { kind: "done" }, result: "返信" }), false), true);
  assert.equal(canCopy(view({ state: { kind: "done" }, result: null }), false), false);
  assert.equal(canCopy(view({ state: { kind: "generating" }, result: "前の結果" }), false), false);
  assert.equal(canCopy(view({ state: { kind: "failed", message: "x" }, result: "前の結果" }), false), true);
});

test("主ボタンは、結果があれば「コピー」、なければ「作成」", () => {
  assert.equal(primaryAction(view()), "create");
  assert.equal(primaryAction(view({ state: { kind: "failed", message: "x" } })), "create");
  assert.equal(primaryAction(view({ state: { kind: "done" }, result: "r" })), "copy");
});

test("出どころ・再調整の指示・文字数の表示", () => {
  assert.equal(originLabel("clipboard"), "クリップボードから");
  assert.deepEqual(QUICK_ADJUSTS.map((q) => q.label), ["短く", "柔らかく", "丁寧に"]);
  assert.equal(charCount("あいう😀"), "4文字");
});

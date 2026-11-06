import { test } from "node:test";
import assert from "node:assert/strict";
import { composerPlaceholder } from "./composerHint.ts";

test("送れない理由が最優先", () => {
  assert.equal(composerPlaceholder("停止を確認できるまで", true, true), "停止を確認できるまで");
});
test("作業中は追加指示の案内", () => {
  assert.equal(composerPlaceholder(null, true, false), "作業中です。送ると追加指示になります");
});
test("選択の有無で文言が変わる", () => {
  assert.equal(composerPlaceholder(null, false, true), "この範囲に依頼…");
  assert.equal(composerPlaceholder(null, false, false), "全体への依頼…");
});

import { latestReplyText } from "./composerHint.ts";
const m = (kind: string, value?: string) => ({ kind: { kind }, text: value === undefined ? { kind: "missing" } : { kind: "value", value } });
test("最新の返答は最後のAI本文。取れていないものは飛ばす", () => {
  const turns = [{ entries: [m("agentMessage", "古い")] }, { entries: [m("userMessage", "依頼"), m("agentMessage", "新しい\n返答"), m("agentMessage")] }];
  assert.equal(latestReplyText(turns), "新しい 返答");
  assert.equal(latestReplyText([{ entries: [m("userMessage", "x")] }]), null);
  assert.equal(latestReplyText([]), null);
});
test("長い返答は切り詰める", () => {
  assert.equal(latestReplyText([{ entries: [m("agentMessage", "あ".repeat(10))] }], 4), "ああああ…");
});

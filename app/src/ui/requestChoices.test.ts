// 実行: node --test src/ui/requestChoices.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { requestChoices } from "./requestChoices.ts";

const opt = (id: string, effect: string, scope: string) => ({ id, label: id, effect, scope, description: null }) as never;

test("コマンド承認: 主・セッション・辞退を出し、永続とネットワークと中止は『その他』", () => {
  const c = requestChoices([opt("accept", "allow", "once"), opt("acceptForSession", "allow", "session"), opt("execpolicy", "allow", "persistent"), opt("network:0", "allow", "persistent"), opt("decline", "deny", "once"), opt("cancel", "cancel", "once")]);
  assert.equal(c.primary!.id, "accept");
  assert.equal(c.session!.id, "acceptForSession");
  assert.equal(c.decline!.id, "decline");
  assert.deepEqual(c.more.map((o) => o.id), ["execpolicy", "network:0", "cancel"]);
  assert.equal(c.inline, null);
});

test("提案が無いとき『その他』は中止だけ → メニューにせず直接出す", () => {
  const c = requestChoices([opt("accept", "allow", "once"), opt("acceptForSession", "allow", "session"), opt("decline", "deny", "once"), opt("cancel", "cancel", "once")]);
  assert.deepEqual(c.more.map((o) => o.id), ["cancel"]);
  assert.equal(c.inline!.id, "cancel");
});

test("MCP確認: 承諾が出せないとき主ボタンは無く、辞退と取りやめは残る", () => {
  const c = requestChoices([opt("decline", "deny", "once"), opt("cancel", "cancel", "once")]);
  assert.equal(c.primary, null);
  assert.equal(c.decline!.id, "decline");
  assert.equal(c.inline!.id, "cancel");
});

test("どの選択肢も落とさない（振り分けの合計が元の件数）", () => {
  const all = [opt("a", "allow", "once"), opt("b", "allow", "session"), opt("c", "allow", "session"), opt("d", "deny", "once"), opt("e", "deny", "persistent"), opt("f", "other", "unknown")];
  const c = requestChoices(all);
  const n = [c.primary, c.session, c.decline].filter(Boolean).length + c.more.length;
  assert.equal(n, all.length);
});

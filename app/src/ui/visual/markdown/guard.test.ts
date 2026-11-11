// 実行: node --test src/ui/visual/markdown/guard.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { confirmLeaveMarkdown, markdownDirty, registerMarkdownGuard } from "./guard.ts";

test("登録がなければ確認なしで続けられる", async () => {
  assert.equal(markdownDirty(), false);
  assert.equal(await confirmLeaveMarkdown(), true);
});
test("未保存がなければ確認を出さず、あれば確認の結果に従う", async () => {
  let asked = 0, dirty = false, answer = false;
  const off = registerMarkdownGuard({ dirty: () => dirty, leave: async () => { asked++; return answer; } });
  assert.equal(await confirmLeaveMarkdown(), true);
  assert.equal(asked, 0);
  dirty = true;
  assert.equal(await confirmLeaveMarkdown(), false);
  answer = true;
  assert.equal(await confirmLeaveMarkdown(), true);
  assert.equal(asked, 2);
  off();
  assert.equal(await confirmLeaveMarkdown(), true);
});
test("後から登録された編集欄を、古い登録の解除で外さない", async () => {
  const a = { dirty: () => true, leave: async () => false };
  const b = { dirty: () => true, leave: async () => true };
  const offA = registerMarkdownGuard(a);
  const offB = registerMarkdownGuard(b);
  offA();
  assert.equal(await confirmLeaveMarkdown(), true);
  offB();
});

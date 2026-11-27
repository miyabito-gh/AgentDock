// 実行: node --test src/ui/backendModels.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { aiFullName, aiName, aiTip, chatKeyOfId } from "./backendModels.ts";

test("chatKeyOfId: 一覧のキー（antigravity を含む）を返し、無ければ codex に戻す", () => {
  const chats = [{ key: { backend: "codex", id: "c1" } }, { key: { backend: "antigravity", id: "g1" } }] as any;
  assert.deepEqual(chatKeyOfId(chats, "g1"), { backend: "antigravity", id: "g1" });
  assert.deepEqual(chatKeyOfId(chats, "c1"), { backend: "codex", id: "c1" });
  assert.deepEqual(chatKeyOfId(chats, "zz"), { backend: "codex", id: "zz" });
});

test("aiName: 会話に出す担当AIの名前。旧チャットと Codex", () => {
  assert.equal(aiName("antigravity"), "旧チャット");
  assert.equal(aiName("codex"), "Codex");
});
test("表示名の一元化: 短い名前（印・発話者）と正式名（選択肢・案内）と印のツールチップ", () => {
  assert.equal(aiFullName("antigravity"), "旧チャット");
  assert.equal(aiFullName("codex"), "Codex");
  assert.equal(aiTip("antigravity"), "旧チャットの会話");
  assert.equal(aiTip("codex"), "Codexの会話");
});

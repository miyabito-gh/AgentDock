// 実行: node --test src/ui/backendModels.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { chatKeyOfId, modelsForBackend } from "./backendModels.ts";

const m = (id: string, isDefault = false): any => ({ id, isDefault });

test("modelsForBackend: antigravity は Gemini 側、それ以外は Codex 側", () => {
  const codex = [m("gpt")], agy = [m("gemini")];
  assert.equal(modelsForBackend("codex", codex, agy), codex);
  assert.equal(modelsForBackend("antigravity", codex, agy), agy);
  assert.deepEqual(modelsForBackend("antigravity", codex, []), []);
});

test("chatKeyOfId: 一覧のキー（antigravity を含む）を返し、無ければ codex に戻す", () => {
  const chats = [{ key: { backend: "codex", id: "c1" } }, { key: { backend: "antigravity", id: "g1" } }] as any;
  assert.deepEqual(chatKeyOfId(chats, "g1"), { backend: "antigravity", id: "g1" });
  assert.deepEqual(chatKeyOfId(chats, "c1"), { backend: "codex", id: "c1" });
  assert.deepEqual(chatKeyOfId(chats, "zz"), { backend: "codex", id: "zz" });
});

// 実行: node --test src/ui/shortcuts.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { shortcutsFor } from "./shortcuts.ts";

test("shortcutsFor: 追加指示が既定のときだけ表示が入れ替わる", () => {
  const q = shortcutsFor("queue").find((s) => s.id === "send")!;
  const s = shortcutsFor("steer");
  assert.ok(q.label.includes("完了後に送信"));
  assert.ok(s.find((x) => x.id === "send")!.label.includes("追加指示"));
  assert.ok(s.find((x) => x.id === "sendQueue")!.label.includes("完了後に送信"));
  assert.equal(s.length, shortcutsFor("queue").length);
});

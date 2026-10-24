// 実行: node --test src/ui/confirmTexts.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { extensionRunConfirm, openFileConfirm } from "./confirmTexts.ts";

test("実行形式の確認は、何が起きるかと信頼できるものだけを書き、パスを含む", () => {
  const c = openFileConfirm({ executable: true, outsideProject: true, path: "C:/w/a.exe" });
  assert.ok(c.body.includes("プログラムが実行される") && c.body.includes("信頼できる") && c.body.includes("作業フォルダの外") && c.body.includes("C:/w/a.exe"));
  assert.equal(c.danger, true);
});

test("実行形式でないファイルは危険扱いにしない", () => {
  const c = openFileConfirm({ executable: false, outsideProject: false, path: "C:/w/a.txt" });
  assert.equal(c.danger, false);
  assert.ok(c.body.includes("関連付けられたアプリ"));
});

test("MCP・Plugins の確認は設定が変わりうることと自動で戻さないことを含む", () => {
  const c = extensionRunConfirm("追加します", "codex mcp add x");
  assert.ok(c.body.includes("codex mcp add x") && c.body.includes("設定が変わる可能性") && c.body.includes("自動では元に戻しません"));
});

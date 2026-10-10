// 実行: node --test src/ui/appearance.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { attrsOf, parseStored, resolveTheme, DEFAULT_APPEARANCE } from "./appearance.ts";

test("resolveTheme: OSに従う／手動選択は OS に関係しない", () => {
  assert.equal(resolveTheme("System", false), "light");
  assert.equal(resolveTheme("System", true), "dark");
  assert.equal(resolveTheme("Light", true), "light");
  assert.equal(resolveTheme("Dark", false), "dark");
});

test("attrsOf: サイズは小文字の data 値", () => {
  assert.deepEqual(attrsOf({ theme: "Dark", uiText: "Large", bodyText: "XLarge" }, false), { theme: "dark", uiSize: "large", bodySize: "xlarge" });
  assert.deepEqual(attrsOf(DEFAULT_APPEARANCE, true), { theme: "dark", uiSize: "normal", bodySize: "normal" });
});

test("parseStored: 無い・壊れた・未知の値は既定値に直す", () => {
  assert.deepEqual(parseStored(null), DEFAULT_APPEARANCE);
  assert.deepEqual(parseStored("{not json"), DEFAULT_APPEARANCE);
  assert.deepEqual(parseStored("null"), DEFAULT_APPEARANCE);
  assert.deepEqual(parseStored(JSON.stringify({ theme: "Dark", uiText: "Huge", bodyText: "Large" })), { theme: "Dark", uiText: "Normal", bodyText: "Large" });
});

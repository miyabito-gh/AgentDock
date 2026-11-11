// 実行: node --test src/ui/visual/markdown/context.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { blockElement, lineRange, markdownSendContext } from "./context.ts";
import { splitVisualContext } from "../api.ts";

test("選んだブロックは行の範囲と抜粋で添える", () => {
  const ctx = markdownSendContext("C:\\d\\a.md", [blockElement(12, 15, "p", "本文の  抜粋"), blockElement(20, 20, "h2", "見出し")], [{ path: "a.md", access: "edit" }, { path: "img/p.png", access: "readOnly" }]);
  assert.match(ctx.sel, /編集対象: C:\\d\\a\.md/);
  assert.match(ctx.sel, /- 行12-15 段落「本文の 抜粋」/);
  assert.match(ctx.sel, /- 行20 見出し「見出し」/);
  assert.match(ctx.fixed, /編集してよいファイル: a\.md/);
  assert.match(ctx.fixed, /表示専用のファイル.*img\/p\.png/);
  assert.match(ctx.fixed, /BOM/);
});
test("チャット表示の分割で選択数が数えられる", () => {
  const ctx = markdownSendContext("a.md", [blockElement(1, 1, "p", "x"), blockElement(3, 4, "li", "y")]);
  const split = splitVisualContext("直して\n\n" + ctx.sel);
  assert.equal(split.body, "直して");
  assert.equal(split.count, 2);
});
test("selector から行の範囲を戻せる", () => {
  assert.deepEqual(lineRange("L12-15"), { start: 12, end: 15 });
  assert.deepEqual(lineRange("L7"), { start: 7, end: 7 });
  assert.equal(lineRange("div.x"), null);
});

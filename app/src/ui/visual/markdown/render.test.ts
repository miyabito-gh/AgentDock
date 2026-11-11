// 実行: node --test src/ui/visual/markdown/render.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { excerptOf, outlineOf, renderMarkdown, tagLabel } from "./render.ts";

const spans = (html: string): string[] => Array.from(html.matchAll(/<(\w+) data-src-start="(\d+)" data-src-end="(\d+)"/g)).map((m) => `${m[1]}:${m[2]}-${m[3]}`);

test("見出し・段落・引用・コード・表に元の行の範囲が付く", () => {
  const src = "# T\n\npara one\nline two\n\n> q\n> r\n\n```js\ncode\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
  assert.deepEqual(spans(renderMarkdown(src).html).filter((s) => !s.startsWith("p:") || s === "p:3-4" || s === "p:6-7"), ["h1:1-1", "p:3-4", "blockquote:6-7", "p:6-7", "pre:9-11", "table:13-15"]);
});

test("リスト項目ごとに行の範囲が付き、入れ子も正しい", () => {
  const src = "intro\n\n- a\n- b\n  - c\n- d\n";
  assert.deepEqual(spans(renderMarkdown(src).html), ["p:1-1", "li:3-3", "li:4-5", "li:5-5", "li:6-6"]);
});

test("番号付き・チェックリスト・ゆるいリストでも項目の行が合う", () => {
  const src = "1. one\n2. two\n\n- [ ] todo\n- [x] done\n\n- loose a\n\n- loose b\n";
  const s = spans(renderMarkdown(src).html);
  assert.ok(s.includes("li:1-1") && s.includes("li:2-2"), s.join());
  assert.ok(s.includes("li:4-4") && s.includes("li:5-5"), s.join());
  assert.ok(s.includes("li:7-7") && s.includes("li:9-9"), s.join());
});

test("空行が続いても次のブロックの開始行がずれない", () => {
  const src = "a\n\n\n\nb\n\n---\n\nc";
  assert.deepEqual(spans(renderMarkdown(src).html), ["p:1-1", "p:5-5", "hr:7-7", "p:9-9"]);
});

test("見出しの一覧は行番号・階層・アンカーを持ち、コード内の # は見出しにしない", () => {
  const src = "# A\n\n```\n# not\n```\n\n## B `x`\n\n## B `x`\n";
  assert.deepEqual(outlineOf(src), [
    { level: 1, text: "A", line: 1, slug: "a" },
    { level: 2, text: "B x", line: 7, slug: "b-x" },
    { level: 2, text: "B x", line: 9, slug: "b-x-1" },
  ]);
  assert.match(renderMarkdown(src).html, /data-slug="b-x-1"/);
});

test("抜粋は範囲の行だけを空白をつぶして切り詰める", () => {
  assert.equal(excerptOf("a\n  b  \nc\nd", 2, 3), "b c");
  assert.equal(excerptOf("x".repeat(300), 1, 1, 10), "xxxxxxxxxx…");
  assert.equal(tagLabel("H2"), "見出し");
  assert.equal(tagLabel("zz"), "ブロック");
});

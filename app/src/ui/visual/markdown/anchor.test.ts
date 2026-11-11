import { test } from "node:test";
import assert from "node:assert/strict";
import { anchorKey } from "./anchor.ts";
import { createHeadingSlugger } from "./lumarkSecurity.ts";

test("符号化された日本語の行き先が、見出しの slug と一致する", () => {
  const slug = createHeadingSlugger();
  assert.equal(anchorKey(encodeURIComponent("1-見出しと段落")), slug("1. 見出しと段落"));
});

test("英字の大文字・全角はそろえる。壊れた %XX はそのまま", () => {
  assert.equal(anchorKey("Hello-World"), "hello-world");
  assert.equal(anchorKey("ＡＢＣ"), "abc");
  assert.equal(anchorKey("%E3%81"), "%e3%81");
});

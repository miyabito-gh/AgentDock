// 実行: node --test src/ui/visual/markdown/lumarkSecurity.test.ts （tscの対象外）。Lumark 0.15.0 の markdownSecurity.test.mjs から該当分を移植。
import assert from "node:assert/strict";
import test from "node:test";
import { createHeadingSlugger, normalizeLocalMarkdownPath, safeDataImageSource, safeLinkTarget } from "./lumarkSecurity.ts";

test("リンクはHTTP(S)、mailto、文書内アンカーだけを許可する", () => {
  assert.equal(safeLinkTarget("https://example.com/docs"), "https://example.com/docs");
  assert.equal(safeLinkTarget("mailto:user@example.com"), "mailto:user@example.com");
  assert.equal(safeLinkTarget("#section"), "#section");
  assert.equal(safeLinkTarget("javascript:alert(1)"), null);
  assert.equal(safeLinkTarget("file:///C:/secret.txt"), null);
  assert.equal(safeLinkTarget("./other.md"), null);
});

test("画像はbase64のdata URLだけを許可し、外部URLは許可しない", () => {
  assert.equal(safeDataImageSource("data:image/png;base64,iVBORw0KGgo="), "data:image/png;base64,iVBORw0KGgo=");
  assert.equal(safeDataImageSource("data:image/svg+xml,<svg onload=alert(1)>"), null);
  assert.equal(safeDataImageSource("https://example.com/a.png"), null);
  assert.equal(safeDataImageSource("javascript:alert(1)"), null);
});

test("ローカル画像パスを正規化し、親・絶対パスへの逸脱を拒否する", () => {
  assert.equal(normalizeLocalMarkdownPath("./images/a%20b.png"), "images/a b.png");
  assert.equal(normalizeLocalMarkdownPath("images/temp/../a.png"), "images/a.png");
  assert.equal(normalizeLocalMarkdownPath("../secret.png"), null);
  assert.equal(normalizeLocalMarkdownPath("%2e%2e/secret.png"), null);
  assert.equal(normalizeLocalMarkdownPath("C:\\secret.png"), null);
  assert.equal(normalizeLocalMarkdownPath("/etc/passwd"), null);
});

test("見出しアンカー用IDを生成し、重複見出しを区別する", () => {
  const slug = createHeadingSlugger();
  assert.equal(slug("見出しテスト"), "見出しテスト");
  assert.equal(slug("Hello, World!"), "hello-world");
  assert.equal(slug("見出しテスト"), "見出しテスト-1");
  assert.equal(slug("✨"), "section");
});

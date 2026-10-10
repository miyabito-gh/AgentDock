// 実行: node --test src/ui/markdown.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { components } from "./markdown/parts.ts";
import { exceedsFormatLimit, hostOf, isIdnHost, linkPolicy, remarkHtmlAsText, shortUrl, textMismatchesUrl } from "./markdown/plugins.ts";

const render = (md: string): string =>
  renderToStaticMarkup(h(ReactMarkdown, { remarkPlugins: [remarkGfm, remarkHtmlAsText], components }, md));

test("生HTMLは要素にならず文字で出る", () => {
  const a = render("<script>alert(1)</script>");
  assert.ok(!a.includes("<script"));
  assert.ok(a.includes("&lt;script&gt;alert(1)&lt;/script&gt;"));
  const b = render("<img src=x onerror=alert(1)>");
  assert.ok(!b.includes("<img"));
  assert.ok(b.includes("&lt;img src=x onerror=alert(1)&gt;"));
  const c = render("文中の <b onclick=x()>太字</b> です");
  assert.ok(!c.includes("<b"));
  assert.ok(c.includes("&lt;b onclick=x()&gt;"));
  const d = render("- 項目\n\n  <div onmouseover=x()>y</div>");
  assert.ok(!d.includes("<div"));
});

test("危険なリンクに href が付かず押せない", () => {
  for (const url of ["javascript:alert(1)", "data:text/html,x", "file:///C:/a", "mailto:a@b.c", "./rel.md"]) {
    const out = render(`[x](${url})`);
    assert.ok(!out.includes("href"), `${url}: ${out}`);
    assert.ok(!out.includes("<a"), out);
  }
  assert.ok(render("<javascript:alert(1)>").indexOf("href") < 0);
});

test("http・https のリンクも href を持たず、クリックでの遷移経路がない", () => {
  const out = render("[ok](https://example.com/a)");
  assert.ok(out.includes('role="link"'));
  assert.ok(!out.includes("href="));
});

test("画像は読み込まない", () => {
  const a = render("![x](https://e/x.png)");
  assert.ok(!a.includes("<img"));
  assert.ok(a.includes("[画像: x]"));
  const b = render("![y](data:image/png;base64," + "A".repeat(200) + ")");
  assert.ok(!b.includes("<img"));
  assert.ok(!b.includes("A".repeat(60)));
});

test("表・取消線・タスクが要素になる", () => {
  assert.ok(render("|a|b|\n|-|-|\n|1|2|").includes("<table>"));
  assert.ok(render("~~x~~").includes("<del>"));
  const t = render("- [x] 済\n- [ ] 未");
  assert.ok(t.includes('type="checkbox"') && t.includes("disabled"));
});

test("閉じていないフェンスは末尾までコードになる", () => {
  const out = render("説明\n\n```ts\nconst a = 1;\nconst b = 2;");
  assert.ok(out.includes("<code>"));
  assert.ok(out.includes("const b = 2;"));
  assert.ok(out.includes("md-code"));
});

test("linkPolicy: http・https だけ開く", () => {
  assert.equal(linkPolicy("https://example.com/a?b=1"), "open");
  assert.equal(linkPolicy("http://localhost:3000"), "open");
  for (const u of ["javascript:alert(1)", "data:text/html,x", "file:///C:/a", "mailto:a@b.c", "ftp://x/y", "/rel", "", "https://", null, undefined]) {
    assert.equal(linkPolicy(u), "copyOnly", String(u));
  }
});

test("上限判定: 逐次表示は3万字、完了後は10万字または3,000行", () => {
  assert.equal(exceedsFormatLimit("a".repeat(30_000), true), false);
  assert.equal(exceedsFormatLimit("a".repeat(30_001), true), true);
  assert.equal(exceedsFormatLimit("a".repeat(30_001), false), false);
  assert.equal(exceedsFormatLimit("a".repeat(100_000), false), false);
  assert.equal(exceedsFormatLimit("a".repeat(100_001), false), true);
  assert.equal(exceedsFormatLimit("x\n".repeat(2_999), false), false);
  assert.equal(exceedsFormatLimit("x\n".repeat(3_000), false), true);
});

test("表示用URLと偽装の判定", () => {
  assert.equal(shortUrl("a".repeat(41)), "a".repeat(40) + "…");
  assert.equal(shortUrl("https://e/x"), "https://e/x");
  assert.equal(hostOf("https://Example.com:8080/p"), "example.com");
  assert.equal(textMismatchesUrl("https://good.example/x", "https://evil.example/x"), true);
  assert.equal(textMismatchesUrl("https://good.example/x", "https://good.example/y"), false);
  assert.equal(textMismatchesUrl("ここを押す", "https://evil.example/x"), false);
});

test("スキームなしのドメイン風の表示文字も実ホストと比べる", () => {
  assert.equal(textMismatchesUrl("apple.com", "https://evil.com/"), true);
  assert.equal(textMismatchesUrl("apple.com", "https://www.apple.com/x"), false);
  assert.equal(textMismatchesUrl("www.apple.com", "https://apple.com./"), false);
  assert.equal(textMismatchesUrl("apple.com", "https://store.apple.com/"), true);
  assert.equal(textMismatchesUrl("apple.com/path", "https://APPLE.com/"), false);
  assert.equal(textMismatchesUrl("ドキュメント", "https://evil.com/"), false);
  assert.equal(textMismatchesUrl("v1.2", "https://evil.com/"), false);
});

test("linkPolicy: 原文が http(s)://ホスト の形だけ開く", () => {
  for (const u of ["https:///x", "http:a.com", " https://a.com", "https://?x", "http://#"]) assert.equal(linkPolicy(u), "copyOnly", u);
  assert.equal(linkPolicy("HTTPS://a.com"), "open");
});

test("IDN の判定", () => {
  assert.equal(isIdnHost(hostOf("https://аpple.com/")), true);
  assert.equal(isIdnHost("example.com"), false);
});

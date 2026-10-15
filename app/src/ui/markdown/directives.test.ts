// 実行: node --test src/ui/markdown/directives.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { components } from "./parts.ts";
import { remarkHtmlAsText } from "./plugins.ts";
import { remarkRefs } from "./refs.ts";
import { interpretDirective, scanDirectives, splitDirectives, MAX_VALUE_CHARS } from "./directives.ts";

const one = (s: string) => { const r = scanDirectives(s); assert.equal(r.length, 1); return interpretDirective(r[0]); };

test("git-stage: cwd 必須。コロン1〜3個、引用符の種類、引用符なしを受ける", () => {
  assert.deepEqual(one('::git-stage{cwd="C:\\repo"}'), { kind: "git", op: "stage", cwd: "C:\\repo", branch: null, url: null, draft: false });
  assert.equal((one("::git-stage{cwd='/a b'}") as { cwd: string }).cwd, "/a b");
  assert.equal((one(":git-commit{cwd=/repo}") as { cwd: string }).cwd, "/repo");
  assert.equal(one(":::git-commit{cwd=x}")?.kind, "git");
  assert.equal(scanDirectives("::::git-commit{cwd=x}").length, 0);
});

test("git 系は引用符内の円記号を文字どおり保つ（cwd の末尾の \\）", () => {
  const d = one('::git-stage{cwd="C:\\repo\\"}') as { cwd: string };
  assert.equal(d.cwd, "C:\\repo\\");
});

test("必須属性が無い・空・キー重複は解釈しない（原文のまま）", () => {
  assert.equal(one("::git-stage{}"), null);
  assert.equal(one('::git-stage{cwd=""}'), null);
  assert.equal(one('::git-push{cwd="/r"}'), null);
  assert.equal(scanDirectives('::git-stage{cwd="a" cwd="b"}').length, 0);
  assert.equal(one('::code-comment{title="t" body="b"}'), null);
});

test("create-pr: url と isDraft。branch 必須", () => {
  const d = one('::git-create-pr{cwd="/r" branch="feat" url="https://x/pr/1" isDraft=true}');
  assert.deepEqual(d, { kind: "git", op: "create-pr", cwd: "/r", branch: "feat", url: "https://x/pr/1", draft: true });
});

test("code-comment: 既定値（start=1、end=start）、end<start は start、priority は 0〜3 と P2 形", () => {
  let d = one('::code-comment{title="T" body="B" file="a.rs"}');
  assert.deepEqual(d, { kind: "codeComment", title: "T", body: "B", file: "a.rs", start: 1, end: 1, priority: null });
  d = one('::code-comment{title="T" body="B" file="a.rs" start=10 end=3 priority=P2}');
  assert.deepEqual(d, { kind: "codeComment", title: "T", body: "B", file: "a.rs", start: 10, end: 10, priority: 2 });
  assert.equal((one('::code-comment{title="T" body="B" file="a" priority=7}') as { priority: number | null }).priority, null);
});

test("code-comment は引用符のエスケープ \\\" を引用符として扱う", () => {
  const d = one('::code-comment{title="say \\"hi\\"" body="b" file="a.rs"}') as { title: string };
  assert.equal(d.title, 'say "hi"');
});

test("followup: [label]{prompt}。label か prompt が無ければ解釈しない", () => {
  assert.deepEqual(one(':codex-followup[次へ進む]{prompt="続きをやって"}'), { kind: "followup", label: "次へ進む", prompt: "続きをやって" });
  assert.equal(one(':codex-followup{prompt="x"}'), null);
  assert.equal(one(":codex-followup[]{prompt=x}"), null);
});

test("file-citation: path 必須、label 任意", () => {
  assert.deepEqual(one(':codex-file-citation{path="src/a.rs" label="A"}'), { kind: "citation", path: "src/a.rs", label: "A" });
  assert.equal(one(":codex-file-citation{purpose=x}"), null);
});

test("未知の名前・created-thread・automation-update は解釈しない", () => {
  assert.equal(one('::automation-update{id="1"}'), null);
  assert.equal(one('::created-thread{threadId="x"}'), null);
  assert.equal(one("::foo{a=b}"), null);
});

test("前が英数字・コロンの位置と URL は候補にしない", () => {
  assert.equal(scanDirectives("http://a.com{x=1}").length, 0);
  assert.equal(scanDirectives("ab::git-stage{cwd=x}").length, 0);
  assert.equal(scanDirectives("文 ::git-stage{cwd=x}").length, 1);
});

test("閉じていない指示は候補にならない（逐次表示中は原文のまま）", () => {
  assert.equal(scanDirectives('::git-stage{cwd="C:\\re').length, 0);
  assert.equal(scanDirectives("::git-stage{cwd=x").length, 0);
});

test("長さ上限: 値が長すぎる指示は原文のまま", () => {
  assert.equal(scanDirectives(`::git-stage{cwd="${"a".repeat(MAX_VALUE_CHARS + 1)}"}`).length, 0);
});

test("壊れた候補を大量に並べても固まらない", () => {
  const t0 = Date.now();
  scanDirectives("::git-stage{cwd=".repeat(20_000));
  scanDirectives(":".repeat(100_000));
  assert.ok(Date.now() - t0 < 3000);
});

test("splitDirectives: code-comment は行頭だけ。文中なら原文のまま", () => {
  const dir = '::code-comment{title="T" body="B" file="a.rs"}';
  assert.equal(splitDirectives(dir).some((p) => p.t === "directive"), true);
  assert.equal(splitDirectives(`  ${dir}`).some((p) => p.t === "directive"), true);
  assert.equal(splitDirectives(`見て ${dir}`).some((p) => p.t === "directive"), false);
  assert.equal(splitDirectives(`前の行\n${dir}`).some((p) => p.t === "directive"), true);
  assert.equal(splitDirectives(dir, false).some((p) => p.t === "directive"), false);
  // git 系は文中でもよい
  assert.equal(splitDirectives("済み ::git-commit{cwd=/r}").some((p) => p.t === "directive"), true);
});

const render = (md: string) =>
  renderToStaticMarkup(h(ReactMarkdown, { remarkPlugins: [remarkGfm, remarkHtmlAsText, remarkRefs], components, urlTransform: (u: string) => u }, md));

test("remarkRefs: コードスパン・コードブロック・リンクの中は変換しない", () => {
  assert.ok(!render("`::git-commit{cwd=/r}`").includes('data-ref="directive"'));
  assert.ok(!render("```\n::git-commit{cwd=/r}\n```").includes('data-ref="directive"'));
  assert.ok(!render("[::git-commit{cwd=/r}](https://example.com)").includes('data-ref="directive"'));
  assert.ok(render("済み ::git-commit{cwd=/r}").includes('data-ref="directive"'));
});

test("remarkRefs: 生HTMLは文字のまま。壊れた指示は原文のまま", () => {
  const html = render("<b>x</b> ::git-commit{cwd=/r}");
  assert.ok(html.includes("&lt;b&gt;"));
  assert.ok(render("::git-commit{cwd=}").includes("::git-commit{cwd=}"));
  assert.ok(!render("::git-commit{cwd=}").includes('data-ref="directive"'));
});

test("remarkRefs: 既存の created-thread は従来どおり", () => {
  assert.ok(render('::created-thread{threadId="01a1b2c3-d4e5-7f60-8a9b-0c1d2e3f4a5b"}').includes('data-ref="thread"'));
});

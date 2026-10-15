// 実行: node --test src/ui/refs.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { components } from "./markdown/parts.ts";
import { remarkHtmlAsText } from "./markdown/plugins.ts";
import { baseNameOf, isExecutableName, pathCandidate, pathFromHref, pathLinkView, remarkRefs, splitThreadRefs, wholeThreadRef, type PathStat } from "./markdown/refs.ts";

const ID = "01a1b2c3-d4e5-7f60-8a9b-0c1d2e3f4a5b";
const ID2 = "01a1b2c3-d4e5-7f60-8a9b-0c1d2e3f4a5c";

test("セッションIDを検出する（複数・前後の文字）", () => {
  const p = splitThreadRefs(`見て ${ID}、と ${ID2}。`);
  assert.deepEqual(p.filter((x) => x.t === "thread").map((x) => (x as { id: string }).id), [ID, ID2]);
  assert.equal(p.map((x) => (x.t === "text" ? x.v : x.raw)).join(""), `見て ${ID}、と ${ID2}。`);
});

test("IDの誤検出を除く（大文字・v4・前後が英数字やハイフン・桁違い）", () => {
  for (const s of [ID.toUpperCase(), "01a1b2c3-d4e5-4f60-8a9b-0c1d2e3f4a5b", `x${ID}`, `${ID}-1`, `${ID}a`, `-${ID}`, "01a1b2c3-d4e5-7f60-8a9b-0c1d2e3f4a5", "abc"]) {
    assert.ok(splitThreadRefs(s).every((x) => x.t === "text"), s);
  }
});

test("::created-thread ディレクティブ（行頭・行内・引用符）", () => {
  const p = splitThreadRefs(`::created-thread{threadId="${ID}"} と文中 ::created-thread{threadId='${ID2}'} です`);
  const refs = p.filter((x) => x.t === "thread") as { id: string; directive: boolean }[];
  assert.deepEqual(refs.map((r) => [r.id, r.directive]), [[ID, true], [ID2, true]]);
  // ディレクティブの中のIDを二重に拾わない
  assert.equal(refs.length, 2);
  // 一覧に無い形のIDでもディレクティブとしては検出する
  assert.equal((splitThreadRefs('::created-thread{threadId="abc"}')[0] as { id: string }).id, "abc");
});

test("全体がIDのインラインコード", () => {
  assert.equal(wholeThreadRef(` ${ID} `)?.id, ID);
  assert.equal(wholeThreadRef(`id=${ID}`), null);
});

test("コードブロックの中は変換せず、インラインコードと本文は変換する", () => {
  const out = renderToStaticMarkup(h(ReactMarkdown, { remarkPlugins: [remarkGfm, remarkHtmlAsText, remarkRefs], components }, `本文 ${ID}\n\n\`${ID2}\`\n\n\`\`\`\n${ID}\n::created-thread{threadId="${ID}"}\n\`\`\``));
  assert.equal(out.split('data-ref="thread"').length - 1, 2);
  assert.ok(out.includes(`<code>${ID}\n::created-thread{threadId=&quot;${ID}&quot;}\n</code>`) || out.includes(`${ID}\n::created-thread`));
});

test("パス候補: 絶対・相対・行番号", () => {
  assert.deepEqual(pathCandidate("C:\\work\\a b\\x.rs"), { path: "C:\\work\\a b\\x.rs", line: null, col: null });
  assert.deepEqual(pathCandidate("C:/work/x.rs:12:3"), { path: "C:/work/x.rs", line: 12, col: 3 });
  assert.deepEqual(pathCandidate("src/foo.rs#L40-L50"), { path: "src/foo.rs", line: 40, col: null });
  assert.deepEqual(pathCandidate("./a.md"), { path: "./a.md", line: null, col: null });
  assert.deepEqual(pathCandidate("Cargo.toml"), { path: "Cargo.toml", line: null, col: null });
  assert.equal(pathCandidate("/C:/work/x.rs")?.path, "C:/work/x.rs");
  assert.equal(pathCandidate("src/ui")?.path, "src/ui");
});

test("パス候補: URL・識別子・拡張子なしの単語は対象外", () => {
  for (const s of ["\\\\srv\\share\\x.txt", "//srv/share/x.txt", "\\\\?\\UNC\\srv\\s\\x.txt", "\\\\.\\pipe\\x", "https://example.com/a.js", "file:///C:/a.txt", "foo", "fooBar", "1.2.3", "std::fs", "a b.rs", "-x.rs", "~/a.rs", "/etc/hosts", "fn(x)", "a=b.c", "..", ".", "x", "a:b", "C:12", "<a.rs>", "a|b.rs"]) {
    assert.equal(pathCandidate(s), null, s);
  }
});

test("リンクのhrefからパスを取る（%エンコード）", () => {
  assert.equal(pathFromHref("C:/a%20b/c.rs")?.path, "C:/a b/c.rs");
  assert.equal(pathFromHref("https://x.y/z.js"), null);
  assert.equal(pathFromHref(undefined), null);
});

test("ファイル名の取り出し", () => {
  assert.equal(baseNameOf("C:\\a\\b\\c.txt"), "c.txt");
  assert.equal(baseNameOf("src/ui/"), "ui");
});

test("リンク: file URL と /C:/ 形式・行番号つきをパス候補にする。ネットワークは対象外", () => {
  assert.deepEqual(pathFromHref("file:///C:/work/a%20b.txt"), { path: "C:/work/a b.txt", line: null, col: null });
  assert.deepEqual(pathFromHref("file://localhost/C:/w/a.rs#L12"), { path: "C:/w/a.rs", line: 12, col: null });
  assert.equal(pathFromHref("file:///C:/w/a.rs:7")?.line, 7);
  assert.equal(pathFromHref("file://server/share/a.txt"), null);
  assert.equal(pathFromHref("file:///etc/passwd"), null);
  assert.equal(pathFromHref("/C:/w/a.txt")?.path, "C:/w/a.txt");
  assert.equal(pathFromHref("src/a.rs:12")?.line, 12);
  assert.equal(pathFromHref("C:\\w\\a.txt")?.path, "C:\\w\\a.txt");
  assert.equal(pathFromHref("\\\\srv\\sh\\a.txt"), null);
  assert.equal(pathFromHref("//srv/sh/a.txt"), null);
  assert.equal(pathFromHref("../x/a.md")?.path, "../x/a.md");
});

test("リンク: 実行形式の名前判定", () => {
  for (const n of ["a.exe", "B.CMD", "x.ps1", "s.lnk", "a.exe.", "a.bat "]) assert.equal(isExecutableName(n), true, n);
  for (const n of ["a.txt", "exe", ".exe", "a.exe.txt", "dir/readme.md"]) assert.equal(isExecutableName(n), false, n);
});

test("リンク: 作業フォルダ内のファイルだけ開ける。外は開かずコピーのみ、実行形式は表示のみ、無い・不明は理由", () => {
  const f = (o: Partial<PathStat>): PathStat => ({ kind: "file", absolute: "C:\\w\\a.txt", relative: "a.txt", ...o });
  assert.equal(pathLinkView(undefined), null);
  assert.deepEqual(pathLinkView(f({})), { kind: "menu", open: true, reveal: true, reason: null });
  const out = pathLinkView(f({ relative: null }));
  assert.equal(out?.kind === "menu" && out.open, false);
  assert.equal(out?.kind === "menu" && out.reveal, false);
  const exe = pathLinkView(f({ absolute: "C:\\w\\run.exe", relative: "run.exe" }));
  assert.equal(exe?.kind === "menu" && exe.open, false);
  assert.equal(exe?.kind === "menu" && exe.reveal, true);
  assert.deepEqual(pathLinkView(f({ kind: "missing", absolute: null, relative: null })), { kind: "missing" });
  assert.deepEqual(pathLinkView(f({ kind: "unconfirmed", absolute: null, relative: null })), { kind: "unconfirmed" });
  const root = f({ kind: "folder", absolute: "C:\\w", relative: null });
  assert.equal((pathLinkView(root, "c:/w/") as { open: boolean }).open, true, "作業フォルダ自身は内");
  assert.equal((pathLinkView(root, "C:\\other") as { open: boolean }).open, false, "外のフォルダは開かない");
  assert.equal((pathLinkView(root) as { open: boolean }).open, false, "作業フォルダが不明なら外");
  assert.equal((pathLinkView(f({ kind: "folder", absolute: "C:\\w\\sub", relative: "sub" }), "C:\\w") as { open: boolean }).open, true);
});

// 実行: node --test src/ui/skillPick.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { filterSkills, highlightSegments, originLabel, pushRecent, queryTerms, removeTrigger, resolveRecent, sameNameNote, shortPath, skillTrigger, sortSkills } from "./skillPick.ts";

const sk = (name: string, path: string, scope = "repo", desc = "", enabled = true): any => ({
  name, path, errors: [],
  description: { kind: "value", value: desc, basis: "direct" }, scope: { kind: "value", value: scope, basis: "direct" }, enabled: { kind: "value", value: enabled, basis: "direct" },
});

test("複数語はAND・大文字小文字・全角半角・かなの違いを吸収", () => {
  const all = [sk("Deploy-Tool", "C:/a/SKILL.md", "repo", "ステージングへ配備"), sk("lint", "C:/b/SKILL.md", "user", "検査")];
  assert.equal(filterSkills(all, "deploy ﾊｲﾋﾞ", []).items.length, 0);
  assert.equal(filterSkills(all, "ＤＥＰＬＯＹ ハイビ", []).items.length, 0);
  assert.deepEqual(filterSkills(all, "DEPLOY すてーじんぐ", []).items.map((s) => s.name), ["Deploy-Tool"]);
  assert.deepEqual(filterSkills(all, "c:/b", []).items.map((s) => s.name), ["lint"]);
  assert.equal(queryTerms("  a　b ").length, 2);
});

test("並び: 名前順、無効は最下段", () => {
  const s = sortSkills([sk("zeta", "1"), sk("alpha", "2", "repo", "", false), sk("あい", "3"), sk("beta", "4")]);
  assert.deepEqual(s.map((x) => x.name), ["beta", "zeta", "あい", "alpha"]);
});

test("最近: 空検索は先頭に集約、検索中は先頭へ寄せるだけ、消えたものと無効は出さない", () => {
  const all = [sk("a", "p1"), sk("b", "p2"), sk("c", "p3"), sk("d", "p4", "repo", "", false)];
  const rec = [{ name: "c", path: "p3" }, { name: "x", path: "gone" }, { name: "d", path: "p4" }, { name: "a", path: "p1" }];
  assert.deepEqual(resolveRecent(rec, all).map((s) => s.path), ["p3", "p1"]);
  const v = filterSkills(all, "", rec);
  assert.deepEqual(v.items.map((s) => s.name), ["c", "a", "b", "d"]);
  assert.equal(v.recentCount, 2);
  const q = filterSkills(all, "p", rec);
  assert.equal(q.recentCount, 0);
  assert.deepEqual(q.items.map((s) => s.name), ["c", "a", "b", "d"]);
});

test("最近の更新は先頭・重複なし・最大5件", () => {
  let r: any[] = [];
  for (const n of ["1", "2", "3", "4", "5", "6"]) r = pushRecent(r, { name: n, path: n });
  assert.deepEqual(r.map((x) => x.path), ["6", "5", "4", "3", "2"]);
  assert.deepEqual(pushRecent(r, { name: "3", path: "3" }).map((x) => x.path), ["3", "6", "5", "4", "2"]);
});

test("同名の見分けと出所", () => {
  const a = sk("build", "C:/proj/.codex/skills/build/SKILL.md", "repo");
  const b = sk("Build", "C:/Users/me/.codex/skills/build/SKILL.md", "user");
  const c = sk("other", "C:/x/SKILL.md");
  assert.match(sameNameNote(a, [a, b, c])!, /個人/);
  assert.equal(sameNameNote(c, [a, b, c]), null);
  assert.equal(originLabel(a), "プロジェクト");
  assert.equal(originLabel(sk("p", "C:/u/.codex/plugins/cache/x/SKILL.md", "user")), "プラグイン");
  assert.equal(shortPath("C:/a/b/c/d/SKILL.md"), "…/c/d/SKILL.md");
});

test("強調は正規化して元の位置へ戻す", () => {
  assert.deepEqual(highlightSegments("Ｄｅｐｌｏｙ ツール", ["deploy", "つーる"]), [{ text: "Ｄｅｐｌｏｙ", hit: true }, { text: " ", hit: false }, { text: "ツール", hit: true }]);
  assert.deepEqual(highlightSegments("a😀b😀", ["b"]), [{ text: "a😀", hit: false }, { text: "b", hit: true }, { text: "😀", hit: false }]);
  assert.deepEqual(highlightSegments("😀😀", ["😀"]), [{ text: "😀😀", hit: true }]);
  assert.deepEqual(highlightSegments("abc", []), [{ text: "abc", hit: false }]);
});

test("$ と /skill の呼出し判定", () => {
  assert.deepEqual(skillTrigger("$dep", 4), { start: 0, end: 4, query: "dep" });
  assert.deepEqual(skillTrigger("hello $", 7), { start: 6, end: 7, query: "" });
  assert.equal(skillTrigger("a$b", 3), null);
  assert.equal(skillTrigger("cost $5 now", 11), null);
  assert.deepEqual(skillTrigger("x\n/skill de", 11), { start: 2, end: 11, query: "de" });
  assert.equal(skillTrigger("/skill", 6), null);
  const t = skillTrigger("見て $dep 確認", 7)!;
  assert.deepEqual(removeTrigger("見て $dep 確認", t), { text: "見て 確認", cursor: 2 });
});

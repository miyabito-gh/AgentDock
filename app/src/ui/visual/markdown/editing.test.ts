// 実行: node --test src/ui/visual/markdown/editing.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { continueList, findRanges, pickMatch, replaceAllText, type TextEdit } from "./editing.ts";

const apply = (text: string, e: TextEdit | null): string | null => e ? text.slice(0, e.start) + e.text + text.slice(e.end) : null;
const at = (text: string, marker = "|"): [string, number] => { const i = text.indexOf(marker); return [text.replace(marker, ""), i]; };

test("箇条書きは次の項目を続け、番号は+1、チェックは未完了で続く", () => {
  let [t, p] = at("- item|");
  assert.equal(apply(t, continueList(t, p, p)), "- item\n- ");
  [t, p] = at("  * nested|");
  assert.equal(apply(t, continueList(t, p, p)), "  * nested\n  * ");
  [t, p] = at("9. nine|");
  assert.equal(apply(t, continueList(t, p, p)), "9. nine\n10. ");
  [t, p] = at("1) a|");
  assert.equal(apply(t, continueList(t, p, p)), "1) a\n2) ");
  [t, p] = at("- [x] done|");
  assert.equal(apply(t, continueList(t, p, p)), "- [x] done\n- [ ] ");
});
test("項目の途中で改行すると、後ろの文字が次の項目へ移る", () => {
  const [t, p] = at("- ab|cd");
  const e = continueList(t, p, p)!;
  assert.equal(apply(t, e), "- ab\n- cd");
  assert.equal(e.selectionStart, "- ab\n- ".length);
});
test("空の項目でEnterすると、印を消して終了する（引用の中のリストは引用を残す）", () => {
  let [t, p] = at("a\n- |");
  assert.equal(apply(t, continueList(t, p, p)), "a\n");
  [t, p] = at("- x\n  - |");
  assert.equal(apply(t, continueList(t, p, p)), "- x\n");
  [t, p] = at("> - |");
  assert.equal(apply(t, continueList(t, p, p)), "> ");
  [t, p] = at("- [ ] |");
  assert.equal(apply(t, continueList(t, p, p)), "");
});
test("引用は続け、空の引用行で終了する", () => {
  let [t, p] = at("> quote|");
  assert.equal(apply(t, continueList(t, p, p)), "> quote\n> ");
  [t, p] = at("> a\n> |");
  assert.equal(apply(t, continueList(t, p, p)), "> a\n");
  [t, p] = at("> - item|");
  assert.equal(apply(t, continueList(t, p, p)), "> - item\n> - ");
});
test("通常の行・範囲選択・印の途中・コードの中では続けない（通常の改行）", () => {
  let [t, p] = at("plain|");
  assert.equal(continueList(t, p, p), null);
  assert.equal(continueList("- a", 0, 3), null);
  [t, p] = at("-| x");
  assert.equal(continueList(t, p, p), null);
  [t, p] = at("```\n- in code|\n```");
  assert.equal(continueList(t, p, p), null);
  [t, p] = at("-x|");
  assert.equal(continueList(t, p, p), null);
});

test("検索は文字そのもので、大文字小文字の区別を選べる", () => {
  const t = "Foo foo FOO a.b a+b";
  assert.equal(findRanges(t, "foo", false).ranges.length, 3);
  assert.equal(findRanges(t, "foo", true).ranges.length, 1);
  assert.equal(findRanges(t, "a.b", false).ranges.length, 1); // . は任意の文字ではない
  assert.equal(findRanges(t, "a+b", false).ranges.length, 1);
  assert.equal(findRanges(t, "", false).ranges.length, 0);
  assert.deepEqual(findRanges("aaa", "a", true).ranges[2], { start: 2, end: 3 });
});
test("次・前の一致は端で回る", () => {
  const { ranges } = findRanges("a_a_a", "a", true);
  assert.equal(pickMatch(ranges, 0, false), 0);
  assert.equal(pickMatch(ranges, 1, false), 1);
  assert.equal(pickMatch(ranges, 5, false), 0);
  assert.equal(pickMatch(ranges, 2, true), 0);
  assert.equal(pickMatch(ranges, 0, true), 2);
  assert.equal(pickMatch([], 0, false), -1);
});
test("すべて置換は件数を返し、$ などを特別扱いしない", () => {
  assert.deepEqual(replaceAllText("a b a", "a", "$&x", true), { text: "$&x b $&x", count: 2 });
  assert.deepEqual(replaceAllText("A a", "a", "z", false), { text: "z z", count: 2 });
  assert.deepEqual(replaceAllText("abc", "", "z", false), { text: "abc", count: 0 });
});

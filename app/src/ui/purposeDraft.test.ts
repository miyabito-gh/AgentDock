// 実行: node --test src/ui/purposeDraft.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { assembleMemo, lineDiff, memoLine, stateHeadline, elapsedText } from "./purposeDraft.ts";
import { charCount } from "./contUsage.ts";

const P = (index: number, over: Record<string, unknown>) =>
  ({ index, kind: "decision", action: "add", text: "本文", replaces: "", sources: [12], noEvidence: false, note: "", issues: [], defaultPick: true, ...over }) as never;
const all = (...ps: { index: number }[]) => new Set(ps.map((p) => p.index));

test("an add goes to the end of an existing section and keeps every original line", () => {
  const memo = "■ 決まったこと\n- 旧い決定\n\n■ 前提・制約\n- 制約A\n";
  const p = P(0, { text: "締切は10/30", sources: [12, 15] });
  const out = assembleMemo(memo, [p], all(p), "global");
  assert.equal(out.text, "■ 決まったこと\n- 旧い決定\n- 締切は10/30 [#12][#15]\n\n■ 前提・制約\n- 制約A\n");
  assert.deepEqual(out.skipped, []);
});

test("a missing section is created at the end of the memo; several adds keep their order", () => {
  const a = P(0, { kind: "openIssue", text: "宛先を確認" }), b = P(1, { kind: "openIssue", text: "金額を確認", sources: [] });
  const out = assembleMemo("目的の文", [a, b], all(a, b), "global");
  assert.equal(out.text, "目的の文\n\n■ 未解決の課題\n- 宛先を確認 [#12]\n- 金額を確認（根拠なし）");
  // 空のメモからは、節だけで始まる
  assert.equal(assembleMemo("", [a], all(a), "global").text, "■ 未解決の課題\n- 宛先を確認 [#12]");
});

test("replace and remove change exactly one matching line; unchosen proposals change nothing (DA7)", () => {
  const memo = "■ 決まったこと\n  - 旧い決定\n- 残す行\n- 消す行\n";
  const rep = P(0, { action: "replace", replaces: "- 旧い決定", text: "新しい決定", sources: [3] });
  const rm = P(1, { action: "remove", replaces: "- 消す行", text: "" });
  const not = P(2, { action: "add", text: "選ばない" });
  const out = assembleMemo(memo, [rep, rm, not], new Set([0, 1]), "global");
  assert.equal(out.text, "■ 決まったこと\n  - 新しい決定 [#3]\n- 残す行\n");
  assert.equal(assembleMemo(memo, [rep, rm, not], new Set(), "global").text, memo, "nothing chosen = byte-identical");
});

test("a target that is missing, ambiguous or already used is skipped, not guessed", () => {
  const memo = "- 同じ行\n- 同じ行\n- 一意\n";
  const dup = P(0, { action: "remove", replaces: "- 同じ行" }), none = P(1, { action: "replace", replaces: "- 無い行" });
  const first = P(2, { action: "remove", replaces: "- 一意" }), second = P(3, { action: "replace", replaces: "- 一意" });
  const out = assembleMemo(memo, [dup, none, first, second], new Set([0, 1, 2, 3]), "global");
  assert.deepEqual(out.skipped, [0, 1, 3]);
  assert.equal(out.text, "- 同じ行\n- 同じ行\n");
});

test("head-only drafts cite with the thread-local mark", () => {
  assert.equal(memoLine({ text: "x", sources: [3] }, "headOnly"), "- x [今#3]");
});

test("lineDiff marks added and removed lines and keeps unchanged ones", () => {
  const d = lineDiff("a\nb\nc", "a\nc\nd");
  assert.deepEqual(d.map((l) => `${l.kind}:${l.text}`), ["same:a", "del:b", "same:c", "add:d"]);
  assert.deepEqual(lineDiff("", "x"), [{ kind: "add", text: "x" }]);
  assert.deepEqual(lineDiff("x", "x"), [{ kind: "same", text: "x" }]);
});

test("the character count treats surrogate pairs as one character", () => {
  assert.equal(charCount("あ𠮷😀"), 3);
});

test("state headlines never call an unconfirmed result done", () => {
  assert.match(stateHeadline({ kind: "lost" }).text, /結果は不明/);
  assert.equal(stateHeadline({ kind: "lost" }).tone, "warn");
  assert.match(stateHeadline({ kind: "ready", proposals: [], omitted: "", dropped: 0, reviewCount: 2 }).text, /確認待ち（要確認 2 件）/);
  assert.equal(stateHeadline({ kind: "ready", proposals: [], omitted: "", dropped: 0, reviewCount: 0 }).text, "確認待ち");
  assert.match(stateHeadline({ kind: "acceptanceUnknown" }).text, /確認できません/);
  assert.equal(elapsedText(0, 65_000), "1:05");
});

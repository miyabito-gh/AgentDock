// 実行: node --test src/ui/creating.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { addCreating, foundSince, creatingLabel, removeCreating, resolveCreating, resultOfError, setCreatingPhase, type CreatingChat } from "./creating.ts";

const mk = (id: string): CreatingChat<null> => ({ tempId: id, label: id, startedAt: 0, phase: "starting", input: null, result: null });

test("追加は先頭、段階の更新、成功で削除", () => {
  let l = addCreating(addCreating([], mk("a")), mk("b"));
  assert.deepEqual(l.map((c) => c.tempId), ["b", "a"]);
  l = setCreatingPhase(l, "a", "worktree");
  assert.equal(l[1].phase, "worktree");
  l = removeCreating(l, "b");
  assert.deepEqual(l.map((c) => c.tempId), ["a"]);
  assert.equal(removeCreating(l, "zzz"), l);
});

test("失敗・不明は結果を持ち、その後の段階更新で動かない", () => {
  let l = [mk("a")];
  l = resolveCreating(l, "a", { kind: "failed", message: "x" });
  assert.equal(l[0].result?.kind, "failed");
  assert.equal(setCreatingPhase(l, "a", "worktree")[0].phase, "starting");
  l = resolveCreating(l, "a", { kind: "unknown", message: "y" });
  assert.equal(l[0].result?.kind, "unknown");
});

test("エラーの種類で失敗と不明を分ける", () => {
  assert.equal(resultOfError("rejected", "m").kind, "failed");
  assert.equal(resultOfError("blocked", "m").kind, "failed");
  assert.equal(resultOfError("outcomeUnknown", "m").kind, "unknown");
  assert.equal(resultOfError("io", "m").kind, "unknown");
});

test("表示名: 依頼の先頭行、フォルダ名、既定", () => {
  assert.equal(creatingLabel("\n  hello\nworld", "C:\\a\\b"), "hello");
  assert.equal(creatingLabel(null, "C:\\a\\proj\\"), "proj");
  assert.equal(creatingLabel(null, null), "新しいチャット");
});

test("foundSince: 作成開始以降のチャットを数える（cwd 指定時は同じフォルダだけ）", () => {
  const v = (n: number) => ({ kind: "value", value: n });
  const cwd = (s: string) => ({ kind: "value", value: s });
  const items = [
    { createdAt: v(10_000), cwd: cwd("C:/Work/A") },
    { createdAt: v(1_000), cwd: cwd("C:/Work/A") },
    { createdAt: { kind: "notFetched" }, cwd: cwd("C:/Work/A") },
    { createdAt: v(10_500), cwd: cwd("C:/Other") },
  ];
  assert.equal(foundSince(items, 10_000, null), 2);
  assert.equal(foundSince(items, 10_000, "c:/work/a/"), 1);
  assert.equal(foundSince([], 10_000, null), 0);
});

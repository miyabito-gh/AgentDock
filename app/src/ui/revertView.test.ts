// 実行: node --test src/ui/revertView.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { reasonLines, sortedItems, initialChosen, pickedPaths, forcedPaths, resultKind, turnOptions, noBaselineReason } from "./revertView.ts";

const item = (path: string, verdict: any) => ({ path, kind: null, includesTurns: [], verdict });
const rev = { kind: "revertible", summary: "" };
const need = (...codes: string[]) => ({ kind: "needsOverride", summary: "", reasons: codes.map((code) => ({ code, message: code })) });
const blk = { kind: "blocked", code: "headMoved", message: "m" };

test("要確認の理由は落とさず固定順", () => {
  const l = reasonLines(need("concurrentChange", "changedAfter", "endUnknown") as any);
  assert.deepEqual(l.map((x) => x.code), ["endUnknown", "changedAfter", "concurrentChange"]);
});
test("止めるは理由1件、戻せるは空", () => {
  assert.equal(reasonLines(blk as any)[0].label, "HEADが変わった（コミット・ブランチ切替など）");
  assert.equal(reasonLines(rev as any).length, 0);
});
test("並びは 戻せる→要確認→止める、同区分は元の順", () => {
  const s = sortedItems([item("a", blk), item("b", need("changedAfter")), item("c", rev), item("d", rev)] as any);
  assert.deepEqual(s.map((i) => i.path), ["c", "d", "b", "a"]);
});
test("初期選択は戻せるだけ。要確認は初期オフ", () => {
  const items = [item("a", rev), item("b", need("changedAfter")), item("c", blk)] as any;
  const ch = initialChosen(items);
  assert.deepEqual(ch, { a: true });
  assert.deepEqual(pickedPaths(items, ch), ["a"]);
  assert.deepEqual(forcedPaths(items, ch), []);
  assert.deepEqual(forcedPaths(items, { ...ch, b: true, c: true }), ["b"]);
});
test("失敗が1件でもあれば一部のみ", () => {
  assert.equal(resultKind({ reverted: ["a"], failed: [{}] }), "partial");
  assert.equal(resultKind({ reverted: ["a"], failed: [] }), "allReverted");
  assert.equal(resultKind({ reverted: [], failed: [] }), "nothing");
});
test("turn選択肢: 失敗・送信なし・未対応づけは出さない", () => {
  const seg = (turn: string | null, state: any, concurrent = false) => ({ turn, startedAt: 1, state, concurrent });
  const o = turnOptions([seg("t1", { kind: "ready" }), seg("t2", { kind: "failed", reason: { kind: "timeout" } }), seg(null, { kind: "ready" }), seg("t3", { kind: "endUnknown" }, true), seg("t1", { kind: "ready" })] as any);
  assert.deepEqual(o.map((x) => x.turn), ["t1", "t3"]);
  assert.match(o[1].label, /同時作業/);
});
test("控えなしの理由: 取得中はnull、空は理由、失敗のみは失敗理由", () => {
  assert.equal(noBaselineReason(null), null);
  assert.ok(noBaselineReason([]));
  assert.match(noBaselineReason([{ turn: "t", startedAt: 1, state: { kind: "failed", reason: { kind: "notARepository" } }, concurrent: false }] as any)!, /Gitリポジトリ/);
  assert.equal(noBaselineReason([{ turn: "t", startedAt: 1, state: { kind: "ready" }, concurrent: false }] as any), null);
});

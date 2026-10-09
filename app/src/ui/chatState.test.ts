// 実行: node --test src/ui/chatState.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { chatDisplayKind, dockRank, sortByDockRank, unknownCount, unknownSignature, validOutcome, parkGroup, historyBreakdown, historyBreakdownText } from "./chatState.ts";

test("親done＋子running→子が作業中", () => assert.equal(chatDisplayKind("done", ["running"]), "childWorking"));
test("親done＋孫waiting→子が作業中", () => assert.equal(chatDisplayKind("done", ["done", "waiting"]), "childWorking"));
test("親done＋子全てdone→親の状態", () => assert.equal(chatDisplayKind("done", ["done", "closed"]), "root"));
test("親done＋子unknown→親の状態（注記だけ添える）", () => assert.equal(chatDisplayKind("done", ["done", "unknown"]), "root"));
test("状態不明の子の件数", () => assert.equal(unknownCount(["done", "unknown", "unknown", "running"]), 2));
test("並び: 実行中・対応待ち→失敗・中断→完了→状態不明、同順位は元の順", () => {
  const items = [["a", "unknown"], ["b", "done"], ["c", "failed"], ["d", "running"], ["e", "waiting"], ["f", "interrupted"], ["g", "idle"]] as const;
  const out = sortByDockRank([...items], (x) => x[1]).map((x) => x[0]);
  assert.deepEqual(out, ["d", "e", "c", "f", "b", "g", "a"]);
});
test("dockRank", () => { assert.equal(dockRank("initializing"), 0); assert.equal(dockRank("closed"), 2); assert.equal(dockRank("unknown"), 3); });
test("確認済みの署名は活動が進むと変わる", () => {
  assert.equal(unknownSignature("notLoaded", "t1", 5), unknownSignature("notLoaded", "t1", 5));
  assert.notEqual(unknownSignature("notLoaded", "t1", 5), unknownSignature("notLoaded", "t2", 5));
  assert.notEqual(unknownSignature("notLoaded", "t1", 5), unknownSignature("notLoaded", "t1", 9));
});
test("親done＋子unknownとrunning→作業中が優先", () => assert.equal(chatDisplayKind("done", ["unknown", "running"]), "childWorking"));
test("親running＋子failed→子の失敗あり", () => assert.equal(chatDisplayKind("running", ["failed"]), "childFail"));
test("親running＋子running→親の状態", () => assert.equal(chatDisplayKind("running", ["running"]), "root"));
test("親failed＋子running→失敗を隠さない", () => assert.equal(chatDisplayKind("failed", ["running"]), "root"));
test("子なし→親の状態", () => assert.equal(chatDisplayKind("done", []), "root"));

const conf = (outcome: "completed" | "failed" | "interrupted" | "noTurns", turn: string | null) => ({ agent: { backend: "codex", id: "x" }, outcome, turn, confirmedAt: 1 }) as never;
test("履歴で終端確認した状態不明は、完了の位置（unknownの手前）、失敗・中断は失敗と同順位", () => {
  const items = [["u", "unknown", null], ["uc", "unknown", "completed"], ["uf", "unknown", "failed"], ["d", "done", null], ["f", "failed", null], ["r", "running", null]] as const;
  const out = sortByDockRank([...items], (x) => x[1], (x) => x[2]).map((x) => x[0]);
  assert.deepEqual(out, ["r", "uf", "f", "uc", "d", "u"]);
});
test("確認は状態不明のときだけ有効。新しいturnが来たら無効", () => {
  assert.equal(validOutcome("unknown", "t1", conf("completed", "t1")), "completed");
  assert.equal(validOutcome("unknown", "t2", conf("completed", "t1")), null);
  assert.equal(validOutcome("done", "t1", conf("completed", "t1")), null);
  assert.equal(validOutcome("unknown", null, conf("noTurns", null)), "noTurns");
  assert.equal(validOutcome("unknown", "t1", conf("noTurns", null)), null);
  assert.equal(validOutcome("unknown", "t1", undefined), null);
});

test("グループ振り分け: 状態不明のうち履歴確認済みは履歴グループ、未確認は状態不明グループ", () => {
  assert.equal(parkGroup("unknown", "completed"), "history");
  assert.equal(parkGroup("unknown", "failed"), "history");
  assert.equal(parkGroup("unknown", null), "unknown");
  assert.equal(parkGroup("done", null), "tree");
});
test("履歴確認の内訳と注意表示", () => {
  const b = historyBreakdown(["completed", "completed", "failed", "noTurns"]);
  assert.equal(historyBreakdownText(b), "完了2／失敗1／中断0／turnなし1");
  assert.equal(b.attention, true);
  assert.equal(historyBreakdown(["completed"]).attention, false);
});

// 実行: node --test src/ui/decisions.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { addDecision, decisionLine, mergeDecisions, placeDecisions, slotKey, type DecisionItem } from "./decisions.ts";

const rec = (id: string, at: number, over: Record<string, unknown> = {}) => ({
  id, chat: { backend: "codex", id: "c" }, agent: { backend: "codex", id: "child" }, agentName: "Kant", turn: "t1", kind: "command", summary: "git status",
  outcome: { kind: "allowed", scope: "once" }, requestedAt: at - 1, at, ...over,
}) as unknown as DecisionItem["record"];
const item = (id: string, at: number, over: Record<string, unknown> = {}, save: DecisionItem["save"] = { kind: "saved" }): DecisionItem => ({ record: rec(id, at, over), save });
const turn = (agentId: string, turnId: string, startedAt: number | null) => ({ key: { agent: { id: agentId }, turnId }, startedAt: startedAt === null ? { kind: "notFetched" } : { kind: "value", value: startedAt } });

test("読み込んだ記録と通知を記録IDで合わせ、時刻順にする（通知の保存結果を優先）", () => {
  const failed: DecisionItem = item("b", 20, {}, { kind: "failed", message: "容量不足" });
  const m = mergeDecisions([rec("a", 10), rec("b", 20)], [failed, item("c", 15)]);
  assert.deepEqual(m.map((x) => x.record.id), ["a", "c", "b"]);
  assert.equal(m[2].save.kind, "failed");
  assert.equal(addDecision(m, item("d", 5))[0].record.id, "d");
});

test("記録は同じ(agent, turn)の末尾へ。turn不明は時刻が開始より後の最後のturnへ、どれより前なら先頭", () => {
  const turns = [turn("root", "t0", 100), turn("child", "t1", 200), turn("root", "t2", 300)];
  const p = placeDecisions(turns, [
    item("x", 250), // child/t1
    item("y", 400, { turn: null }), // 時刻から t2
    item("z", 50, { turn: null }), // どのturnより前
    item("w", 260, { turn: "t9" }), // 履歴にまだ無いturn → 時刻から child/t1
  ]);
  assert.deepEqual(p.byTurn.get(slotKey("child", "t1"))?.map((x) => x.record.id), ["x", "w"]);
  assert.deepEqual(p.byTurn.get(slotKey("root", "t2"))?.map((x) => x.record.id), ["y"]);
  assert.deepEqual(p.head.map((x) => x.record.id), ["z"]);
});

test("開始時刻が未取得のturnだけでは、turn不明の記録は先頭に置く（推測しない）", () => {
  const p = placeDecisions([turn("root", "t0", null)], [item("a", 10, { turn: null })]);
  assert.equal(p.head.length, 1);
});

test("1行の文言: 種類・要約・応答・要求元。保存失敗は別に返す", () => {
  assert.equal(decisionLine(item("a", 1), "Kant").text, "承認: コマンド git status → 許可（今回のみ）［Kant］");
  assert.equal(decisionLine(item("a", 1, { agentName: null }), null).text, "承認: コマンド git status → 許可（今回のみ）");
  assert.equal(decisionLine(item("a", 1, { agentName: null }), "現在の名前").text.endsWith("［現在の名前］"), true);
  assert.equal(decisionLine(item("a", 1, { outcome: { kind: "resolvedElsewhere" } }), null).text.includes("別の場所で解決"), true);
  assert.equal(decisionLine(item("a", 1, {}, { kind: "failed", message: "x" }), null).failed, "x");
  assert.equal(decisionLine(item("a", 1), null).failed, null);
});

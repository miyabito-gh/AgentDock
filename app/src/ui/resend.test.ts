// 実行: node --test src/ui/resend.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { REASON, resendAvailability, type ResendCtx, type ResendTarget } from "./resend.ts";

const ok = (): ResendCtx => ({
  capProblem: null, connected: true, deletePending: false, externalRunning: false, stopUnconfirmed: false, busy: false, rootUnknown: false,
  turns: [{ turnId: "t1", end: "completed", userCount: 1 }, { turnId: "t2", end: "completed", userCount: 1 }, { turnId: "t3", end: "completed", userCount: 1 }],
});
const edit = (turnId: string, userIndex = 0): ResendTarget => ({ turnId, purpose: "editResend", userIndex, requestText: "依頼" });
const regen = (turnId: string): ResendTarget => ({ turnId, purpose: "regenerate", userIndex: null, requestText: "依頼" });
const reason = (c: ResendCtx, t: ResendTarget) => {
  const p = resendAvailability(c, t);
  return p.kind === "blocked" ? p.reason : p.kind;
};

test("途中のturnは直前のturnで分岐する", () => {
  assert.deepEqual(resendAvailability(ok(), edit("t3")), { kind: "fork", throughTurn: "t2", targetTurn: "t3", turnNo: 3 });
  assert.deepEqual(resendAvailability(ok(), regen("t2")), { kind: "fork", throughTurn: "t1", targetTurn: "t2", turnNo: 2 });
});

test("最初のturnは分岐せず、新しいチャットの経路", () => {
  assert.deepEqual(resendAvailability(ok(), edit("t1")), { kind: "first", targetTurn: "t1", turnNo: 1 });
});

test("理由は定められた順で最初の1つだけ", () => {
  const c = ok();
  c.capProblem = "このAIでは非対応です";
  c.connected = false; c.deletePending = true; c.externalRunning = true; c.stopUnconfirmed = true; c.busy = true; c.rootUnknown = true;
  assert.equal(reason(c, edit("t3")), "このAIでは非対応です");
  c.capProblem = null;
  assert.equal(reason(c, edit("t3")), REASON.notConnected);
  c.connected = true;
  assert.equal(reason(c, edit("t3")), REASON.deletePending);
  c.deletePending = false;
  assert.equal(reason(c, edit("t3")), REASON.externalRunning);
  c.externalRunning = false;
  assert.equal(reason(c, edit("t3")), REASON.stopUnconfirmed);
  c.stopUnconfirmed = false;
  assert.equal(reason(c, edit("t3")), REASON.busy);
  c.busy = false;
  assert.equal(reason(c, edit("t3")), REASON.stateUnknown);
  c.rootUnknown = false;
  assert.equal(reason(c, edit("t3")), "fork");
});

test("実行中のチャットでは使えない（合意）", () => {
  const c = ok();
  c.busy = true;
  assert.equal(reason(c, regen("t3")), REASON.busy);
  assert.equal(reason(c, edit("t1")), REASON.busy);
});

test("表示にないturnは古い表示として止める", () => {
  assert.equal(reason(ok(), edit("t9")), REASON.stale);
});

test("追加指示を含むturnと、追加指示そのものの編集", () => {
  const c = ok();
  c.turns[2].userCount = 2;
  assert.equal(reason(c, edit("t3")), REASON.multiple);
  assert.equal(reason(c, regen("t3")), REASON.multiple);
  assert.equal(reason(c, edit("t3", 1)), REASON.followUp);
});

test("依頼の文を取得できていないときは使えない", () => {
  assert.equal(reason(ok(), { ...edit("t2"), requestText: null }), REASON.noText);
});

test("直前のturnの終端が未確認なら分岐できない（最初のturnは対象外）", () => {
  const c = ok();
  c.turns[1].end = null;
  assert.equal(reason(c, edit("t3")), REASON.previousNotEnded);
  assert.equal(reason(c, edit("t1")), "first");
  c.turns[0].end = "failed";
  assert.equal(reason(c, edit("t2")), "fork");
});

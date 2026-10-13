// 実行: node --test src/ui/pendingSend.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { rebindPending, pendingView, stageLabel, outcomeOfAttempt, reconcilePending, setPhase, hasInFlight, appearsInTurns, SLOW_AFTER_MS, type PendingSend } from "./pendingSend.ts";

const base = (o: Partial<PendingSend> = {}): PendingSend => ({ id: "p1", text: "こんにちは", startedAt: 1000, steer: false, phase: null, outcome: null, ...o });
const att = (state: unknown, id = "att1") => ({ attemptId: id, clientMessageId: "cm", at: 1, state }) as never;

test("応答待ち: 3秒未満は印だけ（経過秒・段階を出さない）", () => {
  const v = pendingView(base({ phase: "resuming" }), 1000 + SLOW_AFTER_MS - 1);
  assert.deepEqual(v, { kind: "sending", slow: false, elapsedSec: 2, stage: null });
});

test("応答待ち: 3秒以上は経過秒と、取れている段階だけ出す", () => {
  assert.deepEqual(pendingView(base({ phase: "resuming" }), 1000 + 12_400), { kind: "sending", slow: true, elapsedSec: 12, stage: "会話を再開しています" });
  // 段階が取れていなければ書かない
  assert.deepEqual(pendingView(base(), 1000 + 5000), { kind: "sending", slow: true, elapsedSec: 5, stage: null });
});

test("段階の文言", () => {
  assert.equal(stageLabel("preparing"), "送信の準備をしています");
  assert.equal(stageLabel("awaitingAcceptance"), "Codex が受理するのを待っています");
  assert.equal(stageLabel(null), null);
});

test("時計が戻っても経過秒は負にならない", () => {
  assert.equal((pendingView(base(), 0) as { elapsedSec: number }).elapsedSec, 0);
});

test("結果の写し: 受理だけが accepted。拒否は未送信、受理不明は再送させない", () => {
  assert.deepEqual(outcomeOfAttempt(att({ kind: "accepted", turn: { agent: { backend: "codex", id: "t" }, turnId: "u1" } })), { kind: "accepted", turnId: "u1" });
  assert.equal(outcomeOfAttempt(att({ kind: "sending" })), null);
  const rej = outcomeOfAttempt(att({ kind: "rejected", message: "x" }))!;
  assert.deepEqual(pendingView(base({ outcome: rej }), 5000), { kind: "unsent", reason: "x", canRestore: true });
  const unk = outcomeOfAttempt(att({ kind: "acceptanceUnknown", since: 1 }))!;
  assert.deepEqual(pendingView(base({ outcome: unk }), 5000), { kind: "unknown", canRestore: false });
  assert.deepEqual(pendingView(base({ outcome: { kind: "failed", message: "m" } }), 5000), { kind: "unsent", reason: "m", canRestore: true });
});

test("受理不明の行は、同じ試行の照合結果で確定する（別の試行・不明のままは変えない）", () => {
  const list = [base({ outcome: { kind: "unknown", attemptId: "att1" } })];
  assert.equal(reconcilePending(list, att({ kind: "acceptanceUnknown", since: 1 })), list);
  assert.equal(reconcilePending(list, att({ kind: "rejected", message: "m" }, "other")), list);
  const done = reconcilePending(list, att({ kind: "notFoundAfterReconcile" }));
  assert.equal(done[0].outcome?.kind, "rejected");
  const ok = reconcilePending(list, att({ kind: "accepted", turn: { agent: { backend: "codex", id: "t" }, turnId: "u9" } }));
  assert.deepEqual(ok[0].outcome, { kind: "accepted", turnId: "u9" });
});

test("段階の更新は応答待ちの行だけ。二重送信の判定", () => {
  const waiting = base();
  const done = base({ id: "p2", outcome: { kind: "failed", message: "m" } });
  const r = setPhase([waiting, done], "awaitingAcceptance");
  assert.equal(r[0].phase, "awaitingAcceptance");
  assert.equal(r[1].phase, null);
  assert.equal(setPhase([done], "resuming")[0], done);
  assert.equal(hasInFlight([done]), false);
  assert.equal(hasInFlight([done, waiting]), true);
});

test("受理後の仮の行は、本文に現れたら外す（新しいturnはturn ID、追加指示は同じ文）", () => {
  const turn = (id: string, texts: string[]) => ({ key: { turnId: id }, entries: texts.map((t) => ({ kind: { kind: "userMessage" }, text: { kind: "value", value: t } })) });
  const acc = (steer: boolean) => base({ steer, outcome: { kind: "accepted", turnId: "u1" } });
  assert.equal(appearsInTurns(acc(false), []), false);
  assert.equal(appearsInTurns(acc(false), [turn("u1", [])]), true);
  assert.equal(appearsInTurns(acc(true), [turn("u1", ["最初の依頼"])]), false);
  assert.equal(appearsInTurns(acc(true), [turn("u1", ["最初の依頼", " こんにちは "])]), true);
  assert.equal(appearsInTurns(base(), [turn("u1", ["こんにちは"])]), false);
});

test("再送の結果は元の仮の行へ結び付く（受理なら未送信のまま残さない）。再送できる間は戻すを出さない", () => {
  const rej = base({ outcome: { kind: "rejected", attemptId: "a1", message: "x" } });
  const ok = rebindPending([rej], "a1", att({ kind: "accepted", turn: { agent: { backend: "codex", id: "t" }, turnId: "u2" } }, "a2"));
  assert.deepEqual(ok[0].outcome, { kind: "accepted", turnId: "u2" });
  const again = rebindPending([rej], "a1", att({ kind: "rejected", message: "y" }, "a2"));
  assert.deepEqual(again[0].outcome, { kind: "rejected", attemptId: "a2", message: "y" });
  const unk = rebindPending([rej], "a1", att({ kind: "acceptanceUnknown", since: 1 }, "a2"));
  assert.deepEqual(unk[0].outcome, { kind: "unknown", attemptId: "a2" });
  const same = [rej];
  assert.equal(rebindPending(same, "zz", att({ kind: "rejected", message: "y" }, "a2")), same);
  assert.equal((pendingView({ ...rej, retryable: true }, 5000) as { canRestore: boolean }).canRestore, false);
  assert.equal((pendingView(rej, 5000) as { canRestore: boolean }).canRestore, true);
});

// 実行: node --test src/ui/queueUi.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { parentResumeOffer, recheckOffered, staleWorkingDescendants, steerBlockedNote, steerBlockReason, steerButtonOffered } from "./queueUi.ts";

const root = "chat-1";
const tgt = (id: string, freshness: "live" | "historyOnly" = "historyOnly") => ({ agent: { backend: "codex", id }, state: "idle", freshness });
const q = (hold: unknown, run: { kind: string } = { kind: "active" }) => ({ chat: { backend: "codex", id: root }, run, hold }) as unknown as Parameters<typeof parentResumeOffer>[0];

test("親が原因の保留（notLive に親が含まれる）では「このチャットを再開して送る」を出す", () => {
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt(root)] })), true);
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt("child"), tgt(root)] })), true);
});

test("親が取れない保留（stateUnknown で対象が空、または親だけが対象）でも出す。子孫の探索未完了では出さない", () => {
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [], descendantScanIncomplete: false })), true);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root)], descendantScanIncomplete: false })), true);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [], descendantScanIncomplete: true })), false);
});

// 仕様変更（2026-11-27、まさとの決定）: 親が live のまま状態が不明・終了の保留（受信は戻っているので、従来は出口がなかった）にも、
// 親が live でない保留と同じ出口（再開）を出す。以前は「出さない」ことをここで固定していた。
test("親が live のまま状態が不明・終了の保留（stateUnknown で親だけが対象・live）にも「このチャットを再開して送る」を出す", () => {
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [{ ...tgt(root, "live"), state: "unknown" }], descendantScanIncomplete: false })), true);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [{ ...tgt(root, "live"), state: "closed" }], descendantScanIncomplete: false })), true);
  // ただし、停止中・子孫の探索未完了・子孫が混じる保留では従来どおり出さない。
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root, "live")], descendantScanIncomplete: true })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root, "live")], descendantScanIncomplete: false }, { kind: "stopped" })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root, "live"), tgt("child", "live")], descendantScanIncomplete: false })), false);
});

test("子孫だけが原因の保留、保留なし、停止中、他の理由の保留では出さない", () => {
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt("child")] })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt("child")], descendantScanIncomplete: false })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root), tgt("child")], descendantScanIncomplete: false })), false);
  assert.equal(parentResumeOffer(q(null)), false);
  assert.equal(parentResumeOffer(q({ kind: "parentWorking" })), false);
  assert.equal(parentResumeOffer(q({ kind: "disconnected" })), false);
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt(root)] }, { kind: "stopped" })), false, "停止中は別バナーの「キューを再開」");
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt(root)] }, { kind: "pausedAfterRestart" })), false);
});

const work = (id: string, state: string, freshness: "live" | "historyOnly" = "historyOnly") => ({ ...tgt(id, freshness), state });

test("作業中と記録されたまま live でない子孫だけが原因の保留（notLive）は、子孫として扱い、「状態を再確認」は出さず、理由の説明を出す", () => {
  const stale = q({ kind: "notLive", targets: [work("child", "running")] });
  assert.equal(parentResumeOffer(stale), false, "親の再開では解けない");
  assert.deepEqual(staleWorkingDescendants(stale).map((t) => t.agent.id), ["child"]);
  assert.equal(recheckOffered(stale), false, "履歴を読み直しても記録（作業中）は変わらない");
  assert.equal(recheckOffered(q({ kind: "notLive", targets: [work("a", "waiting"), work("b", "initializing")] })), false);
});

test("作業中でない live でない子孫が混じる／親が対象／状態不明の保留では、従来どおり「状態を再確認」を出す", () => {
  assert.equal(recheckOffered(q({ kind: "notLive", targets: [work("child", "idle")] })), true);
  assert.equal(recheckOffered(q({ kind: "notLive", targets: [work("a", "running"), work("b", "idle")] })), true, "読み直して確認できる子孫もいる");
  assert.equal(staleWorkingDescendants(q({ kind: "notLive", targets: [work("a", "running"), work("b", "idle")] })).length, 1);
  assert.equal(recheckOffered(q({ kind: "stateUnknown", targets: [work("child", "unknown")], descendantScanIncomplete: false })), true);
  assert.equal(recheckOffered(q({ kind: "notLive", targets: [work(root, "running")] })), true, "親が対象なら、子孫の判定は使わない（親は再開で解く）");
  assert.equal(staleWorkingDescendants(q({ kind: "notLive", targets: [work(root, "running")] })).length, 0);
});

test("live で作業中の子孫の保留（descendantsActive）・他の保留・停止中では、出口を出さない（待つ）", () => {
  const active = q({ kind: "descendantsActive", targets: [work("child", "running", "live")] });
  assert.equal(recheckOffered(active), false);
  assert.equal(staleWorkingDescendants(active).length, 0);
  assert.equal(parentResumeOffer(active), false);
  assert.equal(recheckOffered(q(null)), false);
  assert.equal(recheckOffered(q({ kind: "parentWorking" })), false);
  assert.equal(recheckOffered(q({ kind: "notLive", targets: [work("child", "idle")] }, { kind: "stopped" })), false);
});

test("追加指示のボタンは Antigravity には最初から出さず、Codex には出す", () => {
  assert.equal(steerButtonOffered("antigravity"), false);
  assert.equal(steerButtonOffered("codex"), true);
});

test("追加指示にできない理由は、押したときの説明にそのまま使える（無言にしない）", () => {
  const why = steerBlockReason(false, false, undefined, "codex");
  assert.equal(why, "実行中のやりとりがないため、追加指示にはできません");
  assert.equal(steerBlockedNote(why!), "追加指示として送れません: 実行中のやりとりがないため、追加指示にはできません");
  assert.equal(steerBlockReason(true, false, undefined, "codex"), null);
});

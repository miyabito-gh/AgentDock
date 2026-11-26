// 実行: node --test src/ui/queueUi.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { parentResumeOffer, steerBlockedNote, steerBlockReason, steerButtonOffered } from "./queueUi.ts";

const root = "chat-1";
const tgt = (id: string, freshness: "live" | "historyOnly" = "historyOnly") => ({ agent: { backend: "codex", id }, state: "idle", freshness });
const q = (hold: unknown, run: { kind: string } = { kind: "active" }) => ({ chat: { backend: "codex", id: root }, run, hold }) as unknown as Parameters<typeof parentResumeOffer>[0];

test("親が原因の保留（notLive に親が含まれる）では「このチャットを再開して送る」を出す", () => {
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt(root)] })), true);
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt("child"), tgt(root)] })), true);
});

test("親が取れない保留（stateUnknown で対象が空、または親だけが live でない）でも出す。子孫の探索未完了では出さない", () => {
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [], descendantScanIncomplete: false })), true);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root)], descendantScanIncomplete: false })), true);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [], descendantScanIncomplete: true })), false);
});

test("子孫だけが原因の保留、親が live の状態不明、保留なし、停止中、他の理由の保留では出さない", () => {
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt("child")] })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt("child")], descendantScanIncomplete: false })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root, "live")], descendantScanIncomplete: false })), false);
  assert.equal(parentResumeOffer(q({ kind: "stateUnknown", targets: [tgt(root), tgt("child")], descendantScanIncomplete: false })), false);
  assert.equal(parentResumeOffer(q(null)), false);
  assert.equal(parentResumeOffer(q({ kind: "parentWorking" })), false);
  assert.equal(parentResumeOffer(q({ kind: "disconnected" })), false);
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt(root)] }, { kind: "stopped" })), false, "停止中は別バナーの「キューを再開」");
  assert.equal(parentResumeOffer(q({ kind: "notLive", targets: [tgt(root)] }, { kind: "pausedAfterRestart" })), false);
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

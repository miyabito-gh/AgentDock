// 実行: node --test src/pet/cardLogic.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { answersAtOnce, answersComplete, ARM_MS, chooseTarget, clip, isArmed, latestReply, optionTone, pauseLeftText, pendingRequests, recentChats } from "./cardLogic.ts";

const chat = (id: string, lastUsedAt: number | null, archived = false) => ({ key: { backend: "codex", id }, lastUsedAt, archived: { kind: "value", value: archived, basis: "direct" } }) as any;
const opt = (effect: string, scope: string) => ({ id: "x", label: "x", effect, scope, description: null }) as any;

test("送り先: 選んだもの > 最後に通知 > 選択中。一覧にないものは飛ばす", () => {
  const ids = new Set(["a", "b", "c"]);
  assert.equal(chooseTarget({ picked: "a", notified: "b", selected: "c" }, ids), "a");
  assert.equal(chooseTarget({ picked: null, notified: "b", selected: "c" }, ids), "b");
  assert.equal(chooseTarget({ picked: null, notified: null, selected: "c" }, ids), "c");
  assert.equal(chooseTarget({ picked: "gone", notified: "gone2", selected: "c" }, ids), "c");
  assert.equal(chooseTarget({ picked: null, notified: null, selected: null }, ids), null);
});

test("候補は最近順・アーカイブ除外で、いまの送り先は必ず含む", () => {
  const cs = [chat("a", 1), chat("b", 3), chat("c", 2), chat("z", 9, true)];
  assert.deepEqual(recentChats(cs, null).map((c) => c.key.id), ["b", "c", "a"]);
  assert.deepEqual(recentChats(cs, "a", 2).map((c) => c.key.id), ["a", "b"]);
});

test("回答待ちだけを古い順に", () => {
  const r = (id: string, at: number, kind = "pending") => ({ key: { requestId: id }, receivedAt: at, state: { kind } }) as any;
  assert.deepEqual(pendingRequests([r("2", 20), r("1", 10), r("x", 5, "answered")]).map((x) => x.key.requestId), ["1", "2"]);
});

test("選択肢の見た目: 今回だけの許可は主、広い許可は警告、拒否は枠線", () => {
  assert.equal(optionTone(opt("allow", "once")), "primary");
  assert.equal(optionTone(opt("allow", "unknown")), "primary");
  assert.equal(optionTone(opt("allow", "session")), "careful");
  assert.equal(optionTone(opt("allow", "persistent")), "careful");
  assert.equal(optionTone(opt("deny", "once")), "quiet");
  assert.equal(optionTone(opt("cancel", "once")), "quiet");
  assert.equal(optionTone(opt("other", "once")), "quiet");
});

test("誤タップ防止: 表示から0.5秒たつまで押せない", () => {
  assert.equal(isArmed(null, 1000), false);
  assert.equal(isArmed(1000, 1000 + ARM_MS - 1), false);
  assert.equal(isArmed(1000, 1000 + ARM_MS), true);
});

test("質問: 選択肢だけの1件は1回で確定、それ以外は全件そろってから", () => {
  assert.equal(answersAtOnce([{ options: [opt("allow", "once")], allowsFreeText: false }]), true);
  assert.equal(answersAtOnce([{ options: [opt("allow", "once")], allowsFreeText: true }]), false);
  assert.equal(answersAtOnce([{ options: [], allowsFreeText: true }]), false);
  assert.equal(answersAtOnce([{ options: [opt("allow", "once")], allowsFreeText: false }, { options: [opt("allow", "once")], allowsFreeText: false }]), false);
  const qs = [{ id: "q1" }, { id: "q2" }];
  assert.equal(answersComplete(qs, { q1: { questionId: "q1", optionId: "o", text: null } }), false);
  assert.equal(answersComplete(qs, { q1: { questionId: "q1", optionId: "o", text: null }, q2: { questionId: "q2", optionId: null, text: "  " } }), false);
  assert.equal(answersComplete(qs, { q1: { questionId: "q1", optionId: "o", text: null }, q2: { questionId: "q2", optionId: null, text: "はい" } }), true);
  assert.equal(answersComplete([], {}), false);
});

test("最新の返答: 後ろのターンの、本文があるエージェント発言", () => {
  const e = (kind: string, text: string | null, phase = "completed") => ({ kind: { kind }, text: text === null ? { kind: "notFetched" } : { kind: "value", value: text, basis: "direct" }, phase }) as any;
  const turns = [{ entries: [e("agentMessage", "古い")] }, { entries: [e("agentMessage", "新しい"), e("command", "ls"), e("agentMessage", "   "), e("agentMessage", null)] }] as any;
  assert.deepEqual(latestReply(turns), { text: "新しい", inProgress: false });
  assert.equal(latestReply([{ entries: [e("userMessage", "質問")] }] as any), null);
  assert.equal(latestReply(undefined), null);
  assert.equal(latestReply([{ entries: [e("agentMessage", "書き中", "inProgress")] }] as any)!.inProgress, true);
});

test("切り詰めと一時停止の表示", () => {
  assert.equal(clip("あいうえお", 5), "あいうえお");
  assert.equal(clip("あいうえおか", 5), "あいうえお…");
  assert.equal(pauseLeftText(null, 0), null);
  assert.equal(pauseLeftText(1000, 2000), null);
  assert.equal(pauseLeftText(30 * 60_000, 0), "あと30分");
  assert.equal(pauseLeftText(60 * 60_000, 0), "あと1時間");
  assert.equal(pauseLeftText(61 * 60_000, 1), "あと1時間1分");
});

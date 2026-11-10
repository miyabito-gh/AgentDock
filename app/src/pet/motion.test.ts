// 実行: node --test src/pet/motion.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { cellOf, choosePose, emptyTracker, frameAt, gazeCell, gazeIndex, trackAgents, MOTIONS, JUMP_MS, REVIEW_MS, WAVE_MS, FAILED_MS, type AgentLite, type PetInputs } from "./motion.ts";

const base: PetInputs = { now: 100_000, reducedMotion: false, drag: null, jumpAt: null, pending: false, failedAt: null, doneAt: null, running: false, gaze: null };
const name = (i: Partial<PetInputs>) => { const p = choosePose({ ...base, ...i }); return p.kind === "motion" ? p.name : `gaze${p.index}`; };

test("行・コマ数・時間は設計の表どおり", () => {
  assert.deepEqual(MOTIONS.idle.durations, [280, 110, 110, 140, 140, 320]);
  assert.deepEqual(MOTIONS.runningRight.durations, [120, 120, 120, 120, 120, 120, 120, 220]);
  assert.equal(MOTIONS.waving.durations.length, 4);
  assert.equal(MOTIONS.jumping.durations.length, 5);
  assert.equal(MOTIONS.failed.durations.at(-1), 240);
  assert.deepEqual([MOTIONS.idle.row, MOTIONS.runningRight.row, MOTIONS.runningLeft.row, MOTIONS.waving.row, MOTIONS.jumping.row, MOTIONS.failed.row, MOTIONS.waiting.row, MOTIONS.running.row, MOTIONS.review.row], [0, 1, 2, 3, 4, 5, 6, 7, 8]);
});

test("コマは経過時間でループする", () => {
  assert.deepEqual(frameAt("idle", 0), { col: 0, remaining: 280 });
  assert.deepEqual(frameAt("idle", 280), { col: 1, remaining: 110 });
  assert.equal(frameAt("idle", 1100).col, 0); // 1100ms で一周（280+110+110+140+140+320）
});

test("目線は上から時計回り22.5°刻み、0-7が行9・8-15が行10", () => {
  assert.equal(gazeIndex(0, -10), 0); // 上
  assert.equal(gazeIndex(10, 0), 4); // 右
  assert.equal(gazeIndex(0, 10), 8); // 下
  assert.equal(gazeIndex(-10, 0), 12); // 左
  assert.equal(gazeIndex(-1, -100), 0); // 上よりわずかに左は丸めて0
  assert.deepEqual(gazeCell(3), { row: 9, col: 3 });
  assert.deepEqual(gazeCell(8), { row: 10, col: 0 });
  assert.deepEqual(gazeCell(15), { row: 10, col: 7 });
});

test("優先順位: 待ち > 失敗 > 完了 > 実行中 > 待機", () => {
  const now = base.now;
  assert.equal(name({}), "idle");
  assert.equal(name({ running: true }), "running");
  assert.equal(name({ running: true, doneAt: now - 10 }), "review");
  assert.equal(name({ running: true, doneAt: now - 10, failedAt: now - 10 }), "failed");
  assert.equal(name({ running: true, doneAt: now - 10, failedAt: now - 10, pending: true }), "waiting");
});

test("正常終了は review → waving → idle。失敗の動きも時間で終わる", () => {
  const doneAt = base.now - 1;
  assert.equal(name({ doneAt }), "review");
  assert.equal(name({ doneAt, now: doneAt + REVIEW_MS + 1 }), "waving");
  assert.equal(name({ doneAt, now: doneAt + REVIEW_MS + WAVE_MS + 1 }), "idle");
  assert.equal(name({ failedAt: base.now - 1 }), "failed");
  assert.equal(name({ failedAt: base.now - FAILED_MS - 1 }), "idle");
});

test("ドラッグは向きの走り、クリックは1回だけ跳ぶ。操作は待ちより優先", () => {
  assert.equal(name({ drag: "left", pending: true }), "runningLeft");
  assert.equal(name({ drag: "right" }), "runningRight");
  assert.equal(name({ jumpAt: base.now - 10, pending: true }), "jumping");
  assert.equal(name({ jumpAt: base.now - JUMP_MS - 1 }), "idle");
});

test("目線は待機のときだけ。ほかの動きがあれば出ない", () => {
  assert.equal(name({ gaze: 4 }), "gaze4");
  assert.equal(name({ gaze: 4, running: true }), "running");
});

test("動きを減らす設定では idle の先頭コマ固定", () => {
  const p = choosePose({ ...base, reducedMotion: true, pending: true, running: true, drag: "left" });
  assert.deepEqual(cellOf(p, base.now), { row: 0, col: 0, remaining: null });
});

const ag = (key: string, state: AgentLite["state"], extra: Partial<AgentLite> = {}): AgentLite => ({ key, root: true, state, stale: false, ...extra });

test("最初の観測では完了・失敗の遷移を作らない", () => {
  const r = trackAgents(emptyTracker(), [ag("a", "done"), ag("b", "failed")], true);
  assert.equal(r.completed, false);
  assert.equal(r.failed, false);
  // スナップショット前（未準備）は取り込まない
  assert.equal(trackAgents(emptyTracker(), [ag("a", "running")], false).tracker.seeded, false);
});

test("ルートの作業中→done が完了、作業中→failed/interrupted が失敗。子は数えない", () => {
  let t = trackAgents(emptyTracker(), [ag("a", "running"), ag("c", "running", { root: false })], true).tracker;
  let r = trackAgents(t, [ag("a", "done"), ag("c", "failed", { root: false })], true);
  assert.equal(r.completed, true);
  assert.equal(r.failed, false);
  t = trackAgents(emptyTracker(), [ag("a", "running")], true).tracker;
  assert.equal(trackAgents(t, [ag("a", "interrupted")], true).failed, true);
  assert.equal(trackAgents(t, [ag("a", "failed")], true).failed, true);
});

test("作業中→状態不明・接続断は完了にせず、印だけ残す。作業に戻れば外す", () => {
  let t = trackAgents(emptyTracker(), [ag("a", "running")], true).tracker;
  let r = trackAgents(t, [ag("a", "unknown")], true);
  assert.equal(r.completed, false);
  assert.equal(r.failed, false);
  assert.deepEqual(r.tracker.lost, ["a"]);
  // 状態不明のまま続いても印は残る
  r = trackAgents(r.tracker, [ag("a", "unknown")], true);
  assert.deepEqual(r.tracker.lost, ["a"]);
  // 接続断（鮮度が落ちた）も同様。もともと不明だったものは印にしない
  t = trackAgents(emptyTracker(), [ag("b", "running"), ag("old", "unknown")], true).tracker;
  r = trackAgents(t, [ag("b", "running", { stale: true }), ag("old", "unknown")], true);
  assert.deepEqual(r.tracker.lost, ["b"]);
  r = trackAgents(r.tracker, [ag("b", "running"), ag("old", "unknown")], true);
  assert.deepEqual(r.tracker.lost, []);
});

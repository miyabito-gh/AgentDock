// 実行: node --test src/ui/orgActivity.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { activityLines, activityText, elapsedText, stabilize, type ActivityLine } from "./orgActivity.ts";

const st = (phase: string, label: string) => ({ phase, reason: null, label, icon: "play" });
const task = (id: string, phase: string, over: Record<string, unknown> = {}) =>
  ({ id, role: "worker", title: `title ${id}`, waitingOn: [], status: st(phase, phase), chat: null, since: null, held: false, attentionNote: null, ...over }) as never;
const wf = (tasks: unknown[]) => ({ tasks }) as never;
const ctx = (over: Record<string, unknown> = {}) => ({ now: 100_000, waitOf: () => null, activityOf: () => null, roleLabel: (r: string) => r, ...over }) as never;

test("経過時間は分単位で、1分未満・時間+分の形", () => {
  assert.equal(elapsedText(5_000), "1分未満");
  assert.equal(elapsedText(12 * 60_000 + 59_000), "12分");
  assert.equal(elapsedText(125 * 60_000), "2時間5分");
  assert.equal(elapsedText(120 * 60_000), "2時間");
  assert.equal(elapsedText(-5), "1分未満");
});

test("活動文: 種類ごとの語と、本文が取れないときの「内容は未取得」", () => {
  assert.equal(activityText({ live: true, kind: "command", summary: "cargo test" }), "コマンド: cargo test");
  assert.equal(activityText({ live: true, kind: "command", summary: null }), "コマンド（内容は未取得）");
  assert.equal(activityText({ live: true, kind: "reasoning", summary: null }), "考えています");
  assert.equal(activityText({ live: true, kind: "agentMessage", summary: "x" }), "回答を書いています");
  assert.equal(activityText(null), "（活動は未取得）");
});

test("並びは 対応待ち → 作業中 → 順番待ち。完了は件数だけ", () => {
  const r = activityLines(
    wf([task("t1", "completed"), task("t2", "pending", { waitingOn: ["t3"] }), task("t3", "running"), task("t4", "blocked"), task("t5", "failed"), task("t6", "ready", { held: true })]),
    ctx(),
  );
  assert.deepEqual(r.lines.map((l) => [l.task, l.group]), [["t4", "wait"], ["t3", "work"], ["t2", "queue"], ["t6", "queue"]]);
  assert.equal(r.done, 1);
  assert.equal(r.lines[2].detail, "t3 の完了待ち");
  assert.equal(r.lines[3].detail, "保留中");
});

test("鮮度は状態の文字を変えず、別の印（stale）にする", () => {
  const chat = { backend: "codex", id: "c" };
  const mk = (live: boolean) => activityLines(wf([task("t1", "running", { chat })]), ctx({ activityOf: () => ({ live, kind: "reasoning", summary: null }) })).lines[0];
  assert.equal(mk(true).stale, false);
  const off = mk(false);
  assert.equal(off.stale, true);
  assert.equal(off.state, mk(true).state);
});

test("対応待ちは理由の文を3列目に出す", () => {
  const r = activityLines(wf([task("t1", "running")]), ctx({ waitOf: () => "あなたの回答待ち（承認1）" }));
  assert.equal(r.lines[0].group, "wait");
  assert.equal(r.lines[0].detail, "あなたの回答待ち（承認1）");
});

const line = (over: Partial<ActivityLine>): ActivityLine => ({ task: "t1", roleLabel: "worker", title: "", group: "work", state: "実行中", detail: "a", since: null, stale: false, chat: null, detailAt: 0, ...over });

test("活動文は3秒は替えない。状態が変わったら即替える", () => {
  const prev = [line({ detail: "コマンド: a", detailAt: 1000 })];
  const held = stabilize(prev, [line({ detail: "考えています" })], 2500);
  assert.equal(held[0].detail, "コマンド: a");
  assert.equal(held[0].detailAt, 1000);
  const released = stabilize(prev, [line({ detail: "考えています" })], 4500);
  assert.equal(released[0].detail, "考えています");
  assert.equal(released[0].detailAt, 4500);
  const changed = stabilize(prev, [line({ state: "止まっています", group: "wait", detail: "要対応" })], 1500);
  assert.equal(changed[0].detail, "要対応");
  // 同じ文のままなら時刻は据え置き。新しいタスクはそのまま
  assert.equal(stabilize(prev, [line({ detail: "コマンド: a" })], 9000)[0].detailAt, 1000);
  assert.equal(stabilize([], [line({ task: "t9" })], 7)[0].detailAt, 7);
});

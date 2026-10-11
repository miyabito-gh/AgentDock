// 実行: node --test src/ui/chips.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { assignSends, SEND_AFTER_MS, SEND_BEFORE_MS } from "./chips.ts";

const ent = (id: string, text: string, atts: unknown[] = []) => ({ key: { itemId: id }, kind: { kind: "userMessage" }, text: text === "" ? { kind: "missing" } : { kind: "value", value: text }, attachments: atts as never[] });
const tn = (start: number | null, es: ReturnType<typeof ent>[], end: number | null = null) => ({
  startedAt: start === null ? { kind: "missing" } : { kind: "value", value: start },
  completedAt: end === null ? { kind: "missing" } : { kind: "value", value: end },
  entries: es,
});
const att = (name: string) => ({ name, kind: "image", path: "C:/c/" + name, basis: "structured" }) as never;
const sent = (text: string, at: number, atts: unknown[] = [att("a.png")], steer = false) => ({ text, at, atts: atts as never[], steer });

test("同じ本文の連続送信は、古い順に1対1で割り当てる", () => {
  const turns = [tn(1000, [ent("u1", "hi")]), tn(2000, [ent("u2", "hi")])];
  const m = assignSends(turns, [sent("hi", 1000, [att("a.png")]), sent("hi", 2000, [att("b.png")])]);
  assert.equal(m.get("u1")?.[0].name, "a.png");
  assert.equal(m.get("u2")?.[0].name, "b.png");
});

test("添付ありと添付なしの混在: 添付なしの送信も枠を消費し、添付が別の発言へ移らない", () => {
  // 先に添付なしで送り、後から添付つきで同じ本文を送った
  const turns = [tn(1000, [ent("u1", "hi")]), tn(2000, [ent("u2", "hi")])];
  const m = assignSends(turns, [sent("hi", 1000, []), sent("hi", 2000, [att("b.png")])]);
  assert.deepEqual(m.get("u1"), []);
  assert.equal(m.get("u2")?.[0].name, "b.png");
});

test("履歴に添付がある発言は置き換えず、その送信は消費される", () => {
  const real = [{ name: "a.png", kind: "image", path: "C:/c/a.png", basis: "structured" }];
  const turns = [tn(1000, [ent("u1", "hi", real)]), tn(2000, [ent("u2", "hi")])];
  const m = assignSends(turns, [sent("hi", 1000)]);
  assert.equal(m.has("u1"), false);
  assert.equal(m.has("u2"), false);
});

test("時刻窓の境界: 送信の前5秒・後10分まで", () => {
  const at = 100_000;
  const has = (start: number) => assignSends([tn(start, [ent("u", "hi")])], [sent("hi", at)]).has("u");
  assert.equal(has(at - SEND_BEFORE_MS), true);
  assert.equal(has(at - SEND_BEFORE_MS - 1), false);
  assert.equal(has(at + SEND_AFTER_MS), true);
  assert.equal(has(at + SEND_AFTER_MS + 1), false);
  // 開始時刻が分からないturnは窓で除外しない
  assert.equal(assignSends([tn(null, [ent("u", "hi")])], [sent("hi", at)]).has("u"), true);
});

test("本文なし（添付だけ）と切り詰められた本文も割り当てる", () => {
  assert.equal(assignSends([tn(1000, [ent("u1", "")])], [sent("", 1000)]).get("u1")?.length, 1);
  assert.equal(assignSends([tn(1000, [ent("u2", "abc")])], [sent("abcdef", 1000)]).has("u2"), true);
  assert.equal(assignSends([tn(1000, [ent("u3", "xyz")])], [sent("abcdef", 1000)]).has("u3"), false);
});

test("送信の記録が無ければ（拒否・未受理は記録しない）何も割り当てない", () => {
  assert.equal(assignSends([tn(1000, [ent("u1", "hi")])], []).size, 0);
});

test("追加指示は、実行中だった既存turnの発言に割り当てる（開始が窓の外でもよい）", () => {
  const at = 500_000;
  const turns = [tn(1000, [ent("old", "go")], 2000), tn(10_000, [ent("main", "start"), ent("steer", "go")])];
  const m = assignSends(turns, [sent("go", at, [att("s.png")], true)]);
  assert.equal(m.get("steer")?.[0].name, "s.png");
  assert.equal(m.has("old"), false); // 送信の前に終わっていたturnの同文は対象外
});

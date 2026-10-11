// 実行: node --test src/ui/fold.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { formatWorkDuration, workFold, workFoldLabel, createFoldMemory, isLongBody, messageTexts, needsExpand, splitFold } from "./fold.ts";
import { assignSends, chipKey, chipModel, classifyCarry, matchLocal, splitChips, UNDETERMINED_LABEL } from "./chips.ts";

const e = (k: string, text = "x") => ({ kind: { kind: k }, text: { kind: "value", value: text } });
const turn = (...ks: string[]) => ({ entries: ks.map((k) => e(k)) });
const pair = () => turn("userMessage", "agentMessage");

test("20件以内は折りたたまない", () => {
  assert.deepEqual(splitFold(Array.from({ length: 10 }, pair), 20), { hiddenTurns: 0, hiddenMessages: 0 });
  assert.deepEqual(splitFold([], 20), { hiddenTurns: 0, hiddenMessages: 0 });
});

test("直近20件を含む末尾のturn群を残し、境目はturnの間", () => {
  // 12turn=24件: 末尾10turn(20件)を表示、先頭2turn(4件)を折りたたむ
  assert.deepEqual(splitFold(Array.from({ length: 12 }, pair), 20), { hiddenTurns: 2, hiddenMessages: 4 });
});

test("作業の記録・要求は発言として数えない", () => {
  const t = turn("userMessage", "command", "command", "reasoning", "agentMessage");
  assert.deepEqual(splitFold(Array.from({ length: 11 }, () => t), 20), { hiddenTurns: 1, hiddenMessages: 2 });
});

test("折りたたまれる側に発言が無ければ折りたたまない", () => {
  const ts = [turn("command"), ...Array.from({ length: 10 }, pair)];
  assert.deepEqual(splitFold(ts, 20), { hiddenTurns: 0, hiddenMessages: 0 });
});

test("検索語が折りたたみ内にあるときだけ展開が要る（英字は大文字小文字無視、装飾をまたぐ一致も）", () => {
  assert.equal(needsExpand(["foo bar", "Hello World"], "world"), true);
  assert.equal(needsExpand(["foo bar"], "zzz"), false);
  assert.equal(needsExpand(["a**b**c"], "abc"), true);
  assert.equal(needsExpand(["anything"], ""), false);
});

test("折りたたみ内の発言本文だけを集める", () => {
  const ts = [{ entries: [e("userMessage", "q"), e("command", "ls"), { kind: { kind: "agentMessage" }, text: { kind: "missing" } }, e("agentMessage", "a")] }];
  assert.deepEqual(messageTexts(ts), ["q", "a"]);
});

test("長い依頼文の判定（8行超または600文字超）", () => {
  assert.equal(isLongBody("a\n".repeat(7) + "a"), false); // 8行
  assert.equal(isLongBody("a\n".repeat(8) + "a"), true); // 9行
  assert.equal(isLongBody("x".repeat(600)), false);
  assert.equal(isLongBody("x".repeat(601)), true);
});

test("チップ: 名前があれば通常、無ければ中立の「添付あり（詳細未確認）」", () => {
  const ok = chipModel({ name: "a.png", kind: "image", path: "C:/c/a.png", basis: "structured" });
  assert.equal(ok.label, "a.png");
  assert.equal(ok.neutral, false);
  const un = chipModel({ name: null, kind: null, path: null, basis: "undetermined" });
  assert.equal(un.label, UNDETERMINED_LABEL);
  assert.equal(un.neutral, true);
});

test("チップは最大3件＋残り、展開で全件", () => {
  assert.deepEqual(splitChips([1, 2, 3, 4, 5], false), { shown: [1, 2, 3], more: 2 });
  assert.deepEqual(splitChips([1, 2, 3], false), { shown: [1, 2, 3], more: 0 });
  assert.deepEqual(splitChips([1, 2, 3, 4, 5], true), { shown: [1, 2, 3, 4, 5], more: 0 });
});

test("履歴のパスとローカルの添付レコードを、区切り・大文字小文字・\\\\?\\接頭辞を無視して照合", () => {
  const locals = [{ copyPath: "C:\\Data\\Chat\\a.png", id: "1", source: { kind: "file", originalPath: "x" } }] as never;
  assert.equal(matchLocal("\\\\?\\c:/data/chat/A.png", locals)?.id, "1");
  assert.equal(matchLocal("C:/other/a.png", locals), null);
  assert.equal(matchLocal(null, locals), null);
});

const ent = (id: string, text: string, atts: unknown[] = []) => ({ key: { itemId: id }, kind: { kind: "userMessage" }, text: text === "" ? { kind: "missing" } : { kind: "value", value: text }, attachments: atts as never[] });
const tn = (start: number | null, ...es: ReturnType<typeof ent>[]) => ({ startedAt: start === null ? { kind: "missing" } : { kind: "value", value: start }, entries: es });
const sent = (text: string, at: number, name = "a.png") => ({ text, at, atts: [{ name, kind: "image", path: "C:/c/" + name, basis: "structured" }] as never[] });

test("送信直後: 履歴に添付が無い同じ本文の発言へ、送信時の添付を割り当てる", () => {
  const m = assignSends([tn(1000, ent("u1", "見て"))], [sent("見て", 1000)]);
  assert.equal(m.get("u1")?.[0].name, "a.png");
  // 添付だけ（本文なし）
  assert.equal(assignSends([tn(1000, ent("u2", ""))], [sent("", 1000)]).get("u2")?.length, 1);
  // live通知で本文が切り詰められても前方一致で同じ発言とみなす
  assert.equal(assignSends([tn(1000, ent("u3", "abc"))], [sent("abcdef", 1000)]).has("u3"), true);
});

test("送信直後: 履歴の添付がある発言は置き換えず、同じ送信を別の発言に当てない", () => {
  const real = [{ name: "a.png", kind: "image", path: "C:/c/a.png", basis: "structured" }];
  const turns = [tn(1000, ent("u1", "hi", real)), tn(2000, ent("u2", "hi"))];
  const m = assignSends(turns, [sent("hi", 1000)]);
  assert.equal(m.has("u1"), false); // 履歴を優先
  assert.equal(m.has("u2"), false); // 送信は u1 で消費済み。添付なしの別の発言に付けない
});

test("送信直後: 時間の幅の外は割り当てない", () => {
  assert.equal(assignSends([tn(1_000_000, ent("u1", "hi"))], [sent("hi", 1000)]).has("u1"), false);
});

test("チップのキーは送信直後と履歴復元で同じ（コピー先パスの正規化）", () => {
  const a = { name: "a.png", kind: "image", path: "C:\\c\\a.png", basis: "structured" } as never;
  const b = { name: "a.png", kind: "image", path: "c:/c/A.png", basis: "structured" } as never;
  assert.equal(chipKey(a, 0), chipKey(b, 0));
});

test("再送の添付: 保持済みで使えるものだけ付け、欠損・復元不能は理由つきで落とさず示す", () => {
  const loc = (id: string, copyPath: string, state: string) => ({ id, copyPath, state: { kind: state }, source: { kind: "file", originalPath: "x" } });
  const locals = [loc("l1", "C:/c/a.png", "ready"), loc("l2", "C:/c/b.txt", "missing")] as never;
  const atts = [
    { name: "a.png", kind: "image", path: "C:/c/a.png", basis: "structured" },
    { name: "b.txt", kind: "file", path: "C:/c/b.txt", basis: "listedText" },
    { name: "z.txt", kind: "file", path: "D:/other/z.txt", basis: "listedText" },
    { name: null, kind: null, path: null, basis: "undetermined" },
  ] as never;
  const p = classifyCarry(atts, locals);
  assert.deepEqual(p.carry.map((c) => c.id), ["l1"]);
  assert.equal(p.lost.length, 3);
  assert.match(p.lost[0].reason, /欠損/);
});

test("展開状態の記憶は、戻ったとき保たれ、上限を超えると古いチャットから忘れる", () => {
  const m = createFoldMemory(2);
  assert.deepEqual(m.get("a"), { expanded: false, openMsgs: [], openWork: [] });
  m.set("a", { expanded: true, openMsgs: ["i1"], openWork: [] });
  m.set("b", { expanded: true, openMsgs: [], openWork: [] });
  assert.equal(m.get("a").expanded, true); // 使うと新しい側へ
  m.set("c", { expanded: true, openMsgs: [], openWork: [] });
  assert.equal(m.size(), 2);
  assert.equal(m.get("b").expanded, false); // 最も古い b を忘れた
  assert.equal(m.get("a").expanded, true);
});

const we = (id: string, kind: string, messagePhase = "unknown", phase = "completed") => ({ key: { itemId: id }, kind: { kind }, phase, messagePhase });
const wt = (end: string | null, entries: ReturnType<typeof we>[], s: number | null = 1000, c: number | null = 8000) => ({
  end, entries,
  startedAt: s === null ? { kind: "missing" } : { kind: "value", value: s },
  completedAt: c === null ? { kind: "missing" } : { kind: "value", value: c },
});

test("途中経過の折りたたみ: 完了turnで最終回答があれば、commentaryと作業の記録だけ畳む", () => {
  const w = workFold(wt("completed", [we("u", "userMessage"), we("a", "agentMessage", "commentary"), we("c", "command"), we("b", "agentMessage", "commentary"), we("f", "agentMessage", "final"), we("x", "agentMessage", "unknown")]));
  assert.deepEqual(w, { ids: ["a", "c", "b"], count: 3, durationMs: 7000 });
  assert.equal(workFoldLabel(w!), "作業の経過（3件・7秒）");
});

test("途中経過の折りたたみ: 実行中・失敗・中断・最終回答なし・phase不明だけは畳まない", () => {
  const es = [we("a", "agentMessage", "commentary"), we("f", "agentMessage", "final")];
  assert.equal(workFold(wt(null, es)), null);
  assert.equal(workFold(wt("failed", es)), null);
  assert.equal(workFold(wt("interrupted", es)), null);
  assert.equal(workFold(wt("completed", [we("a", "agentMessage", "commentary"), we("c", "command")])), null);
  assert.equal(workFold(wt("completed", [we("a", "agentMessage", "unknown"), we("f", "agentMessage", "unknown")])), null);
  // 畳む対象が無い／未完了・終端未確認の項目がある
  assert.equal(workFold(wt("completed", [we("f", "agentMessage", "final")])), null);
  assert.equal(workFold(wt("completed", [...es, we("c", "command", "unknown", "unconfirmed")])), null);
  assert.equal(workFold(wt("completed", [...es, we("c", "command", "unknown", "inProgress")])), null);
});

test("途中経過の折りたたみ: 時刻が分からなければ件数のみ", () => {
  const es = [we("a", "agentMessage", "commentary"), we("f", "agentMessage", "final")];
  assert.equal(workFoldLabel(workFold(wt("completed", es, null, 5000))!), "作業の経過（1件）");
  assert.equal(workFoldLabel(workFold(wt("completed", es, 9000, 5000))!), "作業の経過（1件）");
});

test("所要時間の表記", () => {
  assert.equal(formatWorkDuration(200), "1秒未満");
  assert.equal(formatWorkDuration(42000), "42秒");
  assert.equal(formatWorkDuration(427000), "7分7秒");
  assert.equal(formatWorkDuration(420000), "7分");
  assert.equal(formatWorkDuration(3720000), "1時間2分");
});

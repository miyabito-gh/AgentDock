// 実行: node --test src/ui/changesView.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { requestOptions, defaultRequest, mergeRows, allState, toggleAll, setChecked, planChanged, keepChosen, headline, confirmSummary, statusSlot, resultText, clip } from "./changesView.ts";

const seg = (turn: string | null, state: any, concurrent = false, startedAt = new Date(2026, 9, 13, 10, 42).getTime()) => ({ turn, startedAt, state, concurrent }) as any;
const rec = (turnId: string, text: string) => ({ key: { turnId }, entries: [{ kind: { kind: "agentMessage" }, text: { kind: "value", value: "x" } }, { kind: { kind: "userMessage" }, text: { kind: "value", value: text } }] }) as any;
const val = (v: number) => ({ kind: "value", value: v, basis: "reported" }) as any;
const file = (path: string, over: any = {}) => ({ path, kind: "modified", moveTo: null, source: "baseline", additions: val(1), deletions: val(2), turn: null, reverted: false, binary: false, ...over }) as any;
const rev = { kind: "revertible", summary: "" };
const need = (...codes: string[]) => ({ kind: "needsOverride", summary: "", reasons: codes.map((code) => ({ code, message: code })) });
const blk = (code = "headMoved") => ({ kind: "blocked", code, message: "m" });
const item = (path: string, verdict: any, over: any = {}) => ({ path, kind: "modified", includesTurns: [], verdict, ...over });
const plan = (...items: any[]) => ({ id: "p", chat: {}, items, createdAt: 0, expiresAt: 10 }) as any;

test("依頼の選択肢: 新しい順・発言の先頭・印・記録なしは無効", () => {
  const segs = [seg("t1", { kind: "ready" }), seg("t2", { kind: "failed", reason: { kind: "timeout" } }), seg("t3", { kind: "endUnknown" }, true), seg(null, { kind: "ready" }), seg("t4", { kind: "running" })];
  const o = requestOptions(segs, [rec("t1", "ログイン画面の\nエラー表示を直して"), rec("t3", "あ".repeat(40))]);
  assert.deepEqual(o.map((x) => x.turn), ["t4", "t3", "t2", "t1"]);
  assert.equal(o[3].label, "#1「ログイン画面の エラー表示を直して」 10:42");
  assert.equal(o[0].label, "#4 10:42 ●作業中"); // 発言が手元にない
  assert.ok(o[1].label.includes("…") && o[1].label.includes("⇆"));
  assert.equal(o[2].disabled, true);
  assert.ok(o[2].label.endsWith("記録なし"));
  assert.equal(defaultRequest(o), "t4");
  assert.equal(defaultRequest(requestOptions([seg("t", { kind: "abandoned" })], [])), null);
  assert.equal(clip("a".repeat(31)).length, 31);
});

test("行の状態: 戻せる・要確認・戻せない・戻し済み・未確認・失敗", () => {
  const files = [file("a"), file("b"), file("c"), file("d"), file("e", { reverted: true }), file("f"), file("g")];
  const p = plan(item("a", rev), item("b", need("changedAfter", "endUnknown")), item("c", blk()), item("d", blk("alreadyReverted")), item("g", rev));
  const rows = mergeRows(files, p, { a: true, b: true }, { reverted: [], forced: [], failed: [{ path: "g", reason: "r" }], backupDir: null });
  assert.deepEqual(rows.map((r) => r.state), ["revertible", "needsOverride", "blocked", "reverted", "reverted", "unknown", "failed"]);
  assert.deepEqual(rows.map((r) => r.selectable), [true, false, false, false, false, false, false]);
  assert.deepEqual(rows.map((r) => r.checked), [true, true, false, false, false, false, false]);
  assert.deepEqual(rows[1].reasons.map((x) => x.code), ["endUnknown", "changedAfter"]);
  assert.equal(rows[6].reasons[0].message, "r");
  assert.deepEqual(mergeRows(files.slice(0, 1), null, {}, null).map((r) => r.state), ["unknown"]); // 計画を取れない
});

test("すべて・個別: 戻せる行だけ。要確認は一覧では選べず、右ペインで入れる", () => {
  const files = [file("a"), file("b"), file("c")];
  const p = plan(item("a", rev), item("b", need("changedAfter")), item("c", blk()));
  const rows = (c: any) => mergeRows(files, p, c, null);
  assert.equal(allState(rows({})), "none");
  const on = toggleAll(rows({}), {});
  assert.deepEqual(on, { a: true }); // 要確認は含めない
  assert.equal(allState(rows(on)), "all");
  assert.deepEqual(toggleAll(rows(on), { ...on, b: true }), { b: true }); // 解除しても要確認の選択はそのまま
  assert.deepEqual(setChecked(rows({}), {}, "b", true), { b: true });
  assert.deepEqual(setChecked(rows({}), {}, "c", true), {}); // 戻せないは入らない
  assert.equal(allState(mergeRows([file("c")], plan(item("c", blk())), {}, null)), "empty");
});

test("planChanged: 選んだファイルの判定が変わった・消えたものだけ", () => {
  const a = plan(item("a", rev), item("b", need("changedAfter")), item("c", rev));
  const b = plan(item("a", rev), item("b", need("changedAfter", "endUnknown")), item("c", blk()));
  assert.deepEqual(planChanged(a, b, ["a", "b", "c"]), ["b", "c"]);
  assert.deepEqual(planChanged(a, b, ["a"]), []);
  assert.deepEqual(planChanged(a, plan(item("a", rev)), ["a", "c"]), ["c"]); // 消えた
  assert.deepEqual(keepChosen({ a: true, b: true, c: true }, a, b), { a: true });
  assert.deepEqual(keepChosen({ a: true }, a, null), {});
});

test("見出しの集計: 未取得は0にせず null、バイナリは行数に含めない", () => {
  assert.deepEqual(headline([file("a"), file("b", { additions: val(5), deletions: val(0) })]), { count: 2, additions: 6, deletions: 2 });
  const unk = { kind: "notFetched" } as any;
  assert.deepEqual(headline([file("a"), file("b", { additions: unk })]), { count: 2, additions: null, deletions: 4 });
  assert.deepEqual(headline([file("a"), file("b", { binary: true, additions: { kind: "unsupported" }, deletions: { kind: "unsupported" } })]), { count: 2, additions: 1, deletions: 2 });
  assert.deepEqual(headline([]), { count: 0, additions: 0, deletions: 0 });
});

test("確認の要約: 通常と要確認を分け、要確認があればボタン名が変わる", () => {
  const items = [item("a", rev, { includesTurns: ["x", "y"] }), item("n", rev, { kind: "added" }), item("b", need("changedAfter")), item("c", blk())] as any;
  const s = confirmSummary(items, { a: true, n: true, b: true, c: true });
  assert.deepEqual(s.paths, ["a", "n", "b"]);
  assert.deepEqual(s.forced, ["b"]);
  assert.equal(s.total, 3);
  assert.deepEqual(s.normal, [{ path: "a", includes: 2, deletes: false }, { path: "n", includes: 0, deletes: true }]);
  assert.equal(s.override[0].reasons[0].code, "changedAfter");
  assert.equal(s.actionLabel, "別の変更も含めて 3 件を戻す");
  assert.equal(confirmSummary(items, { a: true }).actionLabel, "1 件を戻す");
  assert.equal(confirmSummary(items, {}).total, 0);
});

test("状態スロット: 優先順で1つだけ", () => {
  const base = { listError: null, changedPaths: [] as string[], baseText: null, ownerPanel: false, planError: null, result: null, resultError: null, notes: [] as string[] };
  assert.equal(statusSlot(base), null);
  const r = { reverted: ["a"], forced: [], failed: [], backupDir: "d" };
  assert.equal(statusSlot({ ...base, notes: ["n1", "n2"] })?.kind, "notes");
  assert.equal((statusSlot({ ...base, notes: ["n1", "n2"] }) as any).text, "n1・n2");
  assert.equal(statusSlot({ ...base, notes: ["n"], result: r })?.kind, "result");
  assert.equal(statusSlot({ ...base, result: r, planError: "p" })?.kind, "planError");
  assert.equal(statusSlot({ ...base, planError: "p", ownerPanel: true })?.kind, "owner");
  assert.equal(statusSlot({ ...base, ownerPanel: true, baseText: "b" })?.kind, "noBaseline");
  assert.equal(statusSlot({ ...base, baseText: "b", changedPaths: ["a"] })?.kind, "changed");
  assert.equal(statusSlot({ ...base, changedPaths: ["a"], listError: "e" })?.kind, "listError");
  assert.equal(statusSlot({ ...base, result: r, resultError: "x" })?.kind, "resultError");
  assert.equal(statusSlot({ ...base, result: { ...r, failed: [{ path: "z", reason: "r" }] } })?.tone, "err");
  assert.equal(statusSlot({ ...base, result: r })?.tone, "info");
});

test("結果の文: 一部のみは完了と書かない", () => {
  assert.equal(resultText({ reverted: ["a", "b"], forced: [], failed: [], backupDir: null }), "2 件を戻しました");
  assert.equal(resultText({ reverted: ["a"], forced: [], failed: [{ path: "z", reason: "r" }], backupDir: null }), "一部のみ戻しました（戻せた 1・戻せなかった 1）");
  assert.equal(resultText({ reverted: [], forced: [], failed: [], backupDir: null }), "書き換えたファイルはありません");
});

test("差分の行分け: 行番号・記号・ハンク前はmeta", async () => {
  const { diffRows, splitPath } = await import("./changesView.ts");
  const text = ["--- a/x", "+++ b/x", "@@ -10,3 +10,4 @@", " a", "-b", "+c", "+d", " e", "\\ No newline at end of file"].join("\n");
  const r = diffRows(text);
  assert.deepEqual(r.map((x) => x.kind), ["meta", "meta", "hunk", "ctx", "del", "add", "add", "ctx", "meta"]);
  assert.deepEqual(r.slice(3, 8).map((x) => [x.old, x.new, x.text]), [[10, 10, "a"], [11, null, "b"], [null, 11, "c"], [null, 12, "d"], [12, 13, "e"]]);
  assert.deepEqual(splitPath("src\\ui/a.ts"), { name: "a.ts", dir: "src\\ui" });
  assert.deepEqual(splitPath("a.ts"), { name: "a.ts", dir: "" });
});

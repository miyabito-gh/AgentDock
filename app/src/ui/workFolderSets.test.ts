// 実行: node --test src/ui/workFolderSets.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { inspectFolders, MAX_SETS, moveSet, removeSet, renameSet, replaceItemAt, sameFolderInit, saveSet } from "./workFolderSets.ts";
import { MAX_FOLDERS, saveBlockReason } from "./workFolders.ts";

const o = (overwrite = false) => ({ overwrite, now: 100, newId: (() => { let n = 0; return () => `id${++n}`; })() });
const v = (o: any = {}): any => ({ current: { kind: "notFetched" }, lastAccepted: null, unconfirmed: null, next: null, projectMatch: null, support: "supported", ...o });
const value = (folders: string[]): any => ({ kind: "value", value: { folders }, basis: "direct" });

test("保存: 新規は末尾に追加、名前と並びの不正は保存しない", () => {
  const r = saveSet([], "  開発 ", ["C:/a", "C:/b"], o());
  assert.equal(r.kind, "ok");
  if (r.kind === "ok") { assert.equal(r.saved.name, "開発"); assert.deepEqual(r.saved.folders, ["C:/a", "C:/b"]); assert.equal(r.sets.length, 1); }
  assert.equal(saveSet([], " ", ["C:/a"], o()).kind, "error");
  assert.equal(saveSet([], "x".repeat(61), ["C:/a"], o()).kind, "error");
  assert.equal(saveSet([], "a", [], o()).kind, "error");
  assert.equal(saveSet([], "a", Array.from({ length: MAX_FOLDERS + 1 }, (_, i) => `C:/f${i}`), o()).kind, "error");
  assert.equal(saveSet([], "a", ["C:/a", "c:\\A\\"], o()).kind, "error"); // 重複
});

test("保存: 同名は確認なしに上書きしない。上書きでは位置とIDを保つ（大文字小文字は同じ名前）", () => {
  const a = saveSet([], "Dev", ["C:/a"], o());
  const b = a.kind === "ok" ? saveSet(a.sets, "Other", ["C:/b"], o()) : a;
  assert.equal(b.kind, "ok");
  const sets = b.kind === "ok" ? b.sets : [];
  const again = saveSet(sets, "dev", ["C:/z"], o(false));
  assert.equal(again.kind, "needsOverwrite");
  const ow = saveSet(sets, "dev", ["C:/z", "C:/y"], o(true));
  assert.equal(ow.kind, "ok");
  if (ow.kind === "ok") {
    assert.equal(ow.sets.length, 2);
    assert.equal(ow.sets[0].id, sets[0].id);
    assert.deepEqual(ow.sets[0].folders, ["C:/z", "C:/y"]);
    assert.equal(ow.sets[0].updatedAt, 100);
    assert.equal(ow.sets[0].name, "dev");
  }
});

test("保存: 上限件数に達したら新規は保存しない（上書きは可）", () => {
  let sets: any[] = Array.from({ length: MAX_SETS }, (_, i) => ({ id: `i${i}`, name: `n${i}`, folders: ["C:/a"], updatedAt: 1 }));
  assert.equal(saveSet(sets, "new", ["C:/a"], o()).kind, "error");
  assert.equal(saveSet(sets, "n3", ["C:/b"], o(true)).kind, "ok");
  sets = sets.slice(1);
  assert.equal(saveSet(sets, "new", ["C:/a"], o()).kind, "ok");
});

test("名前変更・並べ替え・削除", () => {
  const sets: any[] = [{ id: "1", name: "A", folders: ["C:/a"], updatedAt: 1 }, { id: "2", name: "B", folders: ["C:/b"], updatedAt: 1 }];
  assert.equal(renameSet(sets, "2", "a", 5).kind, "error"); // 他と同名
  assert.equal(renameSet(sets, "2", "", 5).kind, "error");
  assert.equal(renameSet(sets, "9", "z", 5).kind, "error");
  const r = renameSet(sets, "1", "a", 5); // 自分自身は大文字小文字の変更が可
  assert.equal(r.kind, "ok");
  if (r.kind === "ok") assert.equal(r.sets[0].name, "a");
  assert.deepEqual(moveSet(sets, "2", -1).map((s: any) => s.id), ["2", "1"]);
  assert.deepEqual(moveSet(sets, "1", -1).map((s: any) => s.id), ["1", "2"]);
  assert.deepEqual(moveSet(sets, "nope", 1).map((s: any) => s.id), ["1", "2"]);
  assert.deepEqual(removeSet(sets, "1").map((s: any) => s.id), ["2"]);
});

test("復元: 各件を検査し、使えないものは理由つきで残す（外さない・保存不可）", async () => {
  const calls: string[][] = [];
  const check = async (p: string, cur: string[]): Promise<any> => {
    calls.push(cur);
    if (p === "C:/gone") return { kind: "rejected", reason: "notFound" };
    if (p === "C:/boom") throw new Error("x");
    if (p === "C:/home") return { kind: "needsStrongConfirm", normalized: "C:\\home", reasons: ["homeRoot"] };
    return { kind: "ok", normalized: p.replace(/\//g, "\\") };
  };
  const items = await inspectFolders(["C:/a", "C:/gone", "C:/home", "C:/boom"], check);
  assert.equal(items.length, 4);
  assert.deepEqual(items[0], { path: "C:\\a", strong: [], confirmed: false });
  assert.equal(items[1].path, "C:/gone");
  assert.equal(items[1].rejected, "フォルダが見つかりません");
  assert.deepEqual(items[2].strong, ["homeRoot"]);
  assert.equal(items[2].confirmed, false);
  assert.equal(items[3].rejected, "検査できませんでした");
  assert.deepEqual(calls[0], ["C:/gone", "C:/home", "C:/boom"]); // 自分以外の全件
  assert.match(saveBlockReason(items, "supported") ?? "", /使えないフォルダ/);
  // 外すと、残りの強い確認は未確認のまま止まる。
  assert.match(saveBlockReason([items[0], items[2]], "supported") ?? "", /確認が必要/);
});

test("置き換え: 拒否は置き換えず理由を返す。通れば強い確認つきで差し替える", async () => {
  const items = await inspectFolders(["C:/gone"], async () => ({ kind: "rejected", reason: "notFound" }) as any);
  const bad = replaceItemAt(items, 0, { kind: "rejected", reason: "duplicate" } as any);
  assert.equal(bad.items, items);
  assert.ok(bad.error);
  const ok = replaceItemAt(items, 0, { kind: "needsStrongConfirm", normalized: "C:\\x", reasons: ["driveRoot"] } as any);
  assert.equal(ok.error, null);
  assert.deepEqual(ok.items[0], { path: "C:\\x", strong: ["driveRoot"], confirmed: false });
  assert.equal(saveBlockReason(ok.items, "supported") !== null, true);
});

test("同じ作業フォルダ: 次の送信用 → 直接取得 → 送信記録の順。取れなければ cwd のみ・未確認", () => {
  const acc = { folders: { folders: ["C:/r", "C:/s"] }, acceptedAt: 1 };
  assert.deepEqual(sameFolderInit(v({ next: { folders: ["C:/n", "C:/m"] }, current: value(["C:/c", "C:/d"]), lastAccepted: acc }), "C:/cwd"), { folders: ["C:/n", "C:/m"], extrasUnconfirmed: false });
  assert.deepEqual(sameFolderInit(v({ current: value(["C:/c", "C:/d"]), lastAccepted: acc }), "C:/cwd"), { folders: ["C:/c", "C:/d"], extrasUnconfirmed: false });
  assert.deepEqual(sameFolderInit(v({ lastAccepted: acc }), "C:/cwd"), { folders: ["C:/r", "C:/s"], extrasUnconfirmed: false });
  assert.deepEqual(sameFolderInit(v(), " C:/cwd "), { folders: ["C:/cwd"], extrasUnconfirmed: true });
  assert.deepEqual(sameFolderInit(null, "C:/cwd"), { folders: ["C:/cwd"], extrasUnconfirmed: true });
  assert.equal(sameFolderInit(null, null), null);
  assert.equal(sameFolderInit(v(), "  "), null);
  // 上限を超える並びは先頭から8件（ホストが保存する並びは8件までのため）。
  const many = Array.from({ length: 10 }, (_, i) => `C:/f${i}`);
  assert.equal(sameFolderInit(v({ next: { folders: many } }), null)?.folders.length, MAX_FOLDERS);
});

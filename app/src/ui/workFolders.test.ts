// 実行: node --test src/ui/workFolders.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  addBlockReason, applyCheck, confirmedStrongPaths, droppedExtrasNote, folderKey, makePrimary, MAX_FOLDERS, moveItem, needsAddConfirm,
  projectDefaults, projectMatchText, saveBlockReason, toItems, viewRows,
} from "./workFolders.ts";

const v = (o: any = {}): any => ({ current: { kind: "notFetched" }, lastAccepted: null, unconfirmed: null, next: null, projectMatch: null, support: "supported", ...o });
const value = (folders: string[]): any => ({ kind: "value", value: { folders }, basis: "direct" });

test("並べ替え・プライマリ化は範囲外を動かさない", () => {
  assert.deepEqual(moveItem([1, 2, 3], 1, -1), [2, 1, 3]);
  assert.deepEqual(moveItem([1, 2, 3], 0, -1), [1, 2, 3]);
  assert.deepEqual(moveItem([1, 2, 3], 2, 1), [1, 2, 3]);
  assert.deepEqual(makePrimary(["a", "b", "c"], 2), ["c", "a", "b"]);
  assert.deepEqual(makePrimary(["a", "b"], 0), ["a", "b"]);
});

test("検査結果: 拒否は足さず理由を返し、強い確認は未確認のまま足す", () => {
  const r = applyCheck([], { kind: "rejected", reason: "unc" });
  assert.equal(r.items.length, 0);
  assert.match(r.error!, /UNC/);
  const s = applyCheck(toItems(["C:\\a"]), { kind: "needsStrongConfirm", normalized: "D:\\", reasons: ["driveRoot"] });
  assert.equal(s.error, null);
  assert.deepEqual(s.items[1], { path: "D:\\", strong: ["driveRoot"], confirmed: false });
  assert.deepEqual(confirmedStrongPaths(s.items), []);
  const ok = s.items.map((i) => ({ ...i, confirmed: true }));
  assert.deepEqual(confirmedStrongPaths(ok), ["D:\\"]);
  // 上限
  const full = toItems(Array.from({ length: MAX_FOLDERS }, (_, i) => `C:\\p${i}`));
  assert.match(applyCheck(full, { kind: "ok", normalized: "C:\\x" }).error!, /8件/);
  // プライマリが一覧の外にあるとき（新しいチャットの追加分）は、その分を数える。
  const seven = toItems(Array.from({ length: MAX_FOLDERS - 1 }, (_, i) => `C:\\p${i}`));
  assert.match(applyCheck(seven, { kind: "ok", normalized: "C:\\x" }, 1).error!, /8件/);
  assert.equal(applyCheck(seven, { kind: "ok", normalized: "C:\\x" }).error, null);
  assert.match(saveBlockReason(toItems(Array.from({ length: MAX_FOLDERS + 1 }, (_, i) => `C:\\q${i}`)), "supported")!, /8件/);
});

test("保存できない理由: 強い確認の未確認・非対応での複数・空", () => {
  assert.equal(saveBlockReason([], "supported"), "作業フォルダ（プライマリ）が必要です");
  assert.equal(saveBlockReason(toItems(["C:\\a"]), "unsupported"), null, "プライマリだけなら非対応でも保存できる");
  assert.match(saveBlockReason(toItems(["C:\\a", "C:\\b"]), "experimental")!, /experimental/);
  const items = [...toItems(["C:\\a"]), { path: "D:\\", strong: ["driveRoot" as const], confirmed: false }];
  assert.match(saveBlockReason(items, "supported")!, /D:\\/);
  items[1].confirmed = true;
  assert.equal(saveBlockReason(items, "supported"), null);
  assert.equal(addBlockReason("supported"), null);
  assert.ok(addBlockReason("unknown"));
});

test("追加・プライマリ変更だけ確認が要る（削除・並べ替えは不要）", () => {
  assert.equal(needsAddConfirm(["C:\\a", "C:\\b"], ["C:\\a"]), false);
  assert.equal(needsAddConfirm(["C:\\a", "C:\\b"], ["c:/B", "C:\\a"]), true, "プライマリが変わる");
  assert.equal(needsAddConfirm(["C:\\a"], ["C:\\a", "C:\\c"]), true);
  assert.equal(needsAddConfirm(["C:\\a", "C:\\b"], ["C:\\A", "C:\\B\\"]), false);
  assert.equal(folderKey("\\\\?\\C:\\Work\\"), "c:\\work");
});

test("確認表示: 送信記録は直接取得と区別し、未読込みは未確認のまま", () => {
  const rows = viewRows(v({ lastAccepted: { folders: { folders: ["C:\\a", "C:\\b"] }, acceptedAt: 0 } }), null);
  assert.equal(rows[0].how, "未確認（このチャットは読み込まれていません）");
  assert.equal(rows[0].folders, null, "未読込みを空の一覧にしない");
  assert.equal(rows[1].key, "accepted");
  assert.match(rows[1].note!, /断定できません/);
  // 直接取得と一致したときだけ「反映を確認」。
  const same = viewRows(v({ current: value(["c:/a", "C:\\b"]), lastAccepted: { folders: { folders: ["C:\\a", "C:\\b"] }, acceptedAt: 0 } }), 1);
  assert.match(same[0].how, /^直接取得/);
  assert.match(same[1].note!, /反映を確認/);
  const unk = viewRows(v({ unconfirmed: { folders: ["C:\\z"] }, next: { folders: ["C:\\n"] }, current: { kind: "unsupported" } }), null);
  assert.deepEqual(unk.map((r) => r.key), ["current", "unconfirmed", "next"]);
  assert.match(unk[0].how, /^非対応/);
  assert.match(unk[1].how, /^未確認/);
});

test("プロジェクト既定: 先頭がプライマリ、重複を除き上限8件で超過数を返す", () => {
  const ten = Array.from({ length: 10 }, (_, i) => `C:\\p${i}`);
  const r = projectDefaults(["C:\\p0", "c:/P0", ...ten.slice(1)]);
  assert.equal(r.primary, "C:\\p0");
  assert.equal(r.extras.length, 7);
  assert.equal(r.omitted, 2);
  assert.deepEqual(projectDefaults([]), { primary: "", extras: [], omitted: 0 });
});

test("推定の印と非対応での追加分の案内", () => {
  assert.equal(projectMatchText({ projectName: "ア", matched: 2, total: 3, missing: ["C:\\d"], inferred: true }), "プロジェクト「ア」(推定): 一致 2/3（不足: C:\\d）");
  assert.ok(droppedExtrasNote({ folders: ["C:\\a", "C:\\b"] }, "experimental"));
  assert.equal(droppedExtrasNote({ folders: ["C:\\a", "C:\\b"] }, "supported"), null);
  assert.equal(droppedExtrasNote({ folders: ["C:\\a"] }, "unsupported"), null);
  assert.equal(droppedExtrasNote(null, "unsupported"), null);
});

import { multiFolderMark } from "./workFolders.ts";
test("複数フォルダ印: 2件以上の既知の指定だけ・取れないものは出さない・根拠を添える", () => {
  assert.equal(multiFolderMark(null), null);
  assert.equal(multiFolderMark({ acceptedFolders: null, nextFolders: null }), null);
  assert.equal(multiFolderMark({ acceptedFolders: { folders: { folders: ["D:xa"] } }, nextFolders: { folders: ["D:xb"] } }), null);
  const m = multiFolderMark({ acceptedFolders: { folders: { folders: ["D:xa", "D:xb"] } }, nextFolders: null });
  assert.equal(m?.text, "複数フォルダ");
  assert.match(m!.title, /送信記録/);
  assert.match(m!.title, /主: D:xa\n追加: D:xb/);
  const n = multiFolderMark({ nextFolders: { folders: ["D:xa", "D:xb", "D:xc"] } });
  assert.match(n!.title, /未送信/);
  assert.doesNotMatch(n!.title, /送信記録/);
});

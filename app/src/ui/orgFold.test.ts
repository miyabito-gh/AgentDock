// 実行: node --test src/ui/orgFold.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { splitHistory, foldCounts, foldLabel, archiveEligible, pickArchiveTargets, summarizeArchive, clipList } from "./orgFold.ts";

const w = (id: string, o: { fin?: boolean; del?: boolean; ready?: number; bad?: boolean; arch?: number | null } = {}) => ({
  workflowId: id, finished: o.fin ?? true, archivedAt: o.arch ?? null, deletion: o.del ? {} : null,
  artifactHint: o.ready === undefined ? null : { ready: o.ready },
  tasks: [{ confirm: { state: o.bad ? "sendUnknown" : "confirmed" }, status: { phase: "completed" }, needsChatChoice: false }],
});
const ids = (xs: Array<{ workflowId: string }>) => xs.map((x) => x.workflowId);

test("畳み: 直近1件・開いている・未終了・削除途中は見せ、他を畳む", () => {
  const h = [w("a"), w("b"), w("c", { fin: false }), w("d", { del: true }), w("e")];
  const r = splitHistory(h, "e", false, false);
  assert.deepEqual(ids(r.visible), ["d", "a", "c", "e"]);
  assert.deepEqual(ids(r.folded), ["b"]);
});
test("現在が終了済みなら直近枠は使わず、検索中は畳まない", () => {
  const h = [w("a"), w("b")];
  assert.deepEqual(ids(splitHistory(h, null, true, false).folded), ["a", "b"]);
  assert.equal(splitHistory(h, null, true, true).folded.length, 0);
});
test("件数と見出し: 取り込み待ち・要対応は文字で添える", () => {
  const f = [w("a", { ready: 2 }), w("b", { bad: true }), w("c")];
  const c = foldCounts(f);
  assert.deepEqual(c, { total: 3, pendingImport: 1, needAction: 1 });
  assert.equal(foldLabel(c), "過去のワークフロー（3件・うち取り込み待ち 1・要対応 1）");
  assert.equal(foldLabel(foldCounts([w("a")])), "過去のワークフロー（1件）");
});
test("まとめてアーカイブ: 確定済みのみ候補、取り込み待ちは既定で外す", () => {
  const f = [w("a"), w("b", { ready: 1 }), w("c", { bad: true }), w("d", { del: true }), w("e", { fin: false }), w("f", { arch: 1 })];
  const el = archiveEligible(f);
  assert.deepEqual(ids(el), ["a", "b"]);
  assert.deepEqual(ids(pickArchiveTargets(el, false)), ["a"]);
  assert.deepEqual(ids(pickArchiveTargets(el, true)), ["a", "b"]);
  assert.deepEqual(clipList([1, 2, 3, 4, 5, 6, 7]), { head: [1, 2, 3, 4, 5], more: 2 });
});
test("結果の集計", () => {
  assert.equal(summarizeArchive([{ id: "a", ok: true }]), "1件をアーカイブしました（アーカイブ一覧から戻せます）。");
  assert.equal(summarizeArchive([{ id: "a", ok: true }, { id: "b", ok: false, reason: "実行中" }, { id: "c", ok: false, reason: "実行中" }]), "1件をアーカイブしました／2件はできませんでした: 実行中");
});

// 実行: node --test src/ui/intakeDelete.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { intakeChoice, intakeDeleteTargets, intakeOutcomeText, previewLines, reasonOf, resultText, stateCounts, INTAKE_UNLINKED_NOTE } from "./intakeDelete.ts";

const T = new Date(2026, 9, 24, 13, 5).getTime();
const chat = (id: string, o: { pinned?: boolean; archived?: boolean } = {}) =>
  ({ key: { backend: "codex", id }, name: { kind: "unknown" }, preview: { kind: "unknown" }, createdAt: { kind: "unknown" }, lastUsedAt: T, pinned: !!o.pinned, archived: { kind: "value", value: !!o.archived } }) as never;
const local = (intake: unknown, o: Record<string, unknown> = {}) => ({ conversation: { intake }, visibility: { kind: "visible" }, ...o }) as never;
const run = (specs: Array<[string, unknown, Record<string, unknown>?, { pinned?: boolean; archived?: boolean }?]>, wfs: Array<[string, boolean]> = []) => {
  const chats = specs.map(([id, , , c]) => chat(id, c));
  const locals = new Map(specs.map(([id, it, o]) => [id, local(it, o)]));
  return intakeDeleteTargets(chats, (c) => locals.get((c as { key: { id: string } }).key.id), new Map(wfs.map(([id, finished]) => [id, { finished }])));
};

test("対象は開始済み・やめたもの。未開始は対象外", () => {
  const r = run([["a", { kind: "started", workflowId: "w1" }], ["b", { kind: "abandoned" }], ["c", { kind: "drafting" }], ["d", { kind: "summarizing" }], ["e", { kind: "ready" }]], [["w1", true]]);
  assert.deepEqual(r.map((x) => x.chat.id), ["a", "b"]);
  assert.deepEqual(stateCounts(r), { started: 1, abandoned: 1 });
});

test("ピン留め・実行中の紐づき・アーカイブ・引き継ぎ済み・削除保留は対象外", () => {
  const r = run([
    ["pin", { kind: "abandoned" }, {}, { pinned: true }],
    ["run", { kind: "started", workflowId: "w1" }],
    ["arc", { kind: "abandoned" }, {}, { archived: true }],
    ["sup", { kind: "abandoned" }, { supersededBy: { id: "x" } }],
    ["pend", { kind: "abandoned" }, { deletePending: { reason: {} } }],
    ["org", { kind: "abandoned" }, { org: { released: false } }],
    ["ok", { kind: "abandoned" }],
  ], [["w1", false]]);
  assert.deepEqual(r.map((x) => x.chat.id), ["ok"]);
});

test("紐づき先のワークフローが画面に無いもの（削除済み等）は候補に入れる。ホストが最終判定", () => {
  assert.deepEqual(run([["a", { kind: "started", workflowId: "gone" }]]).map((x) => x.chat.id), ["a"]);
});

test("確認の一覧は先頭5件＋ほかN件、名前・状態・日時", () => {
  const rows = run(Array.from({ length: 7 }, (_, i) => [`c${i}`, { kind: "abandoned" }] as [string, unknown]));
  const lines = previewLines(rows);
  assert.equal(lines.length, 6);
  assert.equal(lines[0], "依頼整理（10/24 13:05）（やめたもの・10/24 13:05）");
  assert.equal(lines[5], "ほか 2 件");
  assert.equal(previewLines(rows.slice(0, 5)).length, 5);
});

test("結果の集計文", () => {
  assert.equal(resultText(3, []), "3件を削除しました");
  assert.equal(resultText(2, [{ title: "A", reason: "停止を確認できていません" }, { title: "B", reason: "保存に失敗" }]), "2件を削除しました／2件はできませんでした: A（停止を確認できていません）、B（保存に失敗）");
  assert.equal(resultText(0, [{ title: "A", reason: "x" }]), "0件を削除しました／1件はできませんでした: A（x）");
  assert.equal(reasonOf("一行目\n二行目"), "一行目");
  assert.equal(reasonOf(""), "理由を取得できませんでした");
});

test("ワークフロー削除の依頼整理チェック: 紐づけなしは対象外の文、あれば既定オン、削除済みは出さない", () => {
  assert.deepEqual(intakeChoice({ intake: null, intakeUnlinked: true }), { kind: "none", note: INTAKE_UNLINKED_NOTE });
  assert.equal(INTAKE_UNLINKED_NOTE, "依頼整理の会話は、紐づけを確認できないため対象外です。");
  assert.deepEqual(intakeChoice({ intake: { gone: false, blocked: null }, intakeUnlinked: false }), { kind: "offer", defaultOn: true, blocked: null });
  assert.deepEqual(intakeChoice({ intake: { gone: false, blocked: "x" }, intakeUnlinked: false }), { kind: "offer", defaultOn: true, blocked: "x" });
  assert.equal(intakeChoice({ intake: { gone: true, blocked: null }, intakeUnlinked: false }).kind, "gone");
});

test("ワークフロー削除の結果の一言", () => {
  assert.equal(intakeOutcomeText(null), null);
  assert.equal(intakeOutcomeText({ deleted: true, reason: null }), "依頼整理の会話も削除しました。");
  assert.equal(intakeOutcomeText({ deleted: false, reason: "停止を確認できていません\n詳細" }), "依頼整理の会話は削除しませんでした: 停止を確認できていません");
});

// 実行: node --test src/ui/chatDeleteUi.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { archivedToast, DELETE_HEADLINE, DELETE_IRREVERSIBLE, deleteLocked, deleteRunningLabel, deleteRunningNotice } from "./chatDeleteUi.ts";

test("1行目と、元に戻せない旨・アーカイブの代替を書く", () => {
  assert.equal(DELETE_HEADLINE, "このチャットを削除します");
  assert.ok(DELETE_IRREVERSIBLE.includes("元に戻せません") && DELETE_IRREVERSIBLE.includes("アーカイブ") && DELETE_IRREVERSIBLE.includes("後から戻せます"));
});

test("実行中は経過秒を出し、90秒までは閉じられず、超えたら閉じてよいと案内する", () => {
  assert.equal(deleteRunningLabel(5_000), "削除しています…（5秒）");
  assert.equal(deleteRunningLabel(75_000), "削除しています…（1分15秒）");
  assert.equal(deleteLocked(true, 10_000), true);
  assert.equal(deleteLocked(true, 90_000), false);
  assert.equal(deleteLocked(false, 10_000), false);
  assert.ok(deleteRunningNotice(10_000).includes("閉じられません"));
  assert.ok(deleteRunningNotice(91_000).includes("時間がかかっています") && deleteRunningNotice(91_000).includes("閉じても処理は続きます"));
});

test("アーカイブのトーストは戻し方を書く", () => {
  assert.ok(archivedToast(false).includes("一覧の下の「アーカイブ」"));
  assert.ok(archivedToast(true).includes("作業は続きます") && archivedToast(true).includes("解除"));
});

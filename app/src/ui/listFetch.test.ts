// 実行: node --test src/ui/listFetch.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { fetchAllPages, countFetched, projectFetchMessage, loadMoreMessage, projectUpdateMessage } from "./listFetch.ts";

const pager = (pages: string[][]) => async (cursor: string | null) => {
  const i = cursor === null ? 0 : Number(cursor);
  return { items: pages[i], nextCursor: i + 1 < pages.length ? String(i + 1) : null };
};

test("nextCursor を最後まで辿る", async () => {
  const r = await fetchAllPages(pager([["a", "b"], ["c"], ["d"]]));
  assert.deepEqual(r.items, ["a", "b", "c", "d"]);
  assert.equal(r.pages, 3);
  assert.equal(r.truncated, false);
  assert.equal(r.error, null);
});

test("上限ページで止まり、続きありを示す", async () => {
  const r = await fetchAllPages(pager([["a"], ["b"], ["c"]]), 2);
  assert.deepEqual(r.items, ["a", "b"]);
  assert.equal(r.truncated, true);
});

test("途中の失敗では取れた分を残し、理由を返す", async () => {
  const r = await fetchAllPages(async (c) => {
    if (c === null) return { items: ["a"], nextCursor: "1" };
    throw new Error("boom");
  }, 20, (e) => (e as Error).message);
  assert.deepEqual(r.items, ["a"]);
  assert.equal(r.error, "boom");
  assert.equal(r.truncated, false);
});

test("新規の数は重複を除き、既知を除く", () => {
  assert.deepEqual(countFetched(["a", "b", "b", "c"], new Set(["a"])), { total: 3, fresh: 2 });
});

test("通知文", () => {
  assert.match(projectFetchMessage("P", { total: 5, fresh: 2, truncated: false, error: null }), /5件取得しました（うち新規2件）.*完全一致/);
  assert.match(projectFetchMessage("P", { total: 3, fresh: 1, truncated: false, error: "x" }), /取得できませんでした: x.*3件は取得済み/);
  assert.match(loadMoreMessage({ total: 50, fresh: 50, more: false }), /最後/);
  assert.equal(projectUpdateMessage({ projects: 3, confirmed: 4, guessed: 2, none: 1, unknown: 0 }), "プロジェクト3件を更新しました（確定4件／推定2件／プロジェクトなし1件）");
});

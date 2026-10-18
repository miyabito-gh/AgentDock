// 実行: node --test src/ui/orgHidden.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { isOrgOwned, shownOrgIds } from "./orgHidden.ts";

const local = (workflowId: string, released = false) => ({ org: { workflowId, taskId: "t1", roleId: "worker", released } }) as never;
const view = (cur: string | null, hist: string[], archived: string[] = []) =>
  ({ workflow: cur ? { workflowId: cur } : null, history: hist.map((workflowId) => ({ workflowId })), archived: archived.map((workflowId) => ({ workflowId })) }) as never;

test("役割チャットの隠し条件: 出せるワークフローのものだけ隠す。出せない（読めない・件数の外）ものは通常の一覧に出す", () => {
  const shown = shownOrgIds(view("wf-3", ["wf-2"]), false);
  assert.equal(isOrgOwned(local("wf-3"), shown), true);
  assert.equal(isOrgOwned(local("wf-2"), shown), true); // 過去分も、ワークフローから辿れるので隠す
  assert.equal(isOrgOwned(local("wf-1"), shown), false); // 出せない → 孤立させない
  assert.equal(isOrgOwned(local("wf-3", true), shown), false); // 手動に切替済みは従来どおり通常の一覧
  assert.equal(isOrgOwned(undefined, shown), false);
});

test("未取得は従来どおり隠す。取得に失敗していて何も無いときは隠さない", () => {
  assert.equal(shownOrgIds(null, false), null);
  assert.equal(isOrgOwned(local("wf-1"), null), true);
  const failed = shownOrgIds(null, true);
  assert.equal(isOrgOwned(local("wf-1"), failed), false);
});

test("アーカイブしたワークフローの役割チャットは、通常の一覧へ漏らさず隠し続ける", () => {
  const shown = shownOrgIds(view("wf-3", ["wf-2"], ["wf-1"]), false);
  assert.equal(isOrgOwned(local("wf-1"), shown), true);
  // 削除済み（どこにも出ない）ワークフローのものは、手動に切り替え済みでなければ孤立させず通常の一覧に出す
  assert.equal(isOrgOwned(local("wf-0"), shown), false);
});

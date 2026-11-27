// 実行: node --test src/ui/format.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { holdText, savedPartText, STOP_LABEL } from "./format.ts";

test("保存済みが取れないときは「なし」で埋めず、確認できないと書く", () => {
  assert.equal(savedPartText(null, "ログ 2件"), "保存済みの内容: 確認できません。未保存の内容: ログ 2件");
  assert.equal(savedPartText("本文 12件", null), "保存済みの内容: 本文 12件。未保存の内容: 確認できません");
  assert.ok(!savedPartText(null, null).includes("なし"));
});
test("停止未確認の文に内部語を使わない", () => {
  for (const v of Object.values(STOP_LABEL)) assert.ok(!/turn|管理実行|OS上の実体|ホスト/.test(v), v);
  assert.ok(STOP_LABEL.interruptRequested.includes("まだ確認できていません"));
});

test("接続が切れている保留は、Codex が起動していないためだと示す（ホストの受付カードの文と同じ）", () => {
  const t = holdText({ kind: "disconnected" }, (a) => a.id);
  assert.equal(t, "Codex が起動していない（または接続が切れている）ため保留しています");
});

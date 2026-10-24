// 実行: node --test src/ui/sendNotice.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { hideClosed, noticeOf } from "./sendNotice.ts";

const att = (kind: string, extra: object = {}) => ({ attemptId: "a1", clientMessageId: "c", at: 0, state: { kind, ...extra } }) as never;

test("確定した拒否は言い切り、理由を添える。内部語を使わない", () => {
  const n = noticeOf(att("rejected", { message: "busy" }))!;
  assert.equal(n.title, "送信は受け付けられませんでした");
  assert.ok(n.body.includes("busy"));
  assert.ok(!n.body.includes("確認しています"));
  assert.ok(!/受理|turn/.test(n.title + n.body));
  assert.ok(n.canRetry);
});
test("受理不明は止めていることを書く（再送不可）", () => {
  const n = noticeOf(att("acceptanceUnknown"))!;
  assert.ok(n.body.includes("止めます"));
  assert.ok(!/受理|turn/.test(n.title + n.body));
  assert.equal(n.canRetry, false);
});
test("閉じた通知は同じ識別子の間だけ隠れ、状態が変われば再び出る", () => {
  const n = noticeOf(att("acceptanceUnknown"))!;
  assert.equal(hideClosed(n, n.key), null);
  assert.equal(hideClosed(n, "other"), n);
  assert.equal(hideClosed(null, n.key), null);
  assert.notEqual(noticeOf(att("rejected", { message: "x" }))!.key, n.key);
});

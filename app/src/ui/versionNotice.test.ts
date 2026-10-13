// 実行: node --test src/ui/versionNotice.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { showVersionNotice, versionNoticeKey, isConnectionProblem, UNKNOWN_VERSION_KEY } from "./versionNotice.ts";

const mm = (actual: string) => ({ kind: "mismatch" as const, expected: "0.162.1", actual });
const unk = { kind: "unknown" as const, message: "x" };

test("閉じていなければ出す", () => {
  assert.equal(showVersionNotice(mm("0.161.0"), null), true);
  assert.equal(showVersionNotice(unk, undefined), true);
});
test("閉じた版と同じなら出さない", () => {
  assert.equal(showVersionNotice(mm("0.161.0"), versionNoticeKey(mm("0.161.0"))), false);
  assert.equal(showVersionNotice(unk, UNKNOWN_VERSION_KEY), false);
});
test("版が違えばまた出す", () => {
  assert.equal(showVersionNotice(mm("0.162.0"), versionNoticeKey(mm("0.161.0"))), true);
  assert.equal(showVersionNotice(mm("0.161.0"), UNKNOWN_VERSION_KEY), true);
  assert.equal(showVersionNotice(unk, versionNoticeKey(mm("0.161.0"))), true);
});
test("対象版に一致なら注意なし", () => {
  assert.equal(showVersionNotice({ kind: "match", version: "0.162.1" }, null), false);
});
test("接続断・起動失敗は閉じられない扱い", () => {
  assert.equal(isConnectionProblem({ kind: "disconnected" }), true);
  assert.equal(isConnectionProblem({ kind: "launchFailed" }), true);
  assert.equal(isConnectionProblem({ kind: "connected" }), false);
});

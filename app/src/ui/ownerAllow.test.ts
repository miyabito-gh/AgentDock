// 実行: node --test src/ui/ownerAllow.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { ownerRepoOf, blockedOwnerRepo, ownerAllowConfirmText } from "./ownerAllow.ts";

test("所有者違いの状態だけが許可の対象になる", () => {
  assert.equal(ownerRepoOf({ kind: "ownerMismatch", repo: "C:/w/r" } as any), "C:/w/r");
  assert.equal(ownerRepoOf({ kind: "ownerMismatch", repo: "  " } as any), null);
  assert.equal(ownerRepoOf({ kind: "notRepo" } as any), null);
  assert.equal(ownerRepoOf(null), null);
});

test("開始前の拒否理由は、準備が要る状態と所有者違いを区別する", () => {
  assert.equal(blockedOwnerRepo({ kind: "repoNotPrepared", repo: { kind: "ownerMismatch", repo: "C:/w/r" } } as any), "C:/w/r");
  assert.equal(blockedOwnerRepo({ kind: "repoNotPrepared", repo: { kind: "notRepo" } } as any), null);
  assert.equal(blockedOwnerRepo({ kind: "chatBusy" } as any), null);
  assert.equal(blockedOwnerRepo(undefined), null);
});

test("確認文は場所と、git設定を変えないことを含む", () => {
  const t = ownerAllowConfirmText("C:/w/r");
  assert.ok(t.includes("C:/w/r") && t.includes("変更しません") && t.includes("だけに許可"));
  assert.ok(t.includes("「記録する」になります") && t.includes("「記録しない」を選んでいても上書き"));
});

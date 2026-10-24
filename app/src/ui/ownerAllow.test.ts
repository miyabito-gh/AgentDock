// 実行: node --test src/ui/ownerAllow.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { ownerRepoOf, blockedOwnerRepo, ownerAllowConfirmText, ownerAllowSpec, OWNER_ALLOW_RECORD_NOTE, RECORD_CHOICE_NOTE, RECORD_SKIP_NOTE } from "./ownerAllow.ts";

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

test("記録設定が変わる一文は共通定数で、確認ダイアログの本文に入る", () => {
  assert.ok(OWNER_ALLOW_RECORD_NOTE.includes("「記録する」になります"));
  assert.ok(ownerAllowSpec("C:/w/r").body.includes(OWNER_ALLOW_RECORD_NOTE));
  assert.equal(ownerAllowSpec("C:/w/r").okLabel, "許可する");
});

test("記録の案内は、記録しない影響と取り返し方、所有者違いでは先に許可が要ることを書く", () => {
  assert.ok(RECORD_SKIP_NOTE.includes("戻せなくなります") && RECORD_SKIP_NOTE.includes("後から"));
  assert.ok(RECORD_CHOICE_NOTE.includes("先に下の許可が必要です"));
});

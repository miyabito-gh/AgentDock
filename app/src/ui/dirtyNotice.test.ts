// 実行: node --test src/ui/dirtyNotice.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { dirtyCheckApplies, dirtyNoticeText } from "./dirtyNotice.ts";

test("変更がなければ注意を出さない。件数と代表パス、残りの件数を示す", () => {
  assert.equal(dirtyNoticeText(null), null);
  assert.equal(dirtyNoticeText({ total: 0, sample: [] }), null);
  const t = dirtyNoticeText({ total: 7, sample: ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"] })!;
  assert.match(t, /7件/);
  assert.match(t, /a\.rs、b\.rs/);
  assert.match(t, /ほか2件/);
  assert.match(t, /HEAD/);
  assert.ok(!dirtyNoticeText({ total: 1, sample: ["x"] })!.includes("ほか"));
});

test("コミットのあるリポジトリだけ調べる", () => {
  assert.equal(dirtyCheckApplies({ kind: "ready", root: "r", branch: "m" } as any), true);
  assert.equal(dirtyCheckApplies({ kind: "nested", root: "r" } as any), true);
  assert.equal(dirtyCheckApplies({ kind: "notRepo" } as any), false);
  assert.equal(dirtyCheckApplies(null), false);
});

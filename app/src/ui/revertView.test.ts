// 実行: node --test src/ui/revertView.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { reasonLines, sortedItems, initialChosen, pickedPaths, forcedPaths, resultKind, turnOptions, noBaselineReason, gitKindOf, sameReason, ownerMarkVisible, ownerExplainsNoBaseline, ownerCardShown } from "./revertView.ts";

const item = (path: string, verdict: any) => ({ path, kind: null, includesTurns: [], verdict });
const rev = { kind: "revertible", summary: "" };
const need = (...codes: string[]) => ({ kind: "needsOverride", summary: "", reasons: codes.map((code) => ({ code, message: code })) });
const blk = { kind: "blocked", code: "headMoved", message: "m" };

test("要確認の理由は落とさず固定順", () => {
  const l = reasonLines(need("concurrentChange", "changedAfter", "endUnknown") as any);
  assert.deepEqual(l.map((x) => x.code), ["endUnknown", "changedAfter", "concurrentChange"]);
});
test("止めるは理由1件、戻せるは空", () => {
  assert.equal(reasonLines(blk as any)[0].label, "HEADが変わった（コミット・ブランチ切替など）");
  assert.equal(reasonLines(rev as any).length, 0);
});
test("並びは 戻せる→要確認→止める、同区分は元の順", () => {
  const s = sortedItems([item("a", blk), item("b", need("changedAfter")), item("c", rev), item("d", rev)] as any);
  assert.deepEqual(s.map((i) => i.path), ["c", "d", "b", "a"]);
});
test("初期選択は戻せるだけ。要確認は初期オフ", () => {
  const items = [item("a", rev), item("b", need("changedAfter")), item("c", blk)] as any;
  const ch = initialChosen(items);
  assert.deepEqual(ch, { a: true });
  assert.deepEqual(pickedPaths(items, ch), ["a"]);
  assert.deepEqual(forcedPaths(items, ch), []);
  assert.deepEqual(forcedPaths(items, { ...ch, b: true, c: true }), ["b"]);
});
test("失敗が1件でもあれば一部のみ", () => {
  assert.equal(resultKind({ reverted: ["a"], failed: [{}] }), "partial");
  assert.equal(resultKind({ reverted: ["a"], failed: [] }), "allReverted");
  assert.equal(resultKind({ reverted: [], failed: [] }), "nothing");
});
test("turn選択肢: 失敗・送信なし・未対応づけは出さない", () => {
  const seg = (turn: string | null, state: any, concurrent = false) => ({ turn, startedAt: 1, state, concurrent });
  const o = turnOptions([seg("t1", { kind: "ready" }), seg("t2", { kind: "failed", reason: { kind: "timeout" } }), seg(null, { kind: "ready" }), seg("t3", { kind: "endUnknown" }, true), seg("t1", { kind: "ready" })] as any);
  assert.deepEqual(o.map((x) => x.turn), ["t1", "t3"]);
  assert.match(o[1].label, /同時作業/);
});
test("控えなしの理由（文脈つき）: 設定オフ→Gitなし→Git不明→控え前の順に1つだけ", () => {
  const r = (enabled: boolean, git: any) => noBaselineReason([], { baselinesEnabled: enabled, git })!;
  assert.match(r(false, "notRepo"), /設定で控えの記録がオフ/);
  assert.match(r(true, "notRepo"), /Gitリポジトリではない/);
  assert.match(r(true, "unknown"), /確認できていない/);
  assert.match(r(true, "repo"), /控えを取る前のturn/);
  // ホストが返した失敗の理由は文脈より優先
  const f = [{ turn: "t", startedAt: 1, state: { kind: "failed", reason: { kind: "gitUnavailable" } }, concurrent: false }] as any;
  assert.match(noBaselineReason(f, { baselinesEnabled: true, git: "repo" })!, /Gitを実行できません/);
});
test("sameReason: 同一文だけ重複、別原因は両方", () => {
  assert.equal(sameReason(" a ", "a"), true);
  assert.equal(sameReason("host", "other"), false);
  assert.equal(sameReason(null, "a"), false);
});
test("gitKindOf: 確認できたときだけrepo/notRepo", () => {
  assert.equal(gitKindOf(null), "unknown");
  assert.equal(gitKindOf({ status: { kind: "ready" } } as any), "repo");
  const ns = (message: string) => gitKindOf({ status: { kind: "notSupported", message } } as any);
  assert.equal(ns("このフォルダはGitのリポジトリではありません"), "notRepo");
  assert.equal(ns("フォルダが見つかりません"), "unknown");
  assert.equal(ns("Gitを実行できません（未導入、または設定のGitの場所が違います）"), "unknown");
  assert.equal(gitKindOf({ status: { kind: "notFetched", message: "x" } } as any), "unknown");
  assert.equal(gitKindOf({ status: { kind: "notSupported", message: "このフォルダのgit管理が別のユーザー" }, ownerMismatch: true } as any), "ownerMismatch");
});
test("控えなしの理由: 取得中はnull、空は理由、失敗のみは失敗理由", () => {
  assert.equal(noBaselineReason(null), null);
  assert.ok(noBaselineReason([]));
  assert.match(noBaselineReason([{ turn: "t", startedAt: 1, state: { kind: "failed", reason: { kind: "notARepository" } }, concurrent: false }] as any)!, /Gitリポジトリ/);
  assert.equal(noBaselineReason([{ turn: "t", startedAt: 1, state: { kind: "ready" }, concurrent: false }] as any), null);
});
test("入口の印: 所有者違いで、記録しない・許可済みを選んでいないときだけ", () => {
  const pref = (choice: string, allowGit = false) => [{ path: "C:/r", choice, allowGit }] as any;
  assert.equal(ownerMarkVisible("ownerMismatch", undefined, "C:/r"), true);
  assert.equal(ownerMarkVisible("ownerMismatch", pref("Record"), "C:/r"), true);
  assert.equal(ownerMarkVisible("ownerMismatch", pref("Skip"), "C:/r"), false);
  assert.equal(ownerMarkVisible("ownerMismatch", pref("Record", true), "C:/r"), false);
  assert.equal(ownerMarkVisible("repo", undefined, "C:/r"), false);
  assert.equal(ownerMarkVisible(undefined, undefined, "C:/r"), false);
});
test("所有者違いの案内が説明済みの控えなし理由", () => {
  const ctx = (git: any, en = true) => ({ baselinesEnabled: en, git });
  const failed = (reason: any) => [{ turn: "t", startedAt: 1, state: { kind: "failed", reason }, concurrent: false }] as any;
  assert.equal(ownerExplainsNoBaseline(null, ctx("ownerMismatch")), false);
  assert.equal(ownerExplainsNoBaseline([], ctx("ownerMismatch")), true);
  assert.equal(ownerExplainsNoBaseline([], ctx("ownerMismatch", false)), false);
  assert.equal(ownerExplainsNoBaseline([], ctx("repo")), false);
  assert.equal(ownerExplainsNoBaseline(failed({ kind: "dubiousOwnership", repo: "" })), true);
  assert.equal(ownerExplainsNoBaseline(failed({ kind: "skipped" })), false);
  assert.equal(ownerExplainsNoBaseline(failed({ kind: "notARepository" })), false);
});
test("案内カードを出す条件（RepoRecordPanelと共通）", () => {
  const pref = (choice: string, allowGit = false) => [{ path: "C:/r", choice, allowGit }] as any;
  const ownerFail = [{ turn: "t", startedAt: 1, state: { kind: "failed", reason: { kind: "dubiousOwnership", repo: "C:/r" } }, concurrent: false }] as any;
  assert.equal(ownerCardShown(null, "C:/r", undefined, "C:/r"), true); // 未選択
  assert.equal(ownerCardShown(null, "C:/r", pref("Record"), "C:/r"), true); // 記録する選択済み
  assert.equal(ownerCardShown(null, "C:/r", pref("Record", true), "C:/r"), true); // 許可済み（案内は残る）
  assert.equal(ownerCardShown(null, "C:/r", pref("Skip"), "C:/r"), false); // 記録しない（一行表示）
  assert.equal(ownerCardShown(ownerFail, null, pref("Skip"), "C:/r"), false);
  assert.equal(ownerCardShown(ownerFail, null, undefined, "C:/r"), true); // 区間の失敗
  assert.equal(ownerCardShown(null, null, undefined, "C:/r"), false); // リポジトリでない
  assert.equal(ownerCardShown(null, null, undefined, null), false); // git_info不明
});

import { test } from "node:test";
import assert from "node:assert/strict";
import { composerPlaceholder } from "./composerHint.ts";

test("送れない理由が最優先", () => {
  assert.equal(composerPlaceholder("停止を確認できるまで", true, true), "停止を確認できるまで");
});
test("作業中は追加指示の案内", () => {
  assert.equal(composerPlaceholder(null, true, false), "作業中です。送ると追加指示になります");
});
test("選択の有無で文言が変わる", () => {
  assert.equal(composerPlaceholder(null, false, true), "この範囲をどう直しますか");
  assert.equal(composerPlaceholder(null, false, false), "全体をどう直しますか");
});

import { folderContains, latestReplyText, runStopped } from "./composerHint.ts";
const m = (kind: string, value?: string) => ({ kind: { kind }, text: value === undefined ? { kind: "missing" } : { kind: "value", value } });
test("最新の返答は最後のAI本文。取れていないものは飛ばす", () => {
  const turns = [{ entries: [m("agentMessage", "古い")] }, { entries: [m("userMessage", "依頼"), m("agentMessage", "新しい\n返答"), m("agentMessage")] }];
  assert.equal(latestReplyText(turns), "新しい 返答");
  assert.equal(latestReplyText([{ entries: [m("userMessage", "x")] }]), null);
  assert.equal(latestReplyText([]), null);
});
test("長い返答は切り詰める", () => {
  assert.equal(latestReplyText([{ entries: [m("agentMessage", "あ".repeat(10))] }], 4), "ああああ…");
});

test("作業フォルダが編集ルートそのもの・上位なら書ける。別の場所・未取得は書けない", () => {
  assert.equal(folderContains(String.raw`C:\work\site`, "C:/work/site"), true);
  assert.equal(folderContains(String.raw`C:\work`, String.raw`c:\WORK\site` + "\\"), true);
  assert.equal(folderContains(String.raw`C:\work\site`, String.raw`C:\work`), false);
  assert.equal(folderContains(String.raw`C:\work\site2`, String.raw`C:\work\site`), false);
  assert.equal(folderContains(null, String.raw`C:\work`), false);
});
test("実行中から停止になった瞬間だけ true", () => {
  assert.equal(runStopped(true, false), true);
  assert.equal(runStopped(false, false), false);
  assert.equal(runStopped(true, true), false);
  assert.equal(runStopped(false, true), false);
});

import { statusOf } from "./composerHint.ts";
const turn = (id: string, seq: number, state: "open" | "done" | "undone", n: number, canUndo = false, canRedo = false) =>
  ({ id, seq, atMs: seq, state, files: Array.from({ length: n }, (_, i) => ({ path: `f${i}`, change: "change" as const })), canUndo, canRedo });
test("状態バー: 変更なし・作業中・集計中・変更あり", () => {
  assert.equal(statusOf([], false, null).text, "まだ変更はありません");
  assert.equal(statusOf([turn("a", 1, "open", 0)], true, null).text, "AIが作業中です");
  assert.equal(statusOf([turn("a", 1, "open", 0)], false, null).text, "変更を集計しています");
  assert.equal(statusOf([turn("a", 1, "done", 2, true)], false, null).text, "2ファイルを変更");
});
test("状態バー: 戻した・やり直した結果を優先し、差分はそのターンを指す", () => {
  const ts = [turn("a", 1, "done", 1, false, false), turn("b", 2, "undone", 3, false, true)];
  const s = statusOf(ts, false, { kind: "undo", turnId: "b" });
  assert.equal(s.text, "3ファイルの変更を戻しました");
  assert.equal(s.turn?.id, "b");
  const r = statusOf([turn("b", 2, "done", 3, true)], false, { kind: "redo", turnId: "b" });
  assert.equal(r.text, "3ファイルの変更をやり直しました");
  // 新しい依頼が始まったら直前の操作の結果は優先しない
  assert.equal(statusOf([turn("b", 2, "undone", 3, false, true), turn("c", 3, "open", 0)], true, { kind: "undo", turnId: "b" }).text, "AIが作業中です");
});

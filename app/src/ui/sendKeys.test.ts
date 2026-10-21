import { test } from "node:test";
import assert from "node:assert/strict";
import { sendActionForKey } from "./sendKeys.ts";

const k = (o: Partial<{ key: string; ctrlKey: boolean; shiftKey: boolean; isComposing: boolean }> = {}) => ({ key: "Enter", ctrlKey: false, shiftKey: false, ...o });
test("実行中でない: 送信キーだけ送信、Ctrl+Shift+Enterは送らない", () => {
  assert.equal(sendActionForKey(k({ ctrlKey: true }), "ctrl", false, "steer"), "send");
  assert.equal(sendActionForKey(k(), "ctrl", false, "queue"), null);
  assert.equal(sendActionForKey(k(), "enter", false, "steer"), "send");
  assert.equal(sendActionForKey(k({ ctrlKey: true, shiftKey: true }), "enter", false, "queue"), null);
});
test("実行中・既定（完了後に送信）: 送信キー=キュー、Ctrl+Shift+Enter=追加指示", () => {
  assert.equal(sendActionForKey(k({ ctrlKey: true }), "ctrl", true, "queue"), "queue");
  assert.equal(sendActionForKey(k({ ctrlKey: true, shiftKey: true }), "ctrl", true, "queue"), "steer");
  assert.equal(sendActionForKey(k(), "enter", true, "queue"), "queue");
});
test("実行中・追加指示設定: 入れ替わる", () => {
  assert.equal(sendActionForKey(k({ ctrlKey: true }), "ctrl", true, "steer"), "steer");
  assert.equal(sendActionForKey(k({ ctrlKey: true, shiftKey: true }), "ctrl", true, "steer"), "queue");
  assert.equal(sendActionForKey(k(), "enter", true, "steer"), "steer");
});
test("改行・IME変換中・Enter以外は送らない", () => {
  assert.equal(sendActionForKey(k({ shiftKey: true }), "enter", true, "steer"), null);
  assert.equal(sendActionForKey(k(), "ctrl", true, "steer"), null);
  assert.equal(sendActionForKey(k({ ctrlKey: true, isComposing: true }), "ctrl", true, "steer"), null);
  assert.equal(sendActionForKey(k({ key: "a", ctrlKey: true }), "ctrl", true, "steer"), null);
});

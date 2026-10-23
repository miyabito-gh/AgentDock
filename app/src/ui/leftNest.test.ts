// 実行: node --test src/ui/leftNest.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { classifyLeft, listTitle, stripDocHeader, wfTitleText, wfDisplayName, shortStamp } from "./leftNest.ts";

const chat = (name: string | null, o: { preview?: string; used?: number | null; created?: number } = {}) => ({
  name: name === null ? { kind: "unknown" } : { kind: "value", value: name },
  preview: o.preview ? { kind: "value", value: o.preview } : { kind: "unknown" },
  lastUsedAt: o.used ?? null,
  createdAt: o.created ? { kind: "value", value: o.created } : { kind: "unknown" },
}) as never;
const T = new Date(2026, 9, 24, 13, 5).getTime();

test("資料見出しの除去と『# 』の除去（版表記は残す）", () => {
  assert.equal(stripDocHeader("［AgentDock資料 v1｜依頼整理］ 以下は Agen"), "以下は Agen");
  assert.equal(stripDocHeader("普通の題名"), "普通の題名");
  assert.equal(wfTitleText("# 設計書（要件・第2版）"), "設計書（要件・第2版）");
  assert.equal(wfDisplayName({ title: null, request: "# 依頼本文" } as never), "依頼本文");
});

test("題名: 資料見出しは本文先頭、依頼整理は日時、名前未確認は時刻つき", () => {
  assert.equal(listTitle(chat("［AgentDock資料 v1｜x］ 議事録の作り方"), undefined).text, "議事録の作り方");
  const intake = { conversation: { intake: { kind: "drafting" } } } as never;
  assert.equal(listTitle(chat("［AgentDock資料 v1｜依頼整理］ 以下は Agen", { used: T }), intake).text, "依頼整理（10/24 13:05）");
  assert.equal(listTitle(chat("自分で付けた名前"), intake).text, "自分で付けた名前");
  assert.equal(listTitle(chat(null, { used: T }), undefined).text, `名前未確認・${shortStamp(T)}`);
  assert.equal(listTitle(chat(null), undefined).text, "（名前未確認）");
  assert.equal(listTitle(chat(null, { preview: "最初の依頼" }), undefined).confirmed, false);
});

test("振り分け: 役割・依頼整理(紐づき/未開始)・通常", () => {
  const shown = new Set(["wf-1"]);
  const role = { org: { workflowId: "wf-1", released: false } } as never;
  assert.deepEqual(classifyLeft(role, shown), { kind: "role", workflowId: "wf-1" });
  assert.deepEqual(classifyLeft({ org: { workflowId: "wf-1", released: true } } as never, shown), { kind: "normal" });
  const st = (workflowId: string) => ({ conversation: { intake: { kind: "started", workflowId } } }) as never;
  assert.deepEqual(classifyLeft(st("wf-1"), shown), { kind: "intake", workflowId: "wf-1" });
  assert.deepEqual(classifyLeft(st("wf-9"), shown), { kind: "normal" });
  assert.deepEqual(classifyLeft({ conversation: { intake: { kind: "ready" } } } as never, shown), { kind: "intakeOpen" });
  assert.deepEqual(classifyLeft(undefined, shown), { kind: "normal" });
  // ピン留め優先: 畳みに入れずピン留め欄へ
  assert.deepEqual(classifyLeft(role, shown, true), { kind: "normal" });
  assert.deepEqual(classifyLeft(st("wf-1"), shown, true), { kind: "normal" });
  assert.deepEqual(classifyLeft({ conversation: { intake: { kind: "ready" } } } as never, shown, true), { kind: "normal" });
});

// 実行: node --test src/ui/orgChat.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { isOrgInstruction, orgChatTitle, parseOrgReport } from "./orgChat.ts";

const wf = { workflowId: "w", tasks: [{ id: "r1", title: "README の追加内容と変更範囲を検証" }] } as never;

test("タイトル: 役割名 タスクID: タスク名。手動に切替済み・別ワークフロー・不明タスクは null", () => {
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "r1", roleId: "reviewer", released: false }, wf), "Reviewer r1: README の追加内容と変更範囲を検証");
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "r1", roleId: "reviewer", released: true }, wf), null);
  assert.equal(orgChatTitle({ workflowId: "x", taskId: "r1", roleId: "reviewer", released: false }, wf), null);
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "zz", roleId: "reviewer", released: false }, wf), null);
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "r1", roleId: "custom", released: false }, wf), "custom r1: README の追加内容と変更範囲を検証");
});

test("指示の判定は先頭一致のみ", () => {
  assert.equal(isOrgInstruction("【この役割の指示】\nあなたは"), true);
  assert.equal(isOrgInstruction("前置き 【この役割の指示】"), false);
});

test("Reviewer の判定JSON", () => {
  assert.deepEqual(parseOrgReport('{"findings":["a"],"verdict":"pass"}'), { kind: "verdict", pass: true, findings: ["a"] });
  assert.equal(parseOrgReport('{"findings":["a"],"verdict":"maybe"}'), null);
  assert.equal(parseOrgReport('{"findings":["a"],"verdict":"pass","x":1}'), null);
});

test("Worker・計画・形違い", () => {
  assert.deepEqual(parseOrgReport('{"summary":"s","changedFiles":["a"],"tests":{"ran":true,"passed":false}}'),
    { kind: "worker", summary: "s", changedFiles: ["a"], tests: { ran: true, passed: false } });
  assert.equal(parseOrgReport('{"summary":"s","changedFiles":["a"],"tests":{"ran":true}}'), null);
  const p = parseOrgReport('{"tasks":[{"id":"w1","role":"worker","deps":[],"effects":[],"title":"t","instructions":"i"}]}');
  assert.equal(p?.kind, "plan");
  assert.equal(parseOrgReport('{"tasks":[{"id":"w1"}]}'), null);
  assert.equal(parseOrgReport("普通の文章"), null);
  assert.equal(parseOrgReport("{壊れた"), null);
});

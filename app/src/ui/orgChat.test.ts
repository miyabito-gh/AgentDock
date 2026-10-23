// 実行: node --test src/ui/orgChat.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { isOrgInstruction, orgAgentName, orgChatTitle, parseOrgReport, withOrgTitles } from "./orgChat.ts";

const wf = { workflowId: "w", tasks: [{ id: "r1", title: "README の追加内容と変更範囲を検証" }] } as never;

test("タイトル: 役割名 タスクID: タスク名。手動に切替済み・別ワークフロー・不明タスクは null", () => {
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "r1", roleId: "reviewer", released: false }, wf), "Reviewer r1: README の追加内容と変更範囲を検証");
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "r1", roleId: "reviewer", released: true }, wf), null);
  assert.equal(orgChatTitle({ workflowId: "x", taskId: "r1", roleId: "reviewer", released: false }, wf), null);
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "zz", roleId: "reviewer", released: false }, wf), null);
  assert.equal(orgChatTitle({ workflowId: "w", taskId: "r1", roleId: "custom", released: false }, wf), "custom r1: README の追加内容と変更範囲を検証");
});

test("要求元の代わりの名前: 役割・タスクID『題名』。所有していなければ null", () => {
  assert.equal(orgAgentName({ workflowId: "w", taskId: "r1", roleId: "worker", released: false }, wf), "Worker・r1『README の追加内容と変更範囲を検証』");
  assert.equal(orgAgentName({ workflowId: "w", taskId: "r1", roleId: "worker", released: true }, wf), null);
  assert.equal(orgAgentName(null, wf), null);
});

test("withOrgTitles: 名前が空・欠損の根のエージェントだけ補い、名前があれば触らない", () => {
  const l = { chat: { backend: "codex", id: "c1" }, org: { workflowId: "w", taskId: "r1", roleId: "worker", released: false } };
  const mk = (dn: unknown) => ({ chats: [{ key: { id: "c1" }, name: { kind: "missing" } }], chatLocals: [l], agents: [{ agent: { key: { id: "c1" }, displayName: dn } }, { agent: { key: { id: "x" }, displayName: { kind: "missing" } } }] }) as never;
  const out = withOrgTitles(mk({ kind: "missing" }), wf) as { agents: Array<{ agent: { displayName: { kind: string; value?: string } } }> };
  assert.equal(out.agents[0].agent.displayName.value, "Worker・r1『README の追加内容と変更範囲を検証』");
  assert.equal(out.agents[1].agent.displayName.kind, "missing");
  const keep = withOrgTitles(mk({ kind: "value", value: "自分の名前", basis: "reported" }), wf) as typeof out;
  assert.equal(keep.agents[0].agent.displayName.value, "自分の名前");
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

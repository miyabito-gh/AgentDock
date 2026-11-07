// 実行: node --test src/ui/autoAllow.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { autoAllowOption, dueAutoAllows } from "./autoAllow.ts";

const opt = (id: string, effect: string, scope: string) => ({ id, label: id, effect, scope, description: null });
const req = (id: string, chat: string, kind: string, options: unknown[], state = "pending") =>
  ({ key: { source: "s", requestId: id }, chat: { backend: "codex", id: chat }, kind: { kind }, options, state: { kind: state } }) as never;
const std = [opt("accept", "allow", "once"), opt("acceptForSession", "allow", "session"), opt("decline", "deny", "once")];

test("autoAllowOption: 承認3種の『1回だけ許可』だけを返す", () => {
  assert.equal(autoAllowOption(req("1", "a", "commandApproval", std))!.id, "accept");
  assert.equal(autoAllowOption(req("2", "a", "fileChangeApproval", std))!.id, "accept");
  assert.equal(autoAllowOption(req("3", "a", "permissionsApproval", [opt("grantTurn", "allow", "once")]))!.id, "grantTurn");
});

test("autoAllowOption: 質問・MCP許可・pending以外・1回だけの許可が無い要求は対象外", () => {
  assert.equal(autoAllowOption(req("1", "a", "userInput", std)), null);
  assert.equal(autoAllowOption(req("2", "a", "toolElicitation", std)), null);
  assert.equal(autoAllowOption(req("3", "a", "commandApproval", std, "answered")), null);
  assert.equal(autoAllowOption(req("4", "a", "commandApproval", [opt("acceptForSession", "allow", "session"), opt("decline", "deny", "once")])), null);
});

test("dueAutoAllows: モードの会話だけ、応答済み・除外会話は出さない", () => {
  const rs = [req("1", "a", "commandApproval", std), req("2", "b", "commandApproval", std), req("3", "a", "fileChangeApproval", std), req("4", "a", "userInput", std)];
  const due = dueAutoAllows(rs, new Set(["a"]), new Set(["s/3"]), () => false);
  assert.deepEqual(due.map((d) => d.r.key.requestId), ["1"]);
  assert.deepEqual(dueAutoAllows(rs, new Set(["a"]), new Set(), (c) => c === "a"), []);
});

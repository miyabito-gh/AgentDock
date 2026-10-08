// 実行: node --test src/ui/chatState.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { chatDisplayKind } from "./chatState.ts";

test("親done＋子running→子が作業中", () => assert.equal(chatDisplayKind("done", ["running"]), "childWorking"));
test("親done＋孫waiting→子が作業中", () => assert.equal(chatDisplayKind("done", ["done", "waiting"]), "childWorking"));
test("親done＋子全てdone→親の状態", () => assert.equal(chatDisplayKind("done", ["done", "closed"]), "root"));
test("親done＋子unknown→確認中", () => assert.equal(chatDisplayKind("done", ["done", "unknown"]), "childUnknown"));
test("親done＋子unknownとrunning→作業中が優先", () => assert.equal(chatDisplayKind("done", ["unknown", "running"]), "childWorking"));
test("親running＋子failed→子の失敗あり", () => assert.equal(chatDisplayKind("running", ["failed"]), "childFail"));
test("親running＋子running→親の状態", () => assert.equal(chatDisplayKind("running", ["running"]), "root"));
test("親failed＋子running→失敗を隠さない", () => assert.equal(chatDisplayKind("failed", ["running"]), "root"));
test("子なし→親の状態", () => assert.equal(chatDisplayKind("done", []), "root"));

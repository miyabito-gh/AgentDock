// 実行: node --test src/ui/folderGroups.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { collapseAll, folderKey, groupChats, shortNames, worktreeOf, flattenGroups, GENERAL_KEY, UNKNOWN_KEY } from "./folderGroups.ts";

let n = 0;
const chat = (cwd: string | null, used: number | null, extra: any = {}): any => ({
  key: { backend: "codex", id: `c${n++}` }, kind: "development", pinned: false, lastUsedAt: used,
  createdAt: { kind: "notFetched" },
  cwd: cwd === null ? { kind: "notFetched" } : { kind: "value", value: cwd, basis: {} }, ...extra,
});
const wt = (repoRoot: string, path: string): any => ({ id: "w", repoRoot, path, branch: "agentdock/x" });

test("folderKey: 区切り・末尾・大小・拡張接頭辞", () => {
  assert.equal(folderKey("C:/Users/A/Repo/"), "c:\\users\\a\\repo");
  assert.equal(folderKey("\\\\?\\C:\\Users\\A"), "c:\\users\\a");
  assert.equal(folderKey("C:\\"), "c:\\");
  assert.equal(folderKey("c:\\Users\\a\\repo"), folderKey("C:\\USERS\\A\\REPO\\"));
  assert.equal(folderKey("\\\\?\\UNC\\srv\\share\\d"), "\\\\srv\\share\\d");
});

test("所属: 一般・不明・同一フォルダの表記ゆれ・親へはまとめない", () => {
  const cs = [chat("C:\\a\\b", 5), chat("c:/A/B/", 9), chat("C:\\a", 7), chat(null, 8), { ...chat("C:\\z", 1), kind: "general" }];
  const gs = groupChats(cs, [], []);
  const keys = gs.map((g) => g.key);
  assert.deepEqual(keys, [folderKey("C:\\a\\b"), "c:\\a", GENERAL_KEY, UNKNOWN_KEY]);
  assert.equal(gs[0].chats.length, 2);
  assert.equal(gs[0].path, "c:/A/B/"); // 最近利用が最も新しいチャットの元の文字列
});

test("worktree: 台帳の配下は repoRoot へ、外部は cwd そのもの", () => {
  const w = [wt("C:\\repo", "C:\\wt\\agentdock-1")];
  assert.equal(worktreeOf("c:\\WT\\agentdock-1\\sub", w)?.repoRoot, "C:\\repo");
  assert.equal(worktreeOf("C:\\wt\\agentdock-10", w), null);
  const gs = groupChats([chat("C:\\repo", 1), chat("C:\\wt\\agentdock-1", 2), chat("C:\\other\\wt", 3)], w, []);
  assert.equal(gs.length, 2);
  assert.equal(gs.find((g) => g.key === "c:\\repo")!.chats.length, 2);
});

test("並び: ピンしたフォルダ（ピン順）→最近利用の新しい順→不明は最後。一般もピン可", () => {
  const cs = [chat("C:\\a", 10), chat("C:\\b", 30), chat("C:\\c", 20), chat(null, 99), { ...chat(null, 5), kind: "general" }];
  const gs = groupChats(cs, [], ["c:\\c", GENERAL_KEY, UNKNOWN_KEY]);
  assert.deepEqual(gs.map((g) => g.key), ["c:\\c", GENERAL_KEY, "c:\\b", "c:\\a", UNKNOWN_KEY]);
  assert.equal(gs[4].pinned, false); // 不明はピン不可
});

test("ピン留めチャットを除いて渡せば重複しない／空グループは出ない", () => {
  const a = chat("C:\\a", 1), b = chat("C:\\a", 2, { pinned: true });
  const gs = groupChats([a, b].filter((c) => !c.pinned), [], []);
  assert.equal(gs.length, 1);
  assert.equal(gs[0].chats.length, 1);
  assert.deepEqual(groupChats([], [], ["c:\\a"]), []);
});

test("短縮: 末尾2階層・ドライブ・UNC・同名は階層を延長", () => {
  const m = shortNames([{ key: "1", path: "C:\\Users\\a\\Rust\\AgentDock" }, { key: "2", path: "C:\\foo" }, { key: "3", path: "C:\\" }, { key: "4", path: "\\\\srv\\share\\d\\e" }]);
  assert.equal(m.get("1"), "Rust\\AgentDock");
  assert.equal(m.get("2"), "C:\\foo");
  assert.equal(m.get("3"), "C:\\");
  assert.equal(m.get("4"), "d\\e");
  const d = shortNames([{ key: "x", path: "C:\\p\\q\\app" }, { key: "y", path: "C:\\r\\q\\app" }, { key: "z", path: "C:\\s\\other" }]);
  assert.equal(d.get("x"), "p\\q\\app");
  assert.equal(d.get("y"), "r\\q\\app");
  assert.equal(d.get("z"), "s\\other");
  const e = shortNames([{ key: "x", path: "C:\\q\\app" }, { key: "y", path: "D:\\q\\app" }]);
  assert.equal(e.get("x"), "C:\\q\\app");
  assert.equal(e.get("y"), "D:\\q\\app");
});

test("flatten: 折りたたみ中は見出しだけ", () => {
  const gs = groupChats([chat("C:\\a", 1), chat("C:\\a", 2), chat("C:\\b", 0)], [], []);
  const flat = flattenGroups(gs, (k) => k === "c:\\a");
  assert.deepEqual(flat.map((f) => f.type), ["group", "group", "chat"]);
});

test("一括折りたたみ: 表示中の全見出しを加える（重複なし・保存済みの他キーは残す）／展開は空", () => {
  assert.deepEqual(collapseAll([], ["a", "b"]), ["a", "b"]);
  assert.deepEqual(collapseAll(["x", "a"], ["a", "b"]), ["x", "a", "b"]);
  assert.deepEqual(collapseAll(["x"], []), ["x"]);
});

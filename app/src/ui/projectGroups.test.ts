// 実行: node --test src/ui/projectGroups.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { groupByProject, matchProjects, NO_PROJECT_KEY } from "./projectGroups.ts";
import { UNKNOWN_KEY } from "./folderGroups.ts";

let n = 0;
const chat = (cwd: string | null, extra: any = {}): any => ({
  key: { backend: "codex", id: `c${n++}` }, kind: "development", pinned: false, lastUsedAt: n, projectId: null,
  createdAt: { kind: "notFetched" },
  cwd: cwd === null ? { kind: "notFetched" } : { kind: "value", value: cwd, basis: {} }, ...extra,
});
const proj = (id: string, name: string, roots: string[], position = 0): any => ({ id, name, roots, position });

test("cwdの一致: フォルダ自体・内側・接頭辞・区切り・大小文字（誤一致しない）", () => {
  const ps = [proj("a", "A", ["C:\\Users\\x\\Repo"])];
  assert.deepEqual(matchProjects(chat("c:/users/X/repo"), ps)?.projectIds, ["a"]);
  assert.deepEqual(matchProjects(chat("\\\\?\\C:\\Users\\x\\Repo\\sub\\deep"), ps)?.projectIds, ["a"]);
  assert.deepEqual(matchProjects(chat("C:\\Users\\x\\Repo2"), ps)?.projectIds, []);
  assert.equal(matchProjects(chat("C:\\Users\\x\\Repo"), ps)?.confirmed, false);
});

test("入れ子: 全rootを含む候補が複数なら roots 件数 → 一致rootの深さ → position 順", () => {
  const ps = [proj("a", "A", ["C:\\w"], 2), proj("b", "B", ["C:\\w\\sub"], 1), proj("c", "C", ["C:\\w\\sub"], 0)];
  assert.deepEqual(matchProjects(chat("C:\\w\\other"), ps)?.projectIds, ["a"]);
  assert.deepEqual(matchProjects(chat("C:\\w\\sub\\x"), ps)?.projectIds, ["c", "b", "a"]);
  const ps2 = [proj("p", "親", ["C:\\w"], 0), proj("q", "子", ["C:\\w\\sub"], 5)];
  assert.deepEqual(matchProjects(chat("C:\\w\\sub\\x"), ps2)?.projectIds, ["q", "p"]);
});

// 実データ相当: アプリ統括(roots5件)と、rootが1つだけの作成系プロジェクト。
const R = "C:\\Users\\x\\Documents\\Rust";
const five = ["C:\\Users\\x\\Documents\\project", `${R}\\Pathly`, `${R}\\MeetDock`, `${R}\\Lumark`, "C:\\Users\\x\\GoTodo"];
const real = [proj("app", "アプリ統括", five, 0), proj("meet", "MeetDock作成", [`${R}\\MeetDock`], 1), proj("goto", "gotodo作成", ["C:\\Users\\x\\GoTodo"], 2)];

test("実データ相当: 単一rootのプロジェクトの見出しが消えない", () => {
  const m = matchProjects(chat(`${R}\\MeetDock`), real)!;
  assert.deepEqual(m.projectIds, ["meet"]);
  assert.equal(m.basis, "all");
  assert.equal(m.confirmed, false);
  assert.deepEqual(matchProjects(chat("C:\\Users\\x\\GoTodo\\src"), real)?.projectIds, ["goto"]);
});

test("実データ相当: 全フォルダを持つチャットは roots の多い方、他は multi", () => {
  const c = chat(`${R}\\MeetDock`);
  const m = matchProjects(c, real, five.slice())!;
  assert.deepEqual(m.projectIds, ["app", "meet", "goto"]);
  assert.equal(m.basis, "all");
  const { groups, multi } = groupByProject([c], real, [], () => five);
  assert.deepEqual(groups.filter((g) => g.chats.length > 0).map((g) => g.key), ["project:app"]);
  assert.deepEqual(multi.get(c.key.id), ["MeetDock作成", "gotodo作成"]);
});

test("実データ相当: 先頭root推定・一部のroot・一致なし", () => {
  const p = matchProjects(chat("C:\\Users\\x\\Documents\\project"), real)!;
  assert.deepEqual([p.projectIds, p.basis], [["app"], "primary"]);
  const q = matchProjects(chat(`${R}\\Lumark\\src`), real)!;
  assert.deepEqual([q.projectIds, q.basis], [["app"], "partial"]);
  assert.deepEqual(matchProjects(chat("D:\\other"), real)?.projectIds, []);
});

test("重複なしの件数: 1チャットは1つの見出しにだけ数える", () => {
  const cs = [chat(`${R}\\MeetDock`), chat("C:\\Users\\x\\GoTodo"), chat("C:\\Users\\x\\Documents\\project")];
  const { groups } = groupByProject(cs, real, []);
  assert.equal(groups.reduce((n, g) => n + g.chats.length, 0), 3);
  assert.deepEqual(groups.map((g) => g.chats.length).sort(), [1, 1, 1]);
});

test("複数フォルダのプロジェクト: どのフォルダでも所属、深い方で比較", () => {
  const ps = [proj("a", "A", ["C:\\p", "C:\\q"]), proj("b", "B", ["C:\\q\\z"])];
  assert.deepEqual(matchProjects(chat("C:\\q\\k"), ps)?.projectIds, ["a"]);
  assert.deepEqual(matchProjects(chat("C:\\q\\z"), ps)?.projectIds, ["b"]);
});

test("projectId があれば確定（cwdより優先）。一覧に無いIDはcwdで推測", () => {
  const ps = [proj("a", "A", ["C:\\a"]), proj("b", "B", ["C:\\b"])];
  const m = matchProjects(chat("C:\\a", { projectId: "b" }), ps);
  assert.deepEqual(m, { projectIds: ["b"], confirmed: true });
  assert.deepEqual(matchProjects(chat("C:\\a", { projectId: "zzz" }), ps), { projectIds: ["a"], confirmed: false, basis: "all" });
});

test("cwd不明は判定不能（なしにしない）、一般チャットはなし", () => {
  const ps = [proj("a", "A", ["C:\\a"])];
  assert.equal(matchProjects(chat(null), ps), null);
  assert.deepEqual(matchProjects(chat(null, { kind: "general" }), ps), { projectIds: [], confirmed: false });
});

test("グループ化: 複数一致は先頭の見出しにだけ出る・件数・推測の数・なし・不明・0件の見出しも出る", () => {
  const ps = [proj("a", "A", ["C:\\r"], 0), proj("b", "B", ["C:\\r"], 1), proj("c", "C", ["C:\\c"], 2), proj("e", "E", ["C:\\e"], 3)];
  const both = chat("C:\\r"), onlyC = chat("C:\\c\\x"), conf = chat("C:\\zzz", { projectId: "c" }), none = chat("D:\\x"), unk = chat(null);
  const { groups, multi } = groupByProject([both, onlyC, conf, none, unk], ps, []);
  const by = (k: string) => groups.find((g) => g.key === k);
  assert.equal(by("project:a")!.chats.length, 1);
  assert.equal(by("project:b")!.chats.length, 0);
  assert.equal(by("project:c")!.chats.length, 2);
  assert.equal(by("project:c")!.guessedCount, 1);
  assert.equal(by("project:e")!.chats.length, 0);
  assert.equal(by("project:e")!.guessedCount, undefined);
  assert.deepEqual(groups.filter((g) => g.kind === "project").map((g) => g.key), ["project:c", "project:a", "project:b", "project:e"]);
  assert.equal(groupByProject([both], ps, [], undefined, false).groups.some((g) => g.key === "project:e"), false);
  assert.equal(by(NO_PROJECT_KEY)!.chats.length, 1);
  assert.equal(by(UNKNOWN_KEY)!.chats.length, 1);
  assert.deepEqual(multi.get(both.key.id), ["B"]);
  assert.equal(groups.at(-1)!.key, UNKNOWN_KEY);
  assert.equal(groups.at(-2)!.key, NO_PROJECT_KEY);
});

test("ピン留めしたプロジェクトが先頭", () => {
  const ps = [proj("a", "A", ["C:\\a"]), proj("b", "B", ["C:\\b"])];
  const { groups } = groupByProject([chat("C:\\a"), chat("C:\\b")], ps, ["project:a"]);
  assert.equal(groups[0].key, "project:a");
  assert.equal(groups[0].pinned, true);
});

test("0件のプロジェクトも position 順に出る・ピンは先頭・チャット無しでも全件", () => {
  const ps = [proj("x", "X", ["C:\\x"], 2), proj("y", "Y", ["C:\\y"], 0), proj("z", "Z", ["C:\\z"], 1)];
  const { groups } = groupByProject([], ps, []);
  assert.deepEqual(groups.map((g) => g.key), ["project:y", "project:z", "project:x"]);
  assert.ok(groups.every((g) => g.chats.length === 0));
  const pinned = groupByProject([], ps, ["project:x"]).groups;
  assert.deepEqual(pinned.map((g) => g.key), ["project:x", "project:y", "project:z"]);
  assert.equal(pinned[0].pinned, true);
  assert.deepEqual(groupByProject([], ps, [], undefined, false).groups, []);
});

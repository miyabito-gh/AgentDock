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

test("入れ子は深い方を優先、同じ深さは複数", () => {
  const ps = [proj("a", "A", ["C:\\w"]), proj("b", "B", ["C:\\w\\sub"]), proj("c", "C", ["C:\\w\\sub"])];
  assert.deepEqual(matchProjects(chat("C:\\w\\other"), ps)?.projectIds, ["a"]);
  assert.deepEqual(matchProjects(chat("C:\\w\\sub\\x"), ps)?.projectIds, ["b", "c"]);
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
  assert.deepEqual(matchProjects(chat("C:\\a", { projectId: "zzz" }), ps), { projectIds: ["a"], confirmed: false });
});

test("cwd不明は判定不能（なしにしない）、一般チャットはなし", () => {
  const ps = [proj("a", "A", ["C:\\a"])];
  assert.equal(matchProjects(chat(null), ps), null);
  assert.deepEqual(matchProjects(chat(null, { kind: "general" }), ps), { projectIds: [], confirmed: false });
});

test("グループ化: 複数一致は先頭の見出しにだけ出る・件数・推測の数・なし・不明・空プロジェクトは作らない", () => {
  const ps = [proj("a", "A", ["C:\\r"], 0), proj("b", "B", ["C:\\r"], 1), proj("c", "C", ["C:\\c"], 2), proj("e", "E", ["C:\\e"], 3)];
  const both = chat("C:\\r"), onlyC = chat("C:\\c\\x"), conf = chat("C:\\zzz", { projectId: "c" }), none = chat("D:\\x"), unk = chat(null);
  const { groups, multi } = groupByProject([both, onlyC, conf, none, unk], ps, []);
  const by = (k: string) => groups.find((g) => g.key === k);
  assert.equal(by("project:a")!.chats.length, 1);
  assert.equal(by("project:b"), undefined);
  assert.equal(by("project:c")!.chats.length, 2);
  assert.equal(by("project:c")!.guessedCount, 1);
  assert.equal(by("project:e"), undefined);
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

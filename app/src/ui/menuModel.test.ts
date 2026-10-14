// 実行: node --test src/ui/menuModel.test.ts （tscの対象外。台帳・メニュー定義・ショートカットの整合）
import { test } from "node:test";
import assert from "node:assert/strict";
import { ALL_COMMANDS, cwdOpenWhy, filterCommands, type CommandContext, type CommandView } from "./commands.ts";
import { HEADER_MORE, MENUBAR, entryIds, resolveEntries, resolveMenu } from "./menuModel.ts";
import { SHORTCUTS, kbdForAct, matchShortcut, type KeyLike } from "./shortcuts.ts";

const ctx = (o: Partial<CommandContext> = {}): CommandContext => ({
  cap: undefined, connected: true, hasChat: true, busy: false, deletePending: false, externalRunning: false, gitMissing: false,
  pinned: false, archived: false, isGeneral: false, isLive: true, cwdWhy: null, folderCount: 0, checked: {}, ...o,
});
const viewsOf = (c: CommandContext): CommandView[] => ALL_COMMANDS.map((cmd) => ({
  cmd, state: cmd.enabled(c), label: cmd.dynamicLabel ? cmd.dynamicLabel(c) : cmd.label, act: cmd.dynamicAct ? cmd.dynamicAct(c) : cmd.act, checked: cmd.checked?.(c),
}));
const key = (o: Partial<KeyLike> & { key: string }): KeyLike => ({ ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...o });

test("台帳の id は重複しない", () => {
  const ids = ALL_COMMANDS.map((c) => c.id);
  assert.equal(new Set(ids).size, ids.length);
});

test("メニュー・ヘッダーが参照する id はすべて台帳にある", () => {
  const known = new Set(ALL_COMMANDS.map((c) => c.id));
  for (const m of MENUBAR) for (const id of entryIds(m.entries)) assert.ok(known.has(id), `${m.title}: ${id}`);
  for (const id of entryIds(HEADER_MORE)) assert.ok(known.has(id), `ヘッダー: ${id}`);
});

test("台帳の全項目が、所属メニューのどこかに置かれている", () => {
  const placed = new Map<string, string>();
  for (const m of MENUBAR) for (const id of entryIds(m.entries)) placed.set(id, m.title);
  for (const c of ALL_COMMANDS) {
    assert.ok(placed.has(c.id), `${c.id} がメニューにない`);
    assert.equal(placed.get(c.id), c.group, `${c.id} の所属が違う`);
  }
});

test("危険操作は各メニューの最下段で、直前が区切り。区切りは見出しか危険操作の前だけ", () => {
  const danger = new Set(ALL_COMMANDS.filter((c) => c.danger).map((c) => c.id));
  for (const m of MENUBAR) {
    const e = m.entries;
    const dangerIdx = e.map((x, i) => (x !== "-" && "id" in x && danger.has(x.id) ? i : -1)).filter((i) => i >= 0);
    if (dangerIdx.length) {
      const first = dangerIdx[0];
      assert.equal(e[first - 1], "-", `${m.title}: 危険操作の前に区切り`);
      assert.deepEqual(dangerIdx, e.slice(first).map((_, i) => first + i), `${m.title}: 危険操作は末尾に連続`);
    }
    e.forEach((x, i) => {
      if (x !== "-") return;
      const nx = e[i + 1];
      assert.ok(nx && (("heading" in nx) || ("id" in nx && danger.has(nx.id))), `${m.title}: 区切りの次は見出しか危険操作`);
    });
  }
  // 危険操作は台帳でもこの2つだけ。
  assert.deepEqual([...danger].sort(), ["deleteChat", "quit"]);
});

test("メニュー・Ctrl+K に出る kbd は SHORTCUTS にある表記だけ", () => {
  const shown = new Set(SHORTCUTS.flatMap((s) => [s.keys, s.kbd].filter((x): x is string => !!x)));
  for (const c of ALL_COMMANDS) {
    assert.equal(c.kbd, kbdForAct(c.act), c.id);
    if (c.kbd) assert.ok(shown.has(c.kbd), `${c.id}: ${c.kbd}`);
  }
  // 嘘のショートカット表示は出さない（中断に Esc は付けない）。
  assert.equal(ALL_COMMANDS.find((c) => c.id === "interrupt")?.kbd, undefined);
  assert.equal(kbdForAct("newChatQuick"), "Ctrl+N");
  assert.equal(kbdForAct("newChat"), "Ctrl+Shift+N");
  assert.equal(kbdForAct("toggleMonitor"), "Ctrl+Shift+M");
  assert.equal(kbdForAct("find"), "Ctrl+F");
});

test("SHORTCUTS の id は重複しない・act は台帳に存在する", () => {
  const ids = SHORTCUTS.map((s) => s.id);
  assert.equal(new Set(ids).size, ids.length);
  const acts = new Set(ALL_COMMANDS.flatMap((c) => [c.act, c.dynamicAct?.(ctx({ archived: true }))].filter(Boolean)));
  for (const s of SHORTCUTS) if (s.act) assert.ok(acts.has(s.act), `${s.id}: ${s.act}`);
});

test("同じ act に2つの名前がない（意図した共有は許可リスト）", () => {
  const SHARED = new Set(["review"]); // コードレビュー／レビューを別チャットで は同じダイアログの入口
  const byAct = new Map<string, Set<string>>();
  for (const c of ALL_COMMANDS) byAct.set(c.act, (byAct.get(c.act) ?? new Set()).add(c.label));
  for (const [act, names] of byAct) if (!SHARED.has(act)) assert.equal(names.size, 1, `${act}: ${[...names].join(" / ")}`);
  // 同じラベルを別の act に使わない。
  const byLabel = new Map<string, Set<string>>();
  for (const c of ALL_COMMANDS) byLabel.set(c.label, (byLabel.get(c.label) ?? new Set()).add(c.act));
  for (const [label, acts] of byLabel) assert.equal(acts.size, 1, label);
});

test("matchShortcut: Ctrl+Shift+M / Ctrl+N / Ctrl+B と Ctrl+Alt+B を取り違えない", () => {
  const m = key({ key: "M", code: "KeyM", ctrlKey: true, shiftKey: true });
  assert.ok(matchShortcut(m, "toggleMonitor"));
  assert.ok(matchShortcut(m, "monitorClose"));
  assert.ok(!matchShortcut(m, "newChat") && !matchShortcut(m, "quickNew") && !matchShortcut(m, "toggleLeft"));
  assert.ok(!matchShortcut({ ...m, shiftKey: false }, "toggleMonitor"));
  assert.ok(!matchShortcut({ ...m, altKey: true }, "toggleMonitor"));
  const n = key({ key: "n", code: "KeyN", ctrlKey: true });
  assert.ok(matchShortcut(n, "quickNew") && !matchShortcut(n, "newChat"));
  assert.ok(matchShortcut({ ...n, shiftKey: true }, "newChat") && !matchShortcut({ ...n, shiftKey: true }, "quickNew"));
  const b = key({ key: "b", code: "KeyB", ctrlKey: true });
  assert.ok(matchShortcut(b, "toggleLeft") && !matchShortcut(b, "toggleRight"));
  const ab = { ...b, altKey: true };
  assert.ok(matchShortcut(ab, "toggleRight") && !matchShortcut(ab, "toggleLeft"));
  assert.ok(matchShortcut(key({ key: "k", ctrlKey: true }), "palette"));
  assert.ok(matchShortcut(key({ key: "F3" }), "findNext") && matchShortcut(key({ key: "F3", shiftKey: true }), "findPrev"));
  assert.ok(!matchShortcut(key({ key: "F3", shiftKey: true }), "findNext"));
});

test("matchShortcut: 変換中（isComposing）は常に false。判定を持たない id も false", () => {
  for (const s of SHORTCUTS) {
    assert.equal(matchShortcut(key({ key: "n", code: "KeyN", ctrlKey: true, isComposing: true }), s.id), false);
  }
  assert.equal(matchShortcut(key({ key: "m", code: "KeyM", ctrlKey: true, shiftKey: true, isComposing: true }), "toggleMonitor"), false);
  assert.equal(matchShortcut(key({ key: "n", ctrlKey: true }), "paneWidth"), false);
  assert.equal(matchShortcut(key({ key: "n", ctrlKey: true }), "unknown"), false);
});

test("無効な項目は必ず理由を持つ（チャット未選択）", () => {
  for (const v of viewsOf(ctx({ hasChat: false, cap: undefined }))) {
    if (v.state.kind === "disabled") assert.ok(v.state.reason.length > 0, v.cmd.id);
  }
  const by = (id: string, c: CommandContext) => viewsOf(c).find((v) => v.cmd.id === id)!;
  const none = ctx({ hasChat: false });
  for (const id of ["renameChat", "togglePin", "archive", "exportMd", "openCwd", "cwdChange", "deleteChat", "find"]) {
    assert.deepEqual(by(id, none).state, { kind: "disabled", reason: "チャットを選んでください" }, id);
  }
  assert.equal(by("newChat", none).state.kind, "enabled");
  assert.equal(by("quit", none).state.kind, "enabled");
});

test("無効理由: 削除保留・一般チャット・live・再照合", () => {
  const by = (id: string, c: CommandContext) => viewsOf(c).find((v) => v.cmd.id === id)!;
  assert.deepEqual(by("renameChat", ctx({ deletePending: true })).state, { kind: "disabled", reason: "削除保留中のため使えません" });
  assert.deepEqual(by("deleteChat", ctx({ deletePending: true })).state, { kind: "disabled", reason: "削除保留中です" });
  assert.deepEqual(by("cwdChange", ctx({ isGeneral: true })).state, { kind: "disabled", reason: "開発チャットだけで使えます" });
  assert.deepEqual(by("openCwd", ctx({ isGeneral: true })).state, { kind: "disabled", reason: "一般チャット（作業フォルダなし）です" });
  assert.deepEqual(by("newChatSameFolder", ctx({ isGeneral: true })).state, { kind: "disabled", reason: "一般チャット（作業フォルダなし）です" });
  assert.equal(by("newChatSameFolder", ctx()).state.kind, "enabled");
  assert.deepEqual(by("newChatSameFolder", ctx({ hasChat: false })).state, { kind: "disabled", reason: "チャットを選んでください" });
  assert.deepEqual(by("openCwd", ctx({ cwdWhy: "フォルダが見つかりません" })).state, { kind: "disabled", reason: "フォルダが見つかりません" });
  assert.equal(by("openCwd", ctx()).state.kind, "enabled");
  assert.deepEqual(by("reloadHistory", ctx({ isLive: true })).state, { kind: "disabled", reason: "最新の状態です" });
  assert.equal(by("reloadHistory", ctx({ isLive: false })).state.kind, "enabled");
  // 作業中アーカイブは従来どおり許可。
  assert.equal(by("archive", ctx({ busy: true })).state.kind, "enabled");
});

test("状態つき項目: ピン留め・アーカイブは1項目で文言と操作名が切り替わる／✓", () => {
  const by = (id: string, c: CommandContext) => viewsOf(c).find((v) => v.cmd.id === id)!;
  assert.equal(by("togglePin", ctx()).label, "ピン留め");
  assert.equal(by("togglePin", ctx({ pinned: true })).label, "ピン留めを外す");
  assert.equal(by("togglePin", ctx({ pinned: true })).act, "togglePin");
  assert.deepEqual([by("archive", ctx()).label, by("archive", ctx()).act], ["アーカイブ", "archiveChat"]);
  assert.deepEqual([by("archive", ctx({ archived: true })).label, by("archive", ctx({ archived: true })).act], ["アーカイブを解除", "unarchiveChat"]);
  assert.equal(by("toggleLeft", ctx({ checked: { toggleLeft: true } })).checked, true);
  assert.equal(by("toggleLeft", ctx()).checked, false);
  assert.equal(by("newChat", ctx()).checked, undefined);
});

test("resolveEntries: 台帳にない id は捨て、不要な区切り・空の見出しを詰める", () => {
  const views = viewsOf(ctx());
  const r = resolveEntries(["-", { heading: "空" }, "-", { id: "nope" }, { heading: "A" }, { id: "newChat" }, "-", { id: "nope" }, "-"], views);
  assert.deepEqual(r.map((x) => x.kind), ["heading", "item"]);
  for (const m of MENUBAR) {
    const e = resolveMenu(m.title, views);
    assert.ok(e.length > 0, m.title);
    assert.notEqual(e[0].kind, "sep");
    assert.notEqual(e[e.length - 1].kind, "sep");
  }
});

test("Ctrl+K の検索: 別名・所属メニューで探せる", () => {
  const views = viewsOf(ctx());
  const ids = (q: string) => filterCommands(views, q).map((v) => v.cmd.id);
  assert.ok(ids("MCP").includes("toolServers"));
  assert.ok(ids("plugins").includes("toolServers"));
  for (const q of ["Fast", "速度", "memories", "personality"]) assert.ok(ids(q).includes("backendStatus"), q);
  assert.ok(ids("監視窓").includes("toggleMonitor"));
  assert.ok(ids("名前").includes("renameChat"));
  assert.ok(ids("ピン留めを外す").includes("togglePin"));
  assert.ok(ids("アーカイブを解除").includes("archive"));
  assert.equal(filterCommands(views, "  ").length, views.length);
  assert.equal(filterCommands(views, "存在しない語xyz").length, 0);
});

test("上部メニューは チャット｜作業｜表示｜ツール｜ヘルプ の順", () => {
  assert.deepEqual(MENUBAR.map((m) => m.title), ["チャット", "作業", "表示", "ツール", "ヘルプ"]);
});

test("cwdOpenWhy: モック・不明・ネットワークパス・missing・unconfirmed・開ける", () => {
  assert.equal(cwdOpenWhy("C:/a", "ok", false), "モックでは開きません");
  assert.equal(cwdOpenWhy(null, undefined, true), "作業フォルダを取得できていません");
  assert.equal(cwdOpenWhy("\\\\srv\\share", "ok", true), "ネットワークパスは開きません");
  assert.equal(cwdOpenWhy("C:/a", "missing", true), "フォルダが見つかりません");
  assert.equal(cwdOpenWhy("C:/a", "unconfirmed", true), "フォルダの有無を確認できていません");
  assert.equal(cwdOpenWhy("C:/a", "ok", true), null);
});

test("MCP・Plugins／Codex の状態／利用状況は、未接続・能力不明でも押せる", () => {
  const vs = viewsOf(ctx({ connected: false, hasChat: false, cap: undefined }));
  for (const id of ["toolServers", "backendStatus", "usage"]) assert.equal(vs.find((v) => v.cmd.id === id)!.state.kind, "enabled", id);
});

test("作業フォルダの項目名: 複数フォルダのときだけ件数を添える", () => {
  const by = (c: CommandContext) => viewsOf(c).find((v) => v.cmd.id === "cwdChange")!.label;
  assert.equal(by(ctx({ folderCount: 1 })), "作業フォルダの確認と変更…");
  assert.equal(by(ctx({ folderCount: 3 })), "作業フォルダの確認と変更…（3）");
});

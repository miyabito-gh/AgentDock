// 実行: node --test src/ui/shortcuts.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { shortcutsFor } from "./shortcuts.ts";

test("shortcutsFor: 追加指示が既定のときだけ表示が入れ替わる", () => {
  const q = shortcutsFor("queue").find((s) => s.id === "send")!;
  const s = shortcutsFor("steer");
  assert.ok(q.label.includes("完了後に送信"));
  assert.ok(s.find((x) => x.id === "send")!.label.includes("追加指示"));
  assert.ok(s.find((x) => x.id === "sendQueue")!.label.includes("完了後に送信"));
  assert.equal(s.length, shortcutsFor("queue").length);
});

test("Markdown編集欄: キーの判定と、編集欄の中かどうか", async () => {
  const { matchShortcut, inMarkdownEditor } = await import("./shortcuts.ts");
  const k = (key: string, o: Partial<{ ctrlKey: boolean; shiftKey: boolean; altKey: boolean; isComposing: boolean }> = {}) => ({ key, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...o });
  assert.ok(matchShortcut(k("s", { ctrlKey: true }), "mdSave"));
  assert.ok(matchShortcut(k("B", { ctrlKey: true }), "mdBold"));
  assert.ok(matchShortcut(k("k", { ctrlKey: true }), "mdLink"));
  assert.ok(matchShortcut(k("h", { ctrlKey: true }), "mdReplace"));
  assert.ok(!matchShortcut(k("k", { ctrlKey: true, shiftKey: true }), "mdLink"));
  assert.ok(!matchShortcut(k("s", { ctrlKey: true, isComposing: true }), "mdSave"));
  assert.ok(matchShortcut(k("ArrowLeft", { shiftKey: true, altKey: true }), "mdHeadingDown"));
  assert.ok(matchShortcut(k("ArrowRight", { shiftKey: true, altKey: true }), "mdHeadingUp"));
  assert.ok(!matchShortcut(k("ArrowRight", { shiftKey: true }), "mdHeadingUp"));
  const inside = { closest: (s: string) => (s === "[data-md-editor]" ? {} : null) };
  assert.ok(inMarkdownEditor(inside as unknown as EventTarget));
  assert.ok(!inMarkdownEditor({ closest: () => null } as unknown as EventTarget));
  assert.ok(!inMarkdownEditor(null));
  assert.ok(!inMarkdownEditor({} as EventTarget));
});
test("shortcutsFor: Markdown編集欄のキーが一覧に載る", () => {
  const md = shortcutsFor("queue").filter((s) => s.where === "Markdown編集欄");
  for (const id of ["mdSave", "mdBold", "mdItalic", "mdLink", "mdFind", "mdReplace", "mdIndent", "mdEnter", "mdHeading"]) assert.ok(md.some((s) => s.id === id), id);
});

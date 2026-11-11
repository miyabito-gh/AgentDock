// 実行: node --test src/ui/replySettingsLogic.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { effortChoices, hasMainModifier, keyName, rulesDirty, shortcutFromEvent } from "./replySettingsLogic.ts";

const ev = (code: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) =>
  ({ key: "", code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods });

test("キー入力からショートカットの表記を作る（修飾キーの順は Ctrl, Alt, Shift, Super）", () => {
  assert.equal(shortcutFromEvent(ev("KeyR", { ctrlKey: true, altKey: true })), "Ctrl+Alt+R");
  assert.equal(shortcutFromEvent(ev("F9", { shiftKey: true, ctrlKey: true })), "Ctrl+Shift+F9");
  assert.equal(shortcutFromEvent(ev("Digit1", { metaKey: true })), "Super+1");
  assert.equal(shortcutFromEvent(ev("ArrowUp", { altKey: true })), "Alt+Up");
  assert.equal(shortcutFromEvent(ev("Space", { ctrlKey: true, altKey: true })), "Ctrl+Alt+Space");
});

test("修飾キーだけ・対応外のキーはまだ決まらない", () => {
  assert.equal(shortcutFromEvent(ev("ControlLeft", { ctrlKey: true })), null);
  assert.equal(shortcutFromEvent(ev("AltRight", { altKey: true })), null);
  assert.equal(keyName("IntlYen"), null);
});

test("Ctrl・Alt・Win のどれかが必要（キー単体と Shift だけは不可）", () => {
  assert.equal(hasMainModifier("Ctrl+Alt+R"), true);
  assert.equal(hasMainModifier("Super+1"), true);
  assert.equal(hasMainModifier("Shift+R"), false);
  assert.equal(hasMainModifier("F9"), false);
});

test("強度の選択肢は、選んだモデル（指定なしなら既定のモデル）が示すものだけ", () => {
  const models = [
    { id: "a", isDefault: false, efforts: [{ id: "low" }, { id: "high" }] },
    { id: "b", isDefault: true, efforts: [{ id: "medium" }] },
  ] as any;
  assert.deepEqual(effortChoices(models, "a"), ["low", "high"]);
  assert.deepEqual(effortChoices(models, null), ["medium"]);
  assert.deepEqual(effortChoices(models, "zzz"), []);
  assert.deepEqual(effortChoices([], null), []);
});

test("変更の有無: 空と未指定を区別しない（null 同士）", () => {
  const s = { shortcut: "Ctrl+Alt+R", commonRules: "", replyRules: "", polishRules: "", model: null, effort: "low", lastMode: "reply" } as any;
  assert.equal(rulesDirty(s, { ...s }), false);
  assert.equal(rulesDirty(s, { ...s, replyRules: "署名は付けない" }), true);
  assert.equal(rulesDirty(s, { ...s, effort: null }), true);
  assert.equal(rulesDirty(s, { ...s, shortcut: "Ctrl+Alt+K" }), false);
});

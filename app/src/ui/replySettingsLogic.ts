// 設定「返信・推敲」の純粋なロジック（キー入力 → ショートカットの表記、モデルに応じた強度の選択肢）。React・Tauri に依存しない。
// 実行: node --test src/ui/replySettingsLogic.test.ts （tscの対象外）
import type { ComposeSettings, ModelInfo } from "../ipc/types";

export interface KeyLike { key: string; code: string; ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean }

const NAMED: Record<string, string> = {
  Space: "Space", Enter: "Enter", Tab: "Tab", Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown", Insert: "Insert", Delete: "Delete", Backspace: "Backspace",
  ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
};

/** 押されたキーの本体（修飾キー以外）。対応外は null。物理キー（code）で見るので、キーボード配列や IME の影響を受けない。 */
export function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  const fn = /^F([1-9]|1[0-9]|2[0-4])$/.exec(code);
  if (fn) return `F${fn[1]}`;
  return NAMED[code] ?? null;
}

/** キー入力からショートカットの表記（例 Ctrl+Alt+R）を作る。修飾キーだけ・対応外のキーのときは null（まだ決まらない）。 */
export function shortcutFromEvent(e: KeyLike): string | null {
  const name = keyName(e.code);
  if (!name) return null;
  const mods = [e.ctrlKey ? "Ctrl" : "", e.altKey ? "Alt" : "", e.shiftKey ? "Shift" : "", e.metaKey ? "Super" : ""].filter(Boolean);
  return [...mods, name].join("+");
}

/** Ctrl・Alt・Win のどれかを含むか（ホストの検査と同じ。キー単体や Shift だけは登録できない）。 */
export function hasMainModifier(shortcut: string): boolean {
  return shortcut.split("+").some((p) => ["Ctrl", "Alt", "Super"].includes(p.trim()));
}

/** 強度の選択肢: 選んだモデル（指定なしなら一覧の既定のモデル）が示すものだけ。示されなければ空。 */
export function effortChoices(models: ModelInfo[], model: string | null): string[] {
  const m = (model ? models.find((x) => x.id === model) : models.find((x) => x.isDefault)) ?? null;
  return m ? m.efforts.map((e) => e.id) : [];
}

/** 文章ルール・モデル・強度の変更があるか（ショートカットは別に扱う）。 */
export function rulesDirty(saved: ComposeSettings, draft: ComposeSettings): boolean {
  return saved.commonRules !== draft.commonRules || saved.replyRules !== draft.replyRules || saved.polishRules !== draft.polishRules
    || (saved.model ?? null) !== (draft.model ?? null) || (saved.effort ?? null) !== (draft.effort ?? null);
}

export const RULES_MAX_CHARS = 4000;

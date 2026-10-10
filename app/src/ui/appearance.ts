// 表示設定（テーマ・文字サイズ、M51）の適用。`AppSettings.appearance` が正で、localStorage は起動時のちらつき防止の写しだけ。
// 純粋関数（resolveTheme・attrsOf・parseStored）と、DOM／localStorage／タイトルバーへ反映する部分に分ける。
import type { AppearanceSettings, BodyTextSize, ThemePref, UiTextSize } from "../ipc/types";

export const APPEARANCE_STORAGE_KEY = "agentdock.appearance.v1";

export const DEFAULT_APPEARANCE: AppearanceSettings = { theme: "System", uiText: "Normal", bodyText: "Normal" };

/** 実際に使うテーマ。`System` のときは OS の設定（osDark）に従う。 */
export function resolveTheme(pref: ThemePref, osDark: boolean): "light" | "dark" {
  if (pref === "Light") return "light";
  if (pref === "Dark") return "dark";
  return osDark ? "dark" : "light";
}

export interface AppearanceAttrs { theme: "light" | "dark"; uiSize: "small" | "normal" | "large"; bodySize: "small" | "normal" | "large" | "xlarge" }

/** `<html>` の `data-theme`・`data-ui-size`・`data-body-size` に付ける値。 */
export function attrsOf(a: AppearanceSettings, osDark: boolean): AppearanceAttrs {
  return {
    theme: resolveTheme(a.theme, osDark),
    uiSize: a.uiText.toLowerCase() as AppearanceAttrs["uiSize"],
    bodySize: a.bodyText.toLowerCase() as AppearanceAttrs["bodySize"],
  };
}

const THEMES: ThemePref[] = ["System", "Light", "Dark"];
const UI_SIZES: UiTextSize[] = ["Small", "Normal", "Large"];
const BODY_SIZES: BodyTextSize[] = ["Small", "Normal", "Large", "XLarge"];

/** localStorage の写しを読む。壊れた値・未知の値は既定値に直す（写しなので、読めなくても困らない）。 */
export function parseStored(raw: string | null): AppearanceSettings {
  if (!raw) return DEFAULT_APPEARANCE;
  try {
    const v = JSON.parse(raw) as Partial<AppearanceSettings> | null;
    return {
      theme: THEMES.includes(v?.theme as ThemePref) ? (v!.theme as ThemePref) : DEFAULT_APPEARANCE.theme,
      uiText: UI_SIZES.includes(v?.uiText as UiTextSize) ? (v!.uiText as UiTextSize) : DEFAULT_APPEARANCE.uiText,
      bodyText: BODY_SIZES.includes(v?.bodyText as BodyTextSize) ? (v!.bodyText as BodyTextSize) : DEFAULT_APPEARANCE.bodyText,
    };
  } catch { return DEFAULT_APPEARANCE; }
}

const osDarkNow = (): boolean => typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches;

/** `<html>` へ反映する（DOMだけ。タイトルバー・写しは触らない）。 */
export function applyAttrs(a: AppearanceSettings): void {
  const at = attrsOf(a, osDarkNow());
  const el = document.documentElement;
  el.dataset.theme = at.theme;
  el.dataset.uiSize = at.uiSize;
  el.dataset.bodySize = at.bodySize;
}

/** 起動時、描画前に呼ぶ。最後に反映した値の写しで `data-*` を付ける（ホストの設定が届いたら `applyAppearance` で上書き）。 */
export function applyStoredAppearance(): void {
  let raw: string | null = null;
  try { raw = window.localStorage.getItem(APPEARANCE_STORAGE_KEY); } catch { /* 読めなければ既定値 */ }
  applyAttrs(parseStored(raw));
}

/** 設定を反映し、写しを更新し、タイトルバーのテーマを合わせる（失敗しても画面の中は切り替わっているので、エラーは出さない）。 */
export function applyAppearance(a: AppearanceSettings): void {
  applyAttrs(a);
  try { window.localStorage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(a)); } catch { /* 写しなので失敗してよい */ }
  void import("@tauri-apps/api/window")
    .then((m) => m.getCurrentWindow().setTheme(a.theme === "System" ? null : a.theme === "Light" ? "light" : "dark"))
    .catch(() => undefined);
}

/** `System` のとき OS のテーマ変更を購読して反映し直す。戻り値で購読を解除。`System` 以外は何も購読しない。 */
export function watchSystemTheme(a: AppearanceSettings): () => void {
  if (a.theme !== "System" || typeof window === "undefined" || typeof window.matchMedia !== "function") return () => undefined;
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  const on = () => applyAttrs(a);
  mq.addEventListener("change", on);
  return () => mq.removeEventListener("change", on);
}

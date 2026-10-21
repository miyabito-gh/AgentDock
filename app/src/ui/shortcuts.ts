// ショートカットの唯一の出典（DESIGN_MENUS §4.3）。
// メニューの .kbd 表示・ツールチップ・ヘルプの一覧・キー判定は、すべてここを引く。ハンドラーがあるものだけを載せる。
// 規則: 日本語入力の変換中（isComposing）は、どのショートカットも反応しない。

export type ShortcutWhere = "全体" | "通常画面" | "入力欄" | "検索" | "一覧" | "監視窓" | "ダイアログ";

export interface Shortcut {
  id: string;
  /** ヘルプに出す全体の表記（例: "Ctrl+F／F3／Shift+F3"）。 */
  keys: string;
  /** メニュー右端などに1つだけ出す表記。なければ keys。 */
  kbd?: string;
  label: string;
  where: ShortcutWhere;
  /** 対応する台帳の操作名（App の act）。 */
  act?: string;
}

export const SHORTCUTS: Shortcut[] = [
  { id: "quickNew", keys: "Ctrl+N", label: "新しいチャット（すぐ始める）", where: "通常画面", act: "newChatQuick" },
  { id: "newChat", keys: "Ctrl+Shift+N", label: "新しいチャット（詳細を指定）…", where: "通常画面", act: "newChat" },
  { id: "palette", keys: "Ctrl+K", label: "すべての操作…", where: "通常画面", act: "palette" },
  { id: "find", keys: "Ctrl+F／F3／Shift+F3", kbd: "Ctrl+F", label: "会話内を検索／次／前", where: "通常画面", act: "find" },
  { id: "toggleLeft", keys: "Ctrl+B", label: "チャット一覧の表示切替", where: "通常画面", act: "toggleLeft" },
  { id: "toggleRight", keys: "Ctrl+Alt+B", label: "エージェントのドックの表示切替", where: "通常画面", act: "toggleRight" },
  { id: "toggleMonitor", keys: "Ctrl+Shift+M", label: "監視窓を開く／閉じる", where: "通常画面", act: "toggleMonitor" },
  { id: "monitorClose", keys: "Ctrl+Shift+M", label: "監視窓を閉じる", where: "監視窓" },
  { id: "send", keys: "Enter または Ctrl+Enter（設定による）", label: "送信（実行中は完了後に送信。設定で変更）", where: "入力欄" },
  { id: "sendQueue", keys: "Ctrl+Shift+Enter", label: "追加指示で送る（実行中。設定で変更）", where: "入力欄" },
  { id: "sendDialog", keys: "Ctrl+Enter", label: "送信（side相談・編集して再送）", where: "ダイアログ" },
  { id: "rowMenu", keys: "Shift+F10／アプリケーションキー", label: "行の操作メニュー", where: "一覧" },
  { id: "paneWidth", keys: "←→／Home／End／Enter", label: "ペイン幅の調整／既定に戻す", where: "一覧" },
  { id: "close", keys: "Esc", label: "メニュー・検索・ダイアログを閉じる", where: "全体" },
];

export const shortcutById = (id: string): Shortcut | undefined => SHORTCUTS.find((s) => s.id === id);

/** 操作名（act）に対応するメニュー右端の表記。なければ undefined（ハンドラーのない表示は出さない）。 */
export const kbdForAct = (act: string): string | undefined => {
  const s = SHORTCUTS.find((x) => x.act === act);
  return s ? (s.kbd ?? s.keys) : undefined;
};

/** ツールチップ用。例: tipWithKey("チャット一覧を閉じる", "toggleLeft") → "チャット一覧を閉じる（Ctrl+B）"。 */
export const tipWithKey = (text: string, id: string): string => {
  const s = shortcutById(id);
  return s ? `${text}（${s.kbd ?? s.keys}）` : text;
};

import type { RunningInputMode } from "./sendKeys";
export type { RunningInputMode };

/** 設定「実行中の入力」に合わせた一覧。送信キーと Ctrl+Shift+Enter の役割だけが入れ替わる（ヘルプ表示用）。 */
export function shortcutsFor(mode: RunningInputMode): Shortcut[] {
  if (mode === "queue") return SHORTCUTS;
  return SHORTCUTS.map((s) => s.id === "send" ? { ...s, label: "送信（実行中は追加指示で送る）" }
    : s.id === "sendQueue" ? { ...s, label: "完了後に送信（実行中）" } : s);
}

/** キー判定に必要な最小限のイベント形（テストしやすいよう KeyboardEvent の一部だけ）。 */
export interface KeyLike { key: string; code?: string; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; metaKey: boolean; isComposing?: boolean }

type Matcher = (e: KeyLike) => boolean;
const plain = (e: KeyLike, shift: boolean, alt: boolean) => e.ctrlKey && e.shiftKey === shift && e.altKey === alt && !e.metaKey;
const letter = (e: KeyLike, ch: string) => e.key.toLowerCase() === ch;

const MATCHERS: Record<string, Matcher> = {
  quickNew: (e) => plain(e, false, false) && letter(e, "n"),
  newChat: (e) => plain(e, true, false) && letter(e, "n"),
  palette: (e) => plain(e, false, false) && letter(e, "k"),
  find: (e) => plain(e, false, false) && letter(e, "f"),
  findNext: (e) => e.key === "F3" && !e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey,
  findPrev: (e) => e.key === "F3" && !e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey,
  // B・M は配列によって key が変わるので物理キー（code）で見る。
  toggleLeft: (e) => plain(e, false, false) && e.code === "KeyB",
  toggleRight: (e) => plain(e, false, true) && e.code === "KeyB",
  toggleMonitor: (e) => plain(e, true, false) && e.code === "KeyM",
  monitorClose: (e) => plain(e, true, false) && e.code === "KeyM",
};

/** キー入力が、そのショートカットか。変換中は常に false。判定を持たない id（ペイン幅など個別実装のもの）も false。 */
export function matchShortcut(e: KeyLike, id: string): boolean {
  if (e.isComposing) return false;
  const m = MATCHERS[id];
  return m ? m(e) : false;
}

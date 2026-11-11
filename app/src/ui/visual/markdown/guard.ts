// Markdown 編集欄の未保存を、画面の外（Visual Editor を閉じる・別のファイルを開く）から確かめるための窓口。
// 編集欄（遅延読み込み）が開いている間だけ登録する。登録がなければ「未保存なし」として通す。
// 実行: node --test src/ui/visual/markdown/guard.test.ts （tscの対象外）
export interface MarkdownGuard {
  dirty: () => boolean;
  /** 未保存があるとき、離れてよいかを利用者に確かめる（保存して続ける・保存せず続ける・やめる）。続けてよければ true。 */
  leave: () => Promise<boolean>;
}

let current: MarkdownGuard | null = null;

/** 編集欄が登録する。戻り値で登録を外す（後から登録された別の編集欄を外さない）。 */
export function registerMarkdownGuard(guard: MarkdownGuard): () => void {
  current = guard;
  return () => { if (current === guard) current = null; };
}
export const markdownDirty = (): boolean => !!current?.dirty();
/** 画面を離れる前に呼ぶ。未保存がなければ確認なしで true。 */
export function confirmLeaveMarkdown(): Promise<boolean> {
  return current && current.dirty() ? current.leave() : Promise.resolve(true);
}

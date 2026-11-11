// 未保存の編集の一時退避（アプリの実行中だけ。ファイルには書かない）。
// Visual Editor を閉じる・別の画面へ移るなどで編集欄が消えても、同じファイルを開き直したときに、
// ファイルが変わっていなければ編集中の内容を戻せる。ファイルが変わっていたら捨てる（古い内容で上書きしない）。
// 実行: node --test src/ui/visual/markdown/draftStash.test.ts （tscの対象外）
const drafts = new Map<string, { text: string; stamp: string }>();
const LIMIT = 8;

export function stashDraft(key: string, text: string, stamp: string): void {
  drafts.delete(key);
  drafts.set(key, { text, stamp });
  while (drafts.size > LIMIT) drafts.delete(drafts.keys().next().value as string);
}
/** 退避した内容を取り出す（取り出したら消す）。読み込んだファイルの印が退避時と同じときだけ返す。 */
export function takeDraft(key: string, stamp: string): string | null {
  const d = drafts.get(key);
  drafts.delete(key);
  return d && d.stamp === stamp ? d.text : null;
}
export const dropDraft = (key: string): void => { drafts.delete(key); };

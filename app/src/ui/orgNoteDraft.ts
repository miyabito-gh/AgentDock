// 依頼整理のチャットから、組織画面の「ワークフローへの追加指示」の入力欄へ本文を写すための受け渡し（DESIGN_ORG_R2 2.2）。
// 写すだけで、自動では送らない（送るのは組織画面の「追加する」）。
let pending = "";

export function setNoteDraft(text: string): void {
  pending = text;
}

/** 受け渡された本文を取り出す（一度だけ）。 */
export function takeNoteDraft(): string {
  const t = pending;
  pending = "";
  return t;
}

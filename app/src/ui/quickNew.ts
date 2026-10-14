// クイック新規（DESIGN_IMPROVE §1）の既定値。純粋ロジックのみ。
// 作業フォルダは常に指定なしの一般チャット、worktreeなし、権限は常に初期値（持ち越さない）。モデルだけ前回の作成を引き継ぐ。
// 実行: node --test src/ui/quickNew.test.ts （tscの対象外）

export interface QuickModel { id: string; isDefault: boolean; defaultEffort: string | null }
export interface QuickLast { model: { model: string; effort: string | null } | null }

/** 前回のモデルが一覧にあればそれ、なければ既定のモデル、なければ先頭、なければ空。 */
export function pickQuickModel(models: QuickModel[], last: QuickLast | null | undefined): string {
  const l = last?.model?.model;
  if (l && models.some((m) => m.id === l)) return l;
  return models.find((m) => m.isDefault)?.id ?? models[0]?.id ?? "";
}

/** 作成を呼べる条件: 本文が空でなく、作成中でもない。 */
export function canQuickSend(text: string, busy: boolean): boolean {
  return !busy && text.trim().length > 0;
}

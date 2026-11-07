// 編集対象ファイルの変更の印とプレビュー再読み込みの突き合わせ（純粋ロジック）。
// seen=ポーリングの基準（最初の印、または再読み込みに成功した印）、reloaded=再読み込みに成功した印。
// 基準を取る前に書き換わっていた分は、reloadedが空のままなので、AI終了後の既存の再読み込みが追いつく。
// 実行: node --test src/ui/visual/stampSync.test.ts （tscの対象外）
export interface StampState { seen: string | null; reloaded: string | null }
export const NO_STAMP: StampState = { seen: null, reloaded: null };
export type StampStep = "baseline" | "reload" | "none";

/** ポーリングで印を見たときの次の動き。初回は基準にするだけ、印が変わっていれば再読み込み。 */
export const pollStep = (state: StampState, stamp: string): StampStep => state.seen === null ? "baseline" : stamp !== state.seen ? "reload" : "none";
/** 動きの結果を反映する。再読み込みに失敗したら更新しない（次の周期で再試行する）。 */
export function afterStep(state: StampState, stamp: string, step: StampStep, ok: boolean): StampState {
  if (step === "baseline") return { ...state, seen: stamp };
  if (step === "reload" && ok) return { seen: stamp, reloaded: stamp };
  return state;
}
/** 変更の確認後の再読み込みを省けるか。直前に同じ印で再読み込みに成功しているときだけ。 */
export const canSkipReload = (state: StampState, stamp: string): boolean => state.reloaded !== null && state.reloaded === stamp;
/** 変更の確認後の再読み込みが成功したとき。 */
export const afterAutoReload = (stamp: string): StampState => ({ seen: stamp, reloaded: stamp });

// ダイアログのフォーカス管理の純粋部分（要素の選別と循環）。実行: node --test src/ui/focusTrap.test.ts （tscの対象外）
// DOM に触る部分は useDialogFocus.ts。ここは Node で試せるよう、要素の性質だけを受け取る。

/** フォーカスを受けられる可能性のある要素の候補（Tab 順の判定前）。 */
export const FOCUSABLE_SELECTOR = 'a[href],button,input,select,textarea,summary,[tabindex]';

export interface FocusCand {
  disabled: boolean;
  /** 非表示（display:none・hidden・見えない親の中）。 */
  hidden: boolean;
  tabIndex: number;
  /** type=hidden の入力。 */
  hiddenInput?: boolean;
}

/** Tab で到達できる要素か。無効・非表示・tabindex が負のものは除く。 */
export function isTabbable(c: FocusCand): boolean {
  return !c.disabled && !c.hidden && !c.hiddenInput && c.tabIndex >= 0;
}

/** 候補の並びから、Tab で到達できる要素の添字だけを返す（並びは変えない）。 */
export function tabbableIndexes(cands: FocusCand[]): number[] {
  const out: number[] = [];
  cands.forEach((c, i) => { if (isTabbable(c)) out.push(i); });
  return out;
}

/** 循環。cur は現在の位置（枠の外・どれでもないときは -1）、len は到達できる要素の数。戻り値は次にフォーカスする位置。要素が無ければ null。 */
export function cycleIndex(len: number, cur: number, shift: boolean): number | null {
  if (len <= 0) return null;
  if (cur < 0 || cur >= len) return shift ? len - 1 : 0;
  return shift ? (cur === 0 ? len - 1 : cur - 1) : (cur === len - 1 ? 0 : cur + 1);
}

/** Tab を自前で処理する必要があるか。端で外へ抜けるとき、または枠の外にいるときだけ（それ以外はブラウザの順序に任せる）。 */
export function needsTrap(len: number, cur: number, shift: boolean): boolean {
  if (len <= 0) return true;
  if (cur < 0 || cur >= len) return true;
  return shift ? cur === 0 : cur === len - 1;
}

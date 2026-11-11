// 返信・推敲パネルの純粋なロジック（表示文・押せる条件）。React・Tauri に依存しない。
// 実行: node --test src/pet/composeLogic.test.ts （tscの対象外）
import type { ComposeMode, ComposeOrigin, ComposeView } from "../ipc/types";

export const MODE_LABEL: Record<ComposeMode, string> = { reply: "返信", polish: "推敲" };

/** 再調整のワンタッチ指示（同じ会話へ追加で送る文）。 */
export const QUICK_ADJUSTS: ReadonlyArray<{ label: string; instruction: string }> = [
  { label: "短く", instruction: "もっと短くしてください。" },
  { label: "柔らかく", instruction: "もっと柔らかい表現にしてください。" },
  { label: "丁寧に", instruction: "もっと丁寧な表現にしてください。" },
];

export type Tone = "ok" | "warn" | "busy" | "info";
export interface StatusLine { tone: Tone; text: string }

/** 取り込みの出どころの短い表示（「クリップボードから」など）。 */
export function originLabel(origin: ComposeOrigin): string {
  switch (origin) {
    case "clipboard": return "クリップボードから";
    case "selection": return "選択した文章";
    case "none": return "貼り付け";
  }
}

/** 常に見える状態の1行（平易な日本語）。`sourceText` は入力欄の今の文章。 */
export function statusLine(view: ComposeView, sourceText: string, localError: string | null): StatusLine {
  if (localError) return { tone: "warn", text: localError };
  const s = view.state;
  switch (s.kind) {
    case "generating":
      return { tone: "busy", text: view.round > 1 ? "調整しています…（閉じると中止します）" : "作成しています…（閉じると中止します）" };
    case "done":
      return { tone: "ok", text: "できました。内容を確認して、コピーしてください" };
    case "failed":
      return { tone: "warn", text: `うまくいきませんでした: ${s.message}` };
    case "unknown":
      return { tone: "warn", text: s.message };
    case "idle": {
      if (sourceText.trim() === "") return { tone: "warn", text: "文章がありません。コピーしてから「取り込み直す」を押すか、下の欄に貼り付けてください" };
      const from = view.origin === "clipboard" ? "クリップボードから取り込みました" : view.origin === "selection" ? "選択した文章を取り込みました" : "文章を確認して、「作成」を押してください";
      return { tone: "info", text: view.notice ? `${from}。${view.notice}` : from };
    }
  }
}

/** 作成（新しい会話）を押せるか。作成中・文章が空のときは押せない。 */
export function canCreate(view: ComposeView, sourceText: string, busy: boolean): boolean {
  return !busy && view.state.kind !== "generating" && sourceText.trim() !== "";
}

/** 再調整を押せるか（同じ会話が生きていて、作成中でなく、指示がある）。 */
export function canAdjust(view: ComposeView, instruction: string, busy: boolean): boolean {
  return !busy && view.canAdjust && view.state.kind !== "generating" && instruction.trim() !== "";
}

/** 結果をコピーできるか。作成中でも前の結果は残っていればコピーできるが、新しい結果を待っている間は押させない。 */
export function canCopy(view: ComposeView, busy: boolean): boolean {
  return !busy && view.state.kind !== "generating" && !!view.result && view.result.trim() !== "";
}

/** 主ボタン: 結果があり作り直しを求めていなければ「コピー」、それ以外は「作成」。 */
export function primaryAction(view: ComposeView): "copy" | "create" {
  return view.result && view.state.kind !== "idle" ? "copy" : "create";
}

/** 文字数の表示（例 1,234文字）。 */
export function charCount(text: string): string {
  return `${[...text].length.toLocaleString("ja-JP")}文字`;
}

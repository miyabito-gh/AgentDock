// 編集中の依頼欄の案内文（純粋ロジック）。送れない理由を最優先、次に作業中（送ると追加指示）、最後に選択の有無。
// 実行: node --test src/ui/visual/composerHint.test.ts （tscの対象外）
import type { DirectTurn } from "./api";

export function composerPlaceholder(lock: string | null, running: boolean, hasSelection: boolean): string {
  if (lock) return lock;
  if (running) return "作業中です。送ると追加指示になります";
  return hasSelection ? "この範囲をどう直しますか" : "全体をどう直しますか";
}

/** 会話の作業フォルダが編集対象のルートを含むか（AIが書ける場所か）。区切り・大文字小文字・末尾の区切りを無視して、同じ場所かその下かを見る。 */
export function folderContains(cwd: string | null, root: string): boolean {
  if (!cwd) return false;
  const norm = (value: string) => value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  const c = norm(cwd), r = norm(root);
  return !!c && (r === c || r.startsWith(c + "/"));
}

/** 実行中→停止の変わり目か（依頼のターンを閉じる合図）。会話が変わったときは前の状態を引き継がない。 */
export const runStopped = (prevRunning: boolean, running: boolean): boolean => prevRunning && !running;

interface ReplyEntry{ kind: { kind: string }; text: { kind: string; value?: string } }
/** 畳んでいる間に見せる最新のAIの返答。最後の返答の本文（空白をつぶして、max文字で切り詰め）。本文が取れていない返答は飛ばし、なければnull。 */
export function latestReplyText(turns: { entries: ReplyEntry[] }[], max = 200): string | null {
  for (let i = turns.length - 1; i >= 0; i--) {
    const es = turns[i].entries;
    for (let j = es.length - 1; j >= 0; j--) {
      const e = es[j];
      if (e.kind.kind !== "agentMessage" || e.text.kind !== "value") continue;
      const flat = (e.text.value ?? "").replace(/\s+/g, " ").trim();
      if (!flat) continue;
      const chars = Array.from(flat);
      return chars.length > max ? chars.slice(0, max).join("") + "…" : flat;
    }
  }
  return null;
}

/** 状態バーの1行。直前に戻す・やり直したときはその結果を優先し、そうでなければ最新の依頼での変更を示す。差分は表示している依頼を指す。 */
export function statusOf(turns: DirectTurn[], running: boolean, last: { kind: "undo" | "redo"; turnId: string } | null): { text: string; turn: DirectTurn | null } {
  const undoTurn = [...turns].reverse().find((t) => t.canUndo) ?? null;
  const redoTurn = turns.find((t) => t.canRedo) ?? null;
  const latest = turns.length ? turns[turns.length - 1] : null;
  // 戻す・やり直した結果は、その後に新しい依頼が始まる（開いた依頼がある）までは優先する。
  const lastTurn = last && !turns.some((t) => t.state === "open") ? turns.find((t) => t.id === last.turnId && (last.kind === "undo" ? t.state === "undone" : t.state === "done")) ?? null : null;
  if (lastTurn && last) return { turn: lastTurn, text: `${lastTurn.files.length}ファイルの変更を${last.kind === "undo" ? "戻しました" : "やり直しました"}` };
  const shown = latest?.state === "open" ? latest : undoTurn ?? redoTurn ?? latest;
  if (!shown) return { turn: null, text: "まだ変更はありません" };
  if (shown.state === "open") return { turn: shown, text: running ? "AIが作業中です" : "変更を集計しています" };
  if (shown.state === "undone") return { turn: shown, text: `${shown.files.length}ファイルの変更を戻しました` };
  return { turn: shown, text: `${shown.files.length}ファイルを変更` };
}


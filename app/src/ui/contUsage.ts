// 継続チャット（DESIGN_CONT §5・§7）の表示文と判定の純関数。画面は ContinuityDialogs.tsx・HandoffNotice.tsx。
// 規則: 未取得・非対応・前回の保存値・圧縮後を区別して書く。使用率を推定しない（0や空で代用しない）。
//       引き継ぎは利用者の送信操作の中だけで行う。ここは「案内を出すか」「何と表示するか」だけを決める。
// 実行: node --test src/ui/contUsage.test.ts （型は ipc/types の import type のみ）
import type { ContextUsageView } from "../ipc/types";

/** 目的メモの上限（文字数。ホストの上限と同じ 4,000）。 */
export const PURPOSE_MAX = 4000;
/** 「強い注意」の割合。 */
export const STRONG_PERCENT = 90;

/** 文字数（サロゲートペアを1字と数える。ホストの `chars().count()` に合わせる）。 */
export const charCount = (s: string): number => Array.from(s).length;

export type UsageTone = "normal" | "warn" | "strong" | "dim";
export interface UsageChip { text: string; tip: string; tone: UsageTone }

const pad = (n: number) => String(n).padStart(2, "0");
const validTime = (ms: number) => Number.isFinite(ms) && ms > 0 && !Number.isNaN(new Date(ms).getTime());

/** 観測時刻の整形。同じ日なら `HH:MM`、違う日なら `M/D HH:MM`。不正な時刻は null。 */
export function observedText(ms: number, now: number = Date.now()): string | null {
  if (!validTime(ms)) return null;
  const d = new Date(ms);
  const n = new Date(now);
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return d.toDateString() === n.toDateString() ? hm : `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}

const num = (n: number) => n.toLocaleString("en-US");

/** 入力欄の補助行の「文脈 62%」チップ。threshold が null（案内しない）のときは色を付けない（90% 以上の強い注意だけ）。 */
export function usageChip(u: ContextUsageView, threshold: number | null, now: number = Date.now()): UsageChip {
  switch (u.kind) {
    case "known": {
      const at = observedText(u.observedAt, now);
      const tip = `使用 ${num(u.used)} / 上限 ${num(u.window)} トークン${at ? `、${at} の応答時点` : ""}${u.live ? "" : "。前回の応答時点の値です（この接続ではまだ更新されていません）"}`;
      if (!u.live) return { text: `文脈 ${u.percent}%（前回）`, tip, tone: "dim" };
      const tone: UsageTone = u.percent >= STRONG_PERCENT ? "strong" : threshold !== null && u.percent >= threshold ? "warn" : "normal";
      return { text: `文脈 ${u.percent}%`, tip, tone };
    }
    case "windowUnknown":
      return { text: "文脈 未取得", tip: `使用 ${num(u.used)} トークン。上限が示されていないため、割合は出しません`, tone: "dim" };
    case "notFetched":
      return { text: "文脈 未取得", tip: "この会話の文脈の使用量を、まだ受け取っていません", tone: "dim" };
    case "afterCompaction":
      return { text: "文脈 圧縮後・未取得", tip: "文脈が圧縮されました。次の応答で使用量を受け取るまで、割合は分かりません", tone: "dim" };
    case "unsupported":
      return { text: "文脈 非対応", tip: "このAIでは、文脈の使用量を取得できません", tone: "dim" };
  }
}

/**
 * 閾値の案内を出すか。Known かつ live（この接続で受け取った値）のときだけ。
 * declinedAt は「このthreadで続ける」を押した時点の割合（押していなければ null）。次の値が 90% 以上に達したら、押していても再び出す。
 */
export function shouldSuggest(u: ContextUsageView, threshold: number | null, declinedAt: number | null): boolean {
  if (threshold === null || u.kind !== "known" || !u.live) return false;
  if (u.percent < threshold) return false;
  if (declinedAt === null) return true;
  return u.percent >= STRONG_PERCENT && declinedAt < STRONG_PERCENT;
}

/** 引き継ぎの操作を出せない理由（出せるときは null）。組織実行所有・引き継ぎ済み・worktree 結び付き（Q3）。 */
export function handoffIneligible(a: { orgOwned: boolean; supersededBy: boolean; worktree: boolean }): string | null {
  if (a.orgOwned) return "組織実行のチャットは、引き継げません";
  if (a.supersededBy) return "このチャットは、すでに新しい thread へ引き継ぎ済みです";
  if (a.worktree) return "worktree に結び付いたチャットは、この版では引き継げません（作業場所を新しい thread へ移せないため）";
  return null;
}

/** 目的メモの先頭1行（空行は飛ばす）。無ければ null。長いものは切り詰める。 */
export function purposeFirstLine(text: string, max = 80): string | null {
  const line = text.split(/\r?\n/).map((l) => l.trim()).find((l) => l.length > 0);
  if (!line) return null;
  const chars = Array.from(line);
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : line;
}

/** 目的メモを保存できない理由（保存できるときは null）。空にして保存＝削除は許す。 */
export function purposeSaveBlock(text: string, original: string, orgOwned: boolean): string | null {
  if (orgOwned) return "組織実行が所有するチャットでは、目的メモを編集できません";
  const n = charCount(text);
  if (n > PURPOSE_MAX) return `目的メモは ${num(PURPOSE_MAX)} 字までです（現在 ${num(n)} 字。${num(n - PURPOSE_MAX)} 字超えています）`;
  if (text === original) return "変更がありません";
  return null;
}

/** 案内行の文。使用率の行（threshold）と手動（manual）で冒頭だけ変える。rounds は資料に入れる往復数。 */
export function noticeText(mode: "threshold" | "manual", percent: number | null, rounds: number): string {
  const body = `新しい thread へ引き継ぎます（目的メモ＋直近${rounds}往復）`;
  return mode === "threshold" && percent !== null ? `文脈が ${percent}% です。次の送信で${body}` : `次の送信で${body}`;
}

/** 資料の統計行（確認ダイアログ）。 */
export function statsText(p: { chars: number; roundsIncluded: number; roundsAvailable: number; truncated: boolean }): string {
  return `${num(p.chars)} 字・直近 ${p.roundsIncluded} 往復（会話全体で ${p.roundsAvailable} 往復）${p.truncated ? "・長いため一部を省略しています" : ""}`;
}

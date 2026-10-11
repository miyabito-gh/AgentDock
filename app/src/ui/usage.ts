// 利用状況（利用上限・使用量）の表示文。「Codex の状態」ダイアログと利用状況のポップオーバーが共用する（二重実装しない）。
// 規則: 使用率はバックエンドが返した値のまま（残量・料金・回復の推測に換算しない）。
//       取れなかったものは、取得不能・非対応・未確認を区別して理由つきで書く。0や空で代用しない。
// 型は ipc/types のものだけを使う（実行時の import なし）。
import type { BackendStatus, Known, LimitWindowRole, RateLimitWindowView, UsageSummary } from "../ipc/types";

export const ROLE_TEXT: Record<LimitWindowRole, string> = { primary: "短い期間の枠", secondary: "長い期間の枠" };

const pad = (n: number) => String(n).padStart(2, "0");

/** 時刻として妥当か（有限・正・Date が有効）。 */
export const validTime = (ms: number): boolean => Number.isFinite(ms) && ms > 0 && !Number.isNaN(new Date(ms).getTime());

/** リセット時刻の整形。同じ日なら `HH:MM`、違う日なら `M/D HH:MM`（ローカル時刻）。 */
export function formatReset(ms: number, now: number): string {
  const d = new Date(ms);
  const t = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const n = new Date(now);
  const same = d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate();
  return same ? t : `${d.getMonth() + 1}/${d.getDate()} ${t}`;
}

/** 取得時刻（`取得 20:17`）。 */
export const fetchedText = (ms: number): string => { const d = new Date(ms); return `取得 ${pad(d.getHours())}:${pad(d.getMinutes())}`; };

/** リセット時刻の文。示されていなければ「示されていません」（推測しない）。 */
export function resetText(k: Known<number>, now: number): string {
  switch (k.kind) {
    case "value": return validTime(k.value) ? `リセット ${formatReset(k.value, now)}` : "リセット時刻は示されていません";
    case "notFetched": return "リセット時刻は未確認";
    case "unsupported": return "リセット時刻は非対応";
    case "missing": return "リセット時刻は示されていません";
  }
}

/** 枠1つ分の1行。使用率はそのまま、期間は分のまま（換算しない）。 */
export function windowLine(w: RateLimitWindowView, now: number): string {
  const period = w.windowMinutes.kind === "value" ? `${w.windowMinutes.value}分` : w.windowMinutes.kind === "unsupported" ? "非対応" : w.windowMinutes.kind === "notFetched" ? "未確認" : "欠損";
  return `${ROLE_TEXT[w.role]}: 使用率 ${w.usedPercent}%（期間 ${period}）／${resetText(w.resetsAt ?? { kind: "missing" }, now)}`;
}

/** 累計の使用量の1行。 */
export function usageLine(u: UsageSummary): string {
  const n = (k: Known<number>) => (k.kind === "value" ? String(k.value) : k.kind === "unsupported" ? "非対応" : k.kind === "notFetched" ? "未確認" : "欠損");
  return `累計 ${n(u.lifetimeTokens)} トークン／1日の最大 ${n(u.peakDailyTokens)} トークン`;
}

/** 読めなかった項目の説明。取得不能（読取りの失敗）・非対応・未確認（値が返らなかった）を区別し、理由を添える。 */
export function unavailableText(k: Known<unknown>, error: string | null | undefined, loggedOut: boolean): string | null {
  switch (k.kind) {
    case "value": return null;
    case "unsupported": return `このアカウントでは非対応です${error ? `（${error}）` : ""}`;
    case "notFetched": return `取得できませんでした${error ? `（${error}）` : ""}`;
    case "missing": return loggedOut ? "未確認です（Codex が値を返しません。ログインしていない可能性があります）" : "未確認です（Codex が値を返しませんでした）";
  }
}

/** アカウント情報が無い（未ログインの可能性）か。 */
export const looksLoggedOut = (s: BackendStatus): boolean => s.accountKind.kind === "missing";

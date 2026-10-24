// 送信結果の通知カードの文言（純粋関数）。確定した拒否は言い切り、「受理」「turn」は利用者の語で書く。
// 実行: node --test src/ui/sendNotice.test.ts （tscの対象外）
import type { SendAttempt } from "../ipc/gen/SendAttempt";

export interface SendNotice {
  /** 閉じた通知を覚えるための識別子（試行ID＋状態）。状態が変われば別の通知として再び出る。 */
  key: string;
  cls: "warn" | "err"; title: string; body: string; canRetry: boolean; alt?: { label: string; run: () => void };
}

export function noticeOf(a: SendAttempt | undefined, steer: { attemptId: string; queueInstead: () => void } | null = null): SendNotice | null {
  if (!a) return null;
  const key = `${a.attemptId}:${a.state.kind}`;
  switch (a.state.kind) {
    case "rejected":
      if (steer && steer.attemptId === a.attemptId) {
        return { key, cls: "err", title: "追加指示は受け付けられませんでした", body: `実行中のやりとりがすでに終わっている可能性があります（${a.state.message}）。自動では切り替えません。`, canRetry: false,
          alt: { label: "完了後に送信へ切り替える", run: steer.queueInstead } };
      }
      return { key, cls: "err", title: "送信は受け付けられませんでした", body: `送信は受け付けられませんでした: ${a.state.message}。もう一度送るには、「もう一度送る」を押してください（自動では再送しません）。`, canRetry: true };
    case "acceptanceUnknown":
      return { key, cls: "warn", title: "送信が受け付けられたか確認できません", body: "応答がなく、Codex が送信を受け付けたか分かりません。履歴と自動で照合しています。確認できるまで、再送と新しい送信は止めます。", canRetry: false };
    case "notFoundAfterReconcile":
      return { key, cls: "warn", title: "履歴に送信の記録がないことを確認しました", body: "この送信は受け付けられていません。必要なら、もう一度送ることができます。", canRetry: true };
    default:
      return null;
  }
}

/** 利用者が閉じた通知（同じ識別子）は出さない。 */
export function hideClosed(n: SendNotice | null, closedKey: string | undefined): SendNotice | null {
  return n && closedKey === n.key ? null : n;
}

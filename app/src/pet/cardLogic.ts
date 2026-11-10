// ペットのカード（簡易チャット・承認/質問への回答）の純粋ロジック。Node で直接テストできるよう、型だけに依存する。
import type { Chat, DecisionOption, PendingRequest, QuestionAnswer, TurnRecord } from "../ipc/types";

/** カードが開いてから、回答ボタンを押せるようになるまで（誤タップ防止）。 */
export const ARM_MS = 500;
export const isArmed = (shownAt: number | null, now: number): boolean => shownAt !== null && now - shownAt >= ARM_MS;

/**
 * 簡易チャットの送り先。利用者が選んだもの ＞ 最後に通知したチャット ＞ 通常画面で選択中のチャット。
 * 一覧にないチャット（削除・まだ読み込んでいない）は選ばない。
 */
export function chooseTarget(o: { picked: string | null; notified: string | null; selected: string | null }, knownIds: ReadonlySet<string>): string | null {
  for (const id of [o.picked, o.notified, o.selected]) if (id && knownIds.has(id)) return id;
  return null;
}

/** 送り先の候補（最近使った順、アーカイブ除く、最大 limit 件）。いまの送り先は必ず含める。 */
export function recentChats(chats: Chat[], targetId: string | null, limit = 8): Chat[] {
  const live = chats.filter((c) => !(c.archived.kind === "value" && c.archived.value));
  const sorted = [...live].sort((a, b) => (b.lastUsedAt ?? 0) - (a.lastUsedAt ?? 0));
  const top = sorted.slice(0, limit);
  const t = targetId ? chats.find((c) => c.key.id === targetId) : undefined;
  return t && !top.includes(t) ? [t, ...top.slice(0, limit - 1)] : top;
}

/** 回答待ちの要求（古い順）。 */
export const pendingRequests = (reqs: PendingRequest[]): PendingRequest[] =>
  reqs.filter((r) => r.state.kind === "pending").sort((a, b) => a.receivedAt - b.receivedAt);

export type OptionTone = "primary" | "careful" | "quiet";
/**
 * 選択肢の見た目。意味は変えない（ラベル・順序・個数は要求が示したまま）。
 * 今回だけの許可は主ボタン、会話・恒久の許可（効力が広い）は控えめな警告色、拒否・中止・その他は枠線。
 */
export function optionTone(o: DecisionOption): OptionTone {
  if (o.effect === "allow") return o.scope === "session" || o.scope === "persistent" ? "careful" : "primary";
  return "quiet";
}

/** 質問1件が1回の操作で確定できるか（選択肢だけで自由入力なし・質問が1件のとき）。 */
export const answersAtOnce = (questions: Array<{ options: DecisionOption[]; allowsFreeText: boolean }>): boolean =>
  questions.length === 1 && questions[0].options.length > 0 && !questions[0].allowsFreeText;

/** すべての質問に、選択肢か空でない入力がある。 */
export function answersComplete(questions: Array<{ id: string }>, picked: Record<string, QuestionAnswer | undefined>): boolean {
  return questions.length > 0 && questions.every((q) => {
    const a = picked[q.id];
    return !!a && (a.optionId !== null || (a.text ?? "").trim() !== "");
  });
}

/** 送り先チャットの最新の返答（エージェントの発言で本文があるもの）。画面が受け取った分だけ（追加の取得はしない）。 */
export function latestReply(turns: TurnRecord[] | undefined): { text: string; inProgress: boolean } | null {
  if (!turns) return null;
  for (let i = turns.length - 1; i >= 0; i--) {
    const es = turns[i].entries;
    for (let j = es.length - 1; j >= 0; j--) {
      const e = es[j];
      if (e.kind.kind !== "agentMessage" || e.text.kind !== "value" || e.text.value.trim() === "") continue;
      return { text: e.text.value, inProgress: e.phase === "inProgress" || e.phase === "started" };
    }
  }
  return null;
}

/** 長い文を max 文字（コードポイント）で切り詰める。切ったら「…」を付ける。 */
export function clip(text: string, max: number): string {
  const cs = Array.from(text.trim());
  return cs.length <= max ? cs.join("") : `${cs.slice(0, max).join("")}…`;
}

/** 通知一時停止の表示（残り分）。停止中でなければ null。 */
export function pauseLeftText(until: number | null, now: number): string | null {
  if (until === null || until <= now) return null;
  const min = Math.ceil((until - now) / 60_000);
  return min >= 60 ? `あと${Math.floor(min / 60)}時間${min % 60 ? `${min % 60}分` : ""}` : `あと${min}分`;
}

// 組織画面の「いまの動き」（役割ごとの現在の活動を1タスク1行。DESIGN_ORG_R2 1.3）。状態の判定はホスト。ここは並べ・文言・保持の純関数。
// 鮮度（接続断など）は状態の文字とは別の印で示す。取れない値は「未取得」で、空や完了では代用しない。
import type { ChatKey, OrgTaskView, OrgWorkflowView } from "../ipc/types";

/** あるチャットの今の活動（ルートエージェントの `currentActivity` と鮮度から導いたもの）。 */
export interface ChatActivity {
  /** 接続が live で、最新の状態か。 */
  live: boolean;
  /** 活動の種類（Activity.kind.kind）。取れていなければ null。 */
  kind: string | null;
  /** 短い表示本文。取れていなければ null（空文字は「ない」ではなく「未取得」と区別する）。 */
  summary: string | null;
}

export type ActivityGroup = "wait" | "work" | "queue";

export interface ActivityLine {
  task: string;
  roleLabel: string;
  title: string;
  group: ActivityGroup;
  /** 状態の文字（ホストの label）。鮮度では変えない。 */
  state: string;
  /** 3列目: 活動・待機理由。 */
  detail: string;
  /** 今の業務状態に入った時刻（ミリ秒）。辿れないときは null。 */
  since: number | null;
  /** 「最新ではありません」の印を付けるか（作業中のチャットが live でない）。 */
  stale: boolean;
  chat: ChatKey | null;
  /** この文を最後に替えた時刻（保持の判定用）。 */
  detailAt: number;
}

export interface ActivityResult {
  lines: ActivityLine[];
  /** 完了したタスクの件数（行は出さない）。 */
  done: number;
}

/** 活動を一行にする。取れない本文は「（内容は未取得）」を添える。 */
export function activityText(a: ChatActivity | null): string {
  if (!a || a.kind === null) return "（活動は未取得）";
  const s = a.summary && a.summary.trim() !== "" ? a.summary.replace(/\s+/g, " ").trim() : null;
  const with_ = (head: string) => (s ? `${head}: ${s}` : `${head}（内容は未取得）`);
  switch (a.kind) {
    case "command": return with_("コマンド");
    case "fileChange": return with_("ファイル変更");
    case "toolCall": return with_("ツール");
    case "webSearch": return with_("検索");
    case "reasoning": return "考えています";
    case "agentMessage": return "回答を書いています";
    case "plan": return "計画を立てています";
    case "subAgent": return "子エージェントの作業";
    default: return "作業中";
  }
}

/** 経過時間（分単位）。「1分未満」「12分」「2時間5分」。 */
export function elapsedText(ms: number): string {
  const min = Math.floor(Math.max(0, ms) / 60_000);
  if (min < 1) return "1分未満";
  if (min < 60) return `${min}分`;
  const h = Math.floor(min / 60);
  const m = min % 60;
  return m === 0 ? `${h}時間` : `${h}時間${m}分`;
}

export interface ActivityCtx {
  now: number;
  /** あなたの対応待ちの理由の文（対応待ちでなければ null）。承認・質問・上限・失敗・手動・判断など。 */
  waitOf: (t: OrgTaskView) => string | null;
  activityOf: (chat: ChatKey) => ChatActivity | null;
  roleLabel: (role: string) => string;
}

const FINISHED = new Set(["completed", "failed", "cancelled"]);

/** 並び: あなたの対応待ち → 作業中 → 順番待ち。完了は件数だけ。失敗・取消しは終わったものとして行にしない。 */
export function activityLines(wf: OrgWorkflowView, ctx: ActivityCtx): ActivityResult {
  const lines: ActivityLine[] = [];
  let done = 0;
  for (const t of wf.tasks) {
    const phase = t.status.phase;
    if (phase === "completed") { done += 1; continue; }
    if (FINISHED.has(phase)) continue;
    const wait = ctx.waitOf(t);
    const working = phase === "running" || phase === "reviewing" || phase === "rework";
    let group: ActivityGroup;
    let detail: string;
    let stale = false;
    if (wait !== null || phase === "blocked") {
      group = "wait";
      detail = wait ?? t.attentionNote ?? t.status.label;
    } else if (working) {
      group = "work";
      const a = t.chat ? ctx.activityOf(t.chat) : null;
      detail = phase === "reviewing" ? "検証待ち（結果を確認中）" : phase === "rework" ? "差し戻し（次の送信を待っています）" : activityText(a);
      stale = phase === "running" && a !== null && !a.live;
    } else {
      group = "queue";
      detail = t.held ? "保留中" : t.waitingOn.length > 0 ? `${t.waitingOn.join(",")} の完了待ち` : phase === "ready" ? "実行待ち" : "待機中";
    }
    lines.push({
      task: t.id, roleLabel: ctx.roleLabel(t.role), title: t.title, group, state: t.status.label, detail,
      since: t.since, stale, chat: t.chat, detailAt: ctx.now,
    });
  }
  const order: Record<ActivityGroup, number> = { wait: 0, work: 1, queue: 2 };
  lines.sort((a, b) => order[a.group] - order[b.group]);
  return { lines, done };
}

/** 活動文を最低 `HOLD_MS` は替えない（チカチカ防止）。状態（業務状態の文字）が変わったときは即座に替える。 */
export const HOLD_MS = 3000;
export function stabilize(prev: ActivityLine[], next: ActivityLine[], now: number): ActivityLine[] {
  const before = new Map(prev.map((l) => [l.task, l]));
  return next.map((n) => {
    const p = before.get(n.task);
    if (!p || p.state !== n.state || p.group !== n.group) return { ...n, detailAt: now };
    if (p.detail === n.detail) return { ...n, detailAt: p.detailAt };
    if (now - p.detailAt < HOLD_MS) return { ...n, detail: p.detail, detailAt: p.detailAt };
    return { ...n, detailAt: now };
  });
}

/** 1行の文（title に全文を出す用）。 */
export function lineText(l: ActivityLine, now: number): string {
  const parts = [`${l.roleLabel}・${l.task}『${l.title}』`, l.state, l.detail];
  if (l.since !== null) parts.push(elapsedText(now - l.since));
  if (l.stale) parts.push("最新ではありません（接続断等）");
  return parts.join("｜");
}

// 入力欄の近くに常に出す「いま何を待っているか」の段階表示（純粋ロジック。画面は SendStatusBar.tsx）。
// 送信した → 受理待ち → 応答待ち → 処理中、と、キュー（登録した・送っている・受理不明）を区別する。
// ホストの状態を写すだけで新しい状態機械は持たない。無反応が続いても表示は消さず、通信断・不明を完了・失敗にしない。
// 実行: node --test src/ui/sendStatus.test.ts
import type { SendPhase } from "../ipc/types";
import type { PendingSend } from "./pendingSend";

export type StatusTone = "busy" | "ok" | "warn";

export interface StatusLine {
  /** 段階の識別子（経過時間の起点・読み上げの更新判定に使う）。 */
  id: string;
  tone: StatusTone;
  text: string;
  /** 経過を数える起点（epoch ms）。null なら、この段階を表示し始めた時刻から数える。 */
  since: number | null;
  /** スピナーを回すか（待っている段階）。 */
  spin: boolean;
}

export interface QueueCounts { waiting: number; sending: number; unknown: number }

export interface StatusInput {
  pending: PendingSend[];
  running: boolean;
  /** 実行中のturnに、依頼以外の出力（返答・作業の記録）が出ているか。取れないとき（ドック）は null。 */
  turnHasOutput: boolean | null;
  queue: QueueCounts | null;
}

export function queueCounts(entries: ReadonlyArray<{ state: { kind: string } }> | undefined): QueueCounts | null {
  if (!entries) return null;
  const c: QueueCounts = { waiting: 0, sending: 0, unknown: 0 };
  for (const e of entries) {
    if (e.state.kind === "waiting") c.waiting++;
    else if (e.state.kind === "sending") c.sending++;
    else if (e.state.kind === "acceptanceUnknown") c.unknown++;
  }
  return c.waiting + c.sending + c.unknown > 0 ? c : null;
}

function sendText(steer: boolean, phase: SendPhase | null): string {
  const what = steer ? "追加指示を送信しました" : "送信しました";
  switch (phase) {
    case "preparing": return `${what}（送信の準備をしています）`;
    case "resuming": return `${what}（会話を再開しています）`;
    case "awaitingAcceptance": return `受理待ち（Codex が${steer ? "追加指示を" : ""}受理するのを待っています）`;
    default: return `${what}（画面は入力を受け付けました。受理待ち）`;
  }
}

/** 表示する段階の一覧（上から重要な順。無ければ空）。 */
export function sendStatusLines(i: StatusInput): StatusLine[] {
  const out: StatusLine[] = [];
  for (const p of i.pending) {
    const o = p.outcome;
    if (o === null) {
      out.push({ id: `send:${p.id}`, tone: "busy", text: sendText(p.steer, p.phase), since: p.startedAt, spin: true });
    } else if (o.kind === "unknown") {
      out.push({ id: `unknown:${p.id}`, tone: "warn", text: "受理を確認できません（再送していません。履歴との照合で確定します）", since: null, spin: false });
    } else if (o.kind === "accepted") {
      if (p.steer) out.push({ id: `acc:${p.id}`, tone: "ok", text: "追加指示を Codex が受理しました（反映は会話の表示で確認してください）", since: null, spin: false });
      else if (!i.running) out.push({ id: `acc:${p.id}`, tone: "busy", text: "受理されました（処理の開始を待っています）", since: null, spin: true });
    }
    // rejected・failed は会話内の「未送信」の行が理由と操作を持つ。ここには出さない。
  }
  if (i.running) {
    if (i.turnHasOutput === false) out.push({ id: "wait-first", tone: "busy", text: "応答待ち（Codex からの最初の返答を待っています）", since: null, spin: true });
    else out.push({ id: "working", tone: "busy", text: "処理中（Codex が作業しています）", since: null, spin: true });
  }
  const q = i.queue;
  if (q) {
    if (q.unknown > 0) out.push({ id: "queue-unknown", tone: "warn", text: `キューの依頼 ${q.unknown}件の受理を確認できません（再送していません。履歴と照合してください）`, since: null, spin: false });
    if (q.sending > 0) out.push({ id: "queue-sending", tone: "busy", text: `キューの依頼 ${q.sending}件を送信しました（受理待ち）`, since: null, spin: true });
    if (q.waiting > 0) out.push({ id: "queue-waiting", tone: "ok", text: `キューに入っています: ${q.waiting}件（完了後に1件ずつ送ります）`, since: null, spin: false });
  }
  return out;
}

/** 長く待っているときの補足。表示は消さず、完了・失敗とは判断していないことを添える。 */
export function slowNote(id: string, elapsedMs: number): string | null {
  if (id.startsWith("send:") && elapsedMs >= 10_000) return "受理の返事が遅れています。再送はしていません";
  if (id === "wait-first" && elapsedMs >= 30_000) return "まだ返答がありません。完了・失敗とは判断していません（このまま待てます）";
  if (id === "queue-sending" && elapsedMs >= 10_000) return "受理の返事が遅れています。再送はしていません";
  return null;
}

export const formatElapsed = (ms: number): string => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}秒` : `${Math.floor(s / 60)}分${String(s % 60).padStart(2, "0")}秒`;
};

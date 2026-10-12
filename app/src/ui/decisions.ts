// 応答した承認・質問・MCP許可の記録（項目51）の、会話への差し込みと文言。純粋ロジック。
// 記録はホストが `decisions.jsonl` に保存したものだけ。Codexの履歴には載らない種類なので、無いものを「記録なし」とは言わない。
import type { DecisionKind, DecisionOutcome, DecisionRecord, DecisionSave } from "../ipc/types";

/** 画面が持つ記録1件。`save` が失敗なら再起動後には残らない（保存できていないことを表示する）。 */
export interface DecisionItem { record: DecisionRecord; save: DecisionSave }

const SAVED: DecisionSave = { kind: "saved" };

/** 読み込んだ記録（保存済み）と、受け取った通知（保存結果つき）を、記録ID単位で合わせて時刻順にする。通知の保存結果を優先する。 */
export function mergeDecisions(loaded: DecisionRecord[], live: DecisionItem[]): DecisionItem[] {
  const m = new Map<string, DecisionItem>();
  for (const r of loaded) m.set(r.id, { record: r, save: SAVED });
  for (const it of live) m.set(it.record.id, it);
  return [...m.values()].sort((a, b) => a.record.at - b.record.at || (a.record.id < b.record.id ? -1 : a.record.id > b.record.id ? 1 : 0));
}

/** 通知1件を既存の一覧へ加える（同じIDは置き換え）。 */
export function addDecision(list: DecisionItem[], it: DecisionItem): DecisionItem[] {
  return mergeDecisions([], [...list, it]);
}

export const slotKey = (agentId: string, turnId: string): string => `${agentId}\u0000${turnId}`;

interface TurnLike { key: { agent: { id: string }; turnId: string }; startedAt: { kind: string; value?: unknown } }

/**
 * 記録を、そのturnの末尾（本文・作業の記録の後ろ、まだ出ている承認カードの前）へ差し込む。
 * turnを特定できない記録（turn不明・履歴にまだ無い）は、時刻が開始より後の最後のturnの末尾へ。どのturnより前なら先頭（head）。
 * turn内の並びは時刻順。履歴の各項目は時刻を持たないので、turnの中での前後は決めない。
 */
export function placeDecisions(turns: TurnLike[], items: DecisionItem[]): { head: DecisionItem[]; byTurn: Map<string, DecisionItem[]> } {
  const head: DecisionItem[] = [];
  const byTurn = new Map<string, DecisionItem[]>();
  const known = new Set(turns.map((t) => slotKey(t.key.agent.id, t.key.turnId)));
  const starts = turns.map((t) => ({ slot: slotKey(t.key.agent.id, t.key.turnId), at: t.startedAt.kind === "value" && typeof t.startedAt.value === "number" ? t.startedAt.value : null }));
  for (const it of items) {
    const r = it.record;
    let slot: string | null = r.turn !== null && known.has(slotKey(r.agent.id, r.turn)) ? slotKey(r.agent.id, r.turn) : null;
    if (slot === null) {
      let best: { slot: string; at: number } | null = null;
      for (const s of starts) if (s.at !== null && s.at <= r.at && (best === null || s.at >= best.at)) best = { slot: s.slot, at: s.at };
      slot = best?.slot ?? null;
    }
    if (slot === null) { head.push(it); continue; }
    const l = byTurn.get(slot);
    if (l) l.push(it); else byTurn.set(slot, [it]);
  }
  return { head, byTurn };
}

const KIND_LABEL: Record<DecisionKind, string> = {
  command: "承認: コマンド", fileChange: "承認: ファイル変更", permissions: "承認: 追加権限", question: "質問", toolPermission: "MCP許可", other: "要求",
};
const SCOPE_LABEL = { once: "今回のみ", session: "このセッション中", persistent: "以後ずっと", unknown: "" } as const;

export function outcomeText(o: DecisionOutcome): string {
  switch (o.kind) {
    case "allowed": return SCOPE_LABEL[o.scope] ? `許可（${SCOPE_LABEL[o.scope]}）` : "許可";
    case "declined": return "辞退";
    case "cancelled": return "取りやめ";
    case "answered": return "回答";
    case "chosen": return `選択: ${o.label}`;
    case "unconfirmed": return "応答を送信（受理を確認できません）";
    case "resolvedElsewhere": return "別の場所で解決（AgentDockでは応答していません）";
  }
}

/** 会話に出す1行。要求元は、現在の名前（`who`）を優先し、無ければ記録時の名前、それも無ければ出さない。 */
export function decisionLine(it: DecisionItem, who: string | null): { text: string; failed: string | null } {
  const r = it.record;
  const name = who ?? r.agentName ?? null;
  const from = name ? `［${name}］` : "";
  return {
    text: `${KIND_LABEL[r.kind]}${r.summary ? ` ${r.summary}` : ""} → ${outcomeText(r.outcome)}${from}`,
    failed: it.save.kind === "failed" ? it.save.message : null,
  };
}

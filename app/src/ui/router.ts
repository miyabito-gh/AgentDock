// 受付係（DESIGN_ROUTER §8）の表示用の純関数。判定の正本はホスト（rules/router.rs）で、ここは文字の切り出しと文言だけ。
// 実行: node --test src/ui/router.test.ts （tscの対象外。実行時の import を持たない）
import type { Chat, ChatKey, ChatLocalView, DispatchView, RouterView, RuleNoticeView } from "../ipc/types";

/** 受付係への送信の資料の見出し・終端（ホスト rules/router.rs の ENVELOPE_HEAD / ENVELOPE_END と同じ文字列）。 */
export const ROUTER_ENVELOPE_HEAD = "[AgentDock 受付係の資料]";
export const ROUTER_ENVELOPE_END = "[ここまで資料。以下が利用者の依頼]";

export interface RouterSplit {
  /** 資料の全体（見出し行から終端行まで）。資料がなければ null。 */
  envelope: string | null;
  /** 終端の後ろ（利用者の依頼）。資料がなければ本文そのまま。 */
  userText: string;
}

/** 本文が資料の見出しで始まり、終端があるときだけ分ける（ホストの取り出しと同じ。最初の終端で切り、直後の改行を1つ除く）。 */
export function splitRouterEnvelope(text: string): RouterSplit {
  if (!text.startsWith(ROUTER_ENVELOPE_HEAD)) return { envelope: null, userText: text };
  const i = text.indexOf(ROUTER_ENVELOPE_END);
  if (i < 0) return { envelope: null, userText: text };
  const end = i + ROUTER_ENVELOPE_END.length;
  const rest = text.slice(end);
  const user = rest.startsWith("\r\n") ? rest.slice(2) : rest.startsWith("\n") ? rest.slice(1) : rest;
  return { envelope: text.slice(0, end), userText: user };
}

/** 折りたたみ行の文字（「資料（1,234字）」）。 */
export const routerEnvelopeLabel = (envelope: string): string => `資料（${[...envelope].length.toLocaleString("en-US")}字）`;

/** 受付係の返答（turn）の解析結果。 */
export const turnViewOf = (v: RouterView | null, turnId: string) => v?.turns.find((t) => t.turn === turnId) ?? null;

/** その返答から出た振り分けカード（並びは routeIndex）。 */
export const dispatchesOfTurn = (v: RouterView | null, turnId: string): DispatchView[] =>
  (v?.dispatches ?? []).filter((d) => d.sourceTurn === turnId).sort((a, b) => a.routeIndex - b.routeIndex);

/** 送り先の見出し。既存は今の名前（取れなければ中立の語）、新規は「新しいチャット」＋案の題名・フォルダ。 */
export function targetLabel(d: DispatchView): string {
  if (d.target.kind === "existing") return d.targetTitle ?? "送り先の会話";
  const n = d.proposal?.newChat;
  const parts = [n?.title?.trim(), n?.folder?.trim()].filter((x): x is string => !!x);
  return parts.length ? `新しいチャット：${parts.join("（")}${parts.length > 1 ? "）" : ""}` : "新しいチャット";
}

/** 送った後の1行（たたんだ表示）。まだ送っていない（proposed）は null。 */
export function foldedLine(d: DispatchView): { text: string; kind: "ok" | "warn" | "quiet"; canOpen: boolean } | null {
  const name = d.targetTitle ?? (d.target.kind === "new" ? "新しいチャット" : "送り先の会話");
  const canOpen = d.target.kind === "existing" || d.target.chat !== null;
  switch (d.state.kind) {
    case "proposed": return null;
    case "sent": return { text: `→『${name}』へ送りました`, kind: "ok", canOpen };
    case "queued": return { text: `→『${name}』の順番待ちに入れました`, kind: "ok", canOpen };
    case "unconfirmed": return { text: "送れたか確認できません", kind: "warn", canOpen };
    case "dismissed": return { text: "見送りました", kind: "quiet", canOpen: false };
  }
}

/** カードで送る本文。整えた文と原文の切り替えで、編集が無ければ選んだほうをそのまま使う。 */
export function baseText(d: DispatchView, useOriginal: boolean, original: string | null): string {
  const p = d.proposal;
  if (useOriginal && original !== null) return original;
  return p?.text ?? original ?? "";
}

/** 整えた文と原文の切り替えを出せるか（案の本文が原文と違い、原文を取り出せるとき）。 */
export const canToggleOriginal = (d: DispatchView, original: string | null): boolean => !!d.proposal && !d.proposal.textIsOriginal && !!original?.trim();

/** ルール更新の1行。書かなかった（revision なし）は取り消せない静かな行。 */
export function noticeView(n: RuleNoticeView): { text: string; canUndo: boolean; undone: boolean } {
  const undone = n.revision !== null && n.undone;
  return { text: undone ? `${n.text}（取り消し済み）` : n.text, canUndo: n.revision !== null && !n.undone, undone };
}

/** 振り分け先の候補（アーカイブ・組織所有・引き継ぎ済み・受付係自身を除く）。query は名前・作業フォルダの部分一致。最終の判定はホスト。 */
export function pickableChats(chats: Chat[], localOf: (c: Chat) => ChatLocalView | undefined, router: ChatKey | null, title: (c: Chat) => string, folder: (c: Chat) => string | null, query: string): Chat[] {
  const q = query.trim().toLocaleLowerCase();
  return chats.filter((c) => {
    const l = localOf(c);
    if (router && c.key.id === router.id) return false;
    if (l?.visibility.kind === "archived" || l?.visibility.kind === "removedFromList" || l?.supersededBy) return false;
    if (c.archived.kind === "value" && c.archived.value) return false;
    if (l?.org && !l.org.released) return false;
    if (!q) return true;
    return title(c).toLocaleLowerCase().includes(q) || (folder(c) ?? "").replace(/\\/g, "/").toLocaleLowerCase().includes(q.replace(/\\/g, "/"));
  });
}

/** その turn の利用者の依頼（資料の終端の後ろ）。最初の利用者発話から取る。無ければ null。 */
export function originalRequest(entries: { kind: { kind: string }; text: { kind: string; value?: string } }[]): string | null {
  for (const e of entries) {
    if (e.kind.kind !== "userMessage" || e.text.kind !== "value") continue;
    return splitRouterEnvelope(e.text.value ?? "").userText;
  }
  return null;
}

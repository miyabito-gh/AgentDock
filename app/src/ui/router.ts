// 受付係（DESIGN_ROUTER §8）の表示用の純関数。判定の正本はホスト（rules/router.rs）で、ここは文字の切り出しと文言だけ。
// 実行: node --test src/ui/router.test.ts （tscの対象外。実行時の import を持たない）
import type { BackendKind, Chat, ChatKey, ChatLocalView, DispatchView, RouterView, RuleNoticeView } from "../ipc/types";

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

/** 振り分けカードの状態の印（色だけに頼らず、形＋文字で示す。tone は `.st.<tone>` の語彙）。 */
export interface DispatchChip { tone: "run" | "wait" | "fail" | "int" | "done" | "idle" | "unk"; icon: string; label: string }

/**
 * 担当の結果文（ホスト host/router.rs の `execution_result`）から状態の印を引く。文の先頭の語で判定する
 * （ホスト側の文言を変えるときはここも合わせる）。知らない文は印なし（本文はそのまま表示する）。
 */
export function resultChip(result: string | null): DispatchChip | null {
  if (!result) return null;
  const has = (p: string) => result.startsWith(p);
  if (has("担当完了")) return { tone: "done", icon: "check", label: "完了" };
  if (has("担当の実行が失敗")) return { tone: "fail", icon: "x", label: "失敗" };
  if (has("送信失敗")) return { tone: "fail", icon: "x", label: "送信失敗" };
  if (has("担当の実行が中断")) return { tone: "int", icon: "pause", label: "中断" };
  if (has("担当への受理を照合中")) return { tone: "unk", icon: "help", label: "受理不明" };
  if (has("担当が受理しました")) return { tone: "run", icon: "hourglass", label: "完了を照合中" };
  if (has("担当へ送信中")) return { tone: "run", icon: "hourglass", label: "送信中" };
  if (has("キューは停止中")) return { tone: "wait", icon: "alert", label: "要確認" };
  if (has("キュー記録を照合できません")) return { tone: "unk", icon: "help", label: "照合不能" };
  if (has("キューで取り消")) return { tone: "idle", icon: "ban", label: "取り消し" };
  return null;
}

/**
 * 送った後の1行に添える印。担当の結果が分かればそれ、なければ送信の状態（送信待ち・受理不明・送信済み）。見送りは印なし。
 * 結果文があるのに先頭の語を判定できないとき（ホストの文言が変わった・古い記録）は、送信の状態に戻らず「状態不明」にする
 * （完了しているのに「送信待ち」と見せない。本文は常に表示する）。
 */
export function dispatchChip(d: DispatchView): DispatchChip | null {
  const r = resultChip(d.executionResult);
  if (r) return r;
  if (d.executionResult) return { tone: "unk", icon: "help", label: "状態不明" };
  switch (d.state.kind) {
    case "queued": return { tone: "idle", icon: "hourglass", label: "送信待ち" };
    case "unconfirmed": return { tone: "unk", icon: "help", label: "受理不明" };
    case "sent": return { tone: "run", icon: "play", label: "送信済み" };
    default: return null;
  }
}

/** 結果の本文。先頭行が印と同じ語だけ（「担当完了」）なら、その行は省いて最終回答だけを出す。 */
export function resultBody(result: string): string {
  const i = result.indexOf("\n");
  return i > 0 && result.slice(0, i) === "担当完了" ? result.slice(i + 1) : result;
}

/** 印が「結果を確かめる必要がある」側（失敗・受理不明・要確認）か。カードの枠を警告色にする。 */
export const chipNeedsAttention = (c: DispatchChip | null): boolean => c?.tone === "fail" || c?.tone === "unk" || c?.tone === "wait";

/** 担当AIの表示名（カードの印）。BackendKind を足したらここも足す（型で漏れを検出する）。 */
const PROVIDER_LABEL: Record<BackendKind, string> = { codex: "Codex", antigravity: "Antigravity（Gemini）" };
export const providerLabel = (b: BackendKind): string => PROVIDER_LABEL[b];

/** 提案カードの担当AI。新規は案の provider（省略は Codex）、既存の送り先はそのチャットのキーの backend。分からなければ null。 */
export function proposedProvider(d: DispatchView): BackendKind | null {
  if (d.target.kind === "new" && d.target.chat === null) return d.proposal?.newChat?.provider ?? "codex";
  return d.target.chat?.backend ?? null;
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

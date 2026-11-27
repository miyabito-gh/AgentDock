// 受付係（DESIGN_ROUTER §8）の表示用の純関数。判定の正本はホスト（rules/router.rs）で、ここは文字の切り出しと文言だけ。
// 実行: node --test src/ui/router.test.ts （tscの対象外。実行時の import を持たない）
import type { BackendKind, Chat, ChatKey, ChatLocalView, DispatchView, ExecutionStatus, RouterView, RuleNoticeView } from "../ipc/types";

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
 * 担当の結果の状態（ホストが値で返す `executionStatus`）から状態の印を引く。結果文の文言は見ない
 * （ホストが文言を変えても印は変わらない）。BackendKind と同様、状態を足したらここも足す（型で漏れを検出する）。
 */
const STATUS_CHIP: Record<ExecutionStatus, DispatchChip> = {
  done: { tone: "done", icon: "check", label: "完了" },
  failed: { tone: "fail", icon: "x", label: "失敗" },
  sendFailed: { tone: "fail", icon: "x", label: "送信失敗" },
  enqueueFailed: { tone: "fail", icon: "x", label: "キュー登録失敗" },
  interrupted: { tone: "int", icon: "pause", label: "中断" },
  unconfirmed: { tone: "unk", icon: "help", label: "受理不明" },
  accepted: { tone: "run", icon: "hourglass", label: "完了を照合中" },
  sending: { tone: "run", icon: "hourglass", label: "送信中" },
  queuePaused: { tone: "wait", icon: "alert", label: "要確認" },
  queueMissing: { tone: "unk", icon: "help", label: "照合不能" },
  cancelled: { tone: "idle", icon: "ban", label: "取り消し" },
  // 送り先のキューは動いているが、保留理由があって送られていない（理由の文はホストが executionResult で返す）。
  queueHeld: { tone: "wait", icon: "alert", label: "保留中" },
  // この版が知らない状態（将来の版が値を足した記録）。完了などと見せず「状態不明」にする。
  unrecognized: { tone: "unk", icon: "help", label: "状態不明" },
};
export const statusChip = (s: ExecutionStatus | null | undefined): DispatchChip | null => (s ? STATUS_CHIP[s] : null);

/**
 * 送った後の1行に添える印。担当の結果の状態が分かればそれ、なければ送信の状態（送信待ち・受理不明・送信済み）。見送りは印なし。
 * 結果文はあるが状態がないとき（ホストが読み替えられなかった古い記録）は、送信の状態に戻らず「状態不明」にする
 * （完了しているのに「送信待ち」と見せない。本文は常に表示する）。
 */
export function dispatchChip(d: DispatchView): DispatchChip | null {
  const r = statusChip(d.executionStatus);
  if (r) return r;
  if (d.executionResult) return { tone: "unk", icon: "help", label: "状態不明" };
  switch (d.state.kind) {
    case "queued": return { tone: "idle", icon: "hourglass", label: "送信待ち" };
    case "unconfirmed": return { tone: "unk", icon: "help", label: "受理不明" };
    case "sent": return { tone: "run", icon: "play", label: "送信済み" };
    default: return null;
  }
}

/** 印が「結果を確かめる必要がある」側（失敗・受理不明・要確認）か。カードの枠を警告色にする。 */
export const chipNeedsAttention = (c: DispatchChip | null): boolean => c?.tone === "fail" || c?.tone === "unk" || c?.tone === "wait";

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
    if (c.key.backend !== "codex") return false;
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

/**
 * 受付の返答に添える「出さなかった理由」の1行。ホストの route_rejects には、振り分け案を出せなかった理由と、
 * 自動送信を見送った理由（「自動送信…」で始まる）が入る。後者を「振り分け案を出さなかった理由」と見せない（案は出ており、送られていないだけ）。
 */
export function routeRejectLine(r: string): { text: string; auto: boolean } {
  const auto = r.startsWith("自動送信");
  return { text: auto ? r : `振り分け案を出さなかった理由: ${r}`, auto };
}

// ───────────────────────────── Codex が起動していないとき ─────────────────────────────
// ホストの「接続しているか」の判定は Codex が基準。Codex が起動していない（または接続が切れている）と、
// 順番待ちの送信・受付係への送信は進まない。無言の停滞にしないため、止まっている理由を画面に出す（判定そのものは変えない）。

/** 接続の状態の最小形（SourceInfo の一部）。 */
interface SourceLike { connection: { kind: string } }

/**
 * Codex が起動していない・接続が切れている理由の種類。接続済み・接続中（起動の途中）・まだ状態が分からないときは null
 * （それらは別の表示「接続中…」があり、ここで止まっているとは言わない）。
 */
export function codexStoppedKind(sources: readonly SourceLike[]): "notStarted" | "launchFailed" | "disconnected" | null {
  const s = sources[0];
  if (!s || sources.some((x) => x.connection.kind === "connected")) return null;
  const k = s.connection.kind;
  return k === "notStarted" || k === "launchFailed" || k === "disconnected" ? k : null;
}

/** 受付係の画面の上に出す、カードと順番待ちが進まない理由（Codex が止まっているときだけ）。 */
export function routerStoppedNotice(sources: readonly SourceLike[]): string | null {
  return codexStoppedKind(sources)
    ? "Codex が起動していない（または接続が切れている）ため、振り分けカードの［送る］や順番待ちの送信は進みません。Codex に接続すると動きます。"
    : null;
}

/** 受付係（Codex のチャット）への送信を止める理由。Codex が止まっているときだけ。 */
export function routerSendLock(sources: readonly SourceLike[]): string | null {
  return codexStoppedKind(sources)
    ? "Codex が起動していない（または接続が切れている）ため、受付係へは送れません。Codex に接続してから送ってください。"
    : null;
}

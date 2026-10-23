// 左一覧の入れ子・題名の整形（純粋関数。表示だけで、保存名・状態は変えない）。U-19。
import type { Chat, ChatLocalView, OrgWorkflowView } from "../ipc/types";
import { displayWorkflowTitle } from "./orgText.ts";
// node --test で単体実行できるよう、format/derive には依存しない（同等の最小処理を持つ）。
const knownValue = <T>(k: { kind: string; value?: T } | undefined): T | undefined => (k && k.kind === "value" ? k.value : undefined);
const previewTitle = (c: Chat): string | null => {
  const p = (knownValue<string>(c.preview as never) ?? "").replace(/\s+/g, " ").trim();
  if (!p) return null;
  const a = Array.from(p);
  return a.length > 30 ? `${a.slice(0, 30).join("")}…` : p;
};

/** 資料の見出し『［AgentDock資料 v1｜依頼整理］』（先頭のみ）。 */
const DOC_HEADER = /^[［\[]\s*AgentDock資料[^］\]]*[］\]]\s*/;

/** 題名が資料の見出しで始まるか。 */
export const hasDocHeader = (name: string): boolean => DOC_HEADER.test(name.trim());

/** 資料の見出しを除いた本文（空白を畳む）。見出しが無ければそのまま。 */
export const stripDocHeader = (name: string): string => name.trim().replace(DOC_HEADER, "").replace(/\s+/g, " ").trim();

/** ワークフロー名の整形（先頭の『# 』を除く。『（…・第N版）』は残す）。担当Aの displayWorkflowTitle が公開されたらここを差し替える。 */
export const wfTitleText = (title: string): string => displayWorkflowTitle(title) ?? "";

/** ワークフローの表示名（title が無ければ依頼の先頭30字）。 */
export const wfDisplayName = (wf: Pick<OrgWorkflowView, "title" | "request">): string =>
  wfTitleText(wf.title ?? wf.request.slice(0, 30)) || "（無題）";

/** 『MM/DD HH:MM』（端末の時刻）。 */
export function shortStamp(ms: number): string {
  const d = new Date(ms), p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

const stampOf = (c: Chat): number | null => c.lastUsedAt ?? knownValue(c.createdAt) ?? null;

/** 先頭 n 字（超えたら…）。 */
export const clip = (s: string, n: number): string => { const a = Array.from(s); return a.length > n ? `${a.slice(0, n).join("")}…` : s; };

export type LeftClass =
  | { kind: "normal" }
  | { kind: "role"; workflowId: string }
  | { kind: "intake"; workflowId: string }
  | { kind: "intakeOpen" };

/**
 * 左一覧での置き場所。role＝組織の役割チャット（手動に切替前。ワークフローの下）、intake＝そのワークフローに紐づく依頼整理、
 * intakeOpen＝ワークフローに紐づかない依頼整理（未開始・取りやめ）。shown＝画面に出せるワークフローID（null は未取得）。
 */
export function classifyLeft(l: ChatLocalView | undefined, shown: ReadonlySet<string> | null, pinned = false): LeftClass {
  if (pinned) return { kind: "normal" }; // ピン留めは優先（ピン留め欄に出す。外せば畳みに戻る）
  if (l?.org && !l.org.released && (shown === null || shown.has(l.org.workflowId))) return { kind: "role", workflowId: l.org.workflowId };
  const it = l?.conversation?.intake;
  if (!it) return { kind: "normal" };
  if (it.kind === "started") return shown && shown.has(it.workflowId) ? { kind: "intake", workflowId: it.workflowId } : { kind: "normal" };
  return { kind: "intakeOpen" };
}

/**
 * 一覧に出す題名。依頼整理は見出し付きの名前をやめて『依頼整理（日時）』、通常チャットは資料見出しを除いた本文の先頭、
 * 名前も最初の依頼も無いものは『名前未確認・日時』（時刻が無ければ従来の『（名前未確認）』）。保存名は変えない。
 */
export function listTitle(c: Chat, l: ChatLocalView | undefined): { text: string; confirmed: boolean } {
  const name = knownValue(c.name);
  const t = stampOf(c);
  if (l?.conversation?.intake && (!name || hasDocHeader(name))) {
    return { text: t === null ? "依頼整理" : `依頼整理（${shortStamp(t)}）`, confirmed: true };
  }
  if (name) {
    if (!hasDocHeader(name)) return { text: name, confirmed: true };
    const body = stripDocHeader(name);
    return { text: body ? clip(body, 60) : t === null ? "（資料）" : `資料（${shortStamp(t)}）`, confirmed: true };
  }
  const p = previewTitle(c);
  if (p) return { text: hasDocHeader(p) ? clip(stripDocHeader(p) || p, 30) : p, confirmed: false };
  return { text: t === null ? "（名前未確認）" : `名前未確認・${shortStamp(t)}`, confirmed: false };
}

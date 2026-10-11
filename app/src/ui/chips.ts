// 発言の吹き出しに出す添付チップの純粋ロジック（DOMは扱わない）。
import type { AttachmentEntry, EntryAttachment } from "../ipc/types";

/** 履歴から復元できなかった添付の中立の表示（断定しない）。 */
export const UNDETERMINED_LABEL = "添付あり（詳細未確認）";
/** 吹き出しに並べるチップの最大数（超えた分は「＋N」）。 */
export const CHIP_MAX = 3;

export type ChipKind = "image" | "file" | "audio" | "skill" | "unknown";

export interface ChipModel {
  label: string;
  kind: ChipKind;
  /** 復元できた根拠が弱い（名前が分からない）。中立の見た目にする。 */
  neutral: boolean;
  path: string | null;
  /** ツールチップ用の全文。 */
  title: string;
}

const KIND_TEXT: Record<ChipKind, string> = { image: "画像", file: "ファイル", audio: "音声", skill: "Skill", unknown: "添付" };

/** チップの識別（再描画で同じ添付を同一視する。コピー先パスの正規化 + 表示名）。送信直後の記録と履歴から復元したものが同じキーになる。 */
export function chipKey(a: EntryAttachment, i: number): string {
  return a.path ? `${normPath(a.path)}` : `${a.name ?? "?"}#${i}`;
}

export function chipModel(a: EntryAttachment): ChipModel {
  const kind: ChipKind = a.kind ?? "unknown";
  if (!a.name) {
    return { label: UNDETERMINED_LABEL, kind, neutral: true, path: a.path, title: `${UNDETERMINED_LABEL}。履歴から名前を復元できませんでした${kind !== "unknown" ? `（種類: ${KIND_TEXT[kind]}）` : ""}` };
  }
  const basis = a.basis === "listedText" ? "本文の「添付ファイル:」から復元" : "履歴の入力項目から復元";
  return { label: a.name, kind, neutral: false, path: a.path, title: `${a.path ?? a.name}\n${KIND_TEXT[kind]}（${basis}）` };
}

/** 表示する分と「＋N」の残り。expanded なら全件。 */
export function splitChips<T>(all: T[], expanded: boolean, max: number = CHIP_MAX): { shown: T[]; more: number } {
  if (expanded || all.length <= max) return { shown: all, more: 0 };
  return { shown: all.slice(0, max), more: all.length - max };
}

/** パスの比較用の形（区切り・大文字小文字・\\?\ 接頭辞を無視）。 */
export function normPath(p: string): string {
  return p.replace(/^\\\\\?\\/, "").replace(/\\/g, "/").toLowerCase();
}

/** 履歴のパスに対応する、このチャットのローカルの添付レコード（コピー先のパスで照合。Skillは定義ファイルのパス。見つからなければ null）。 */
export function matchLocal(path: string | null, locals: AttachmentEntry[]): AttachmentEntry | null {
  if (!path) return null;
  const n = normPath(path);
  return locals.find((l) => (l.copyPath !== null && normPath(l.copyPath) === n) || (l.source.kind === "skill" && normPath(l.source.path) === n)) ?? null;
}

/** 送信時にこのアプリが持っていた添付（ローカルの台帳）を、チップ用の形にする。 */
export function fromLocal(a: AttachmentEntry): EntryAttachment {
  return { name: a.displayName, kind: a.kind, path: a.copyPath ?? (a.source.kind === "skill" ? a.source.path : null), basis: "structured" };
}

/**
 * 送った発言のローカルの記録（送信時の本文・時刻・添付）。履歴・live通知が添付を運ぶまでの間、吹き出しに添付を出すために使う。
 * 添付なしの送信も記録する（同じ本文の別の発言に、他の送信の添付が付かないよう、割当ての枠を消費させる）。受理が確認できた送信だけを記録する。
 */
export interface SentLocal { text: string; at: number; atts: EntryAttachment[]; /** 追加指示（実行中のturnへの送信）。turnの開始は送信より前なので、開始時刻の窓を使わない。 */ steer?: boolean }

interface EntryLike { key: { itemId: string }; kind: { kind: string }; text: { kind: string; value?: string }; attachments?: EntryAttachment[] }
interface TurnLike { startedAt: { kind: string; value?: number }; completedAt?: { kind: string; value?: number }; entries: EntryLike[] }

/** 送信時刻より前（時計のずれ）・後（実行の遅れ）に許す、turn開始時刻の幅。 */
export const SEND_BEFORE_MS = 5_000;
export const SEND_AFTER_MS = 10 * 60_000;

const num = (k: { kind: string; value?: number } | undefined): number | null => (k && k.kind === "value" && typeof k.value === "number" ? k.value : null);

/**
 * 送信の記録（古い順）を、発言（userMessage）へ1対1で割り当てる。本文が同じ未割当ての発言のうち、
 * 通常の送信は「turnの開始時刻が送信の前5秒〜後10分」の最初のもの、追加指示は「送信の前5秒より後まで終わっていないturn」の最新のものを選ぶ。
 * 履歴から復元した添付が既にある発言は、割り当てるだけで置き換えない（履歴を優先。同じ送信が別の発言に当たらないように消費する）。
 * 戻り値は「履歴に添付が無い発言のID → 送信時の添付（添付なしの送信は空）」。本文はlive通知で切り詰められることがあるので、短い側が前方一致すれば同じとみなす。
 */
export function assignSends(turns: TurnLike[], sends: SentLocal[]): Map<string, EntryAttachment[]> {
  const out = new Map<string, EntryAttachment[]>();
  const used = new Set<string>();
  const same = (entryText: string, sent: string): boolean => {
    const a = entryText.trim(), b = sent.trim();
    return a === b || (a.length > 0 && b.startsWith(a));
  };
  const take = (t: TurnLike, s: SentLocal): boolean => {
    for (const e of t.entries) {
      if (e.kind.kind !== "userMessage" || used.has(e.key.itemId)) continue;
      if (!same(e.text.kind === "value" ? e.text.value ?? "" : "", s.text)) continue;
      used.add(e.key.itemId);
      if (!e.attachments || e.attachments.length === 0) out.set(e.key.itemId, s.atts);
      return true;
    }
    return false;
  };
  for (const s of sends) {
    if (s.steer) {
      for (let i = turns.length - 1; i >= 0; i--) {
        const end = num(turns[i].completedAt);
        if (end !== null && end < s.at - SEND_BEFORE_MS) continue; // 送信の前に終わっていたturnの発言ではない
        if (take(turns[i], s)) break;
      }
      continue;
    }
    for (const t of turns) {
      const st = num(t.startedAt);
      if (st !== null && (st < s.at - SEND_BEFORE_MS || st > s.at + SEND_AFTER_MS)) continue;
      if (take(t, s)) break;
    }
  }
  return out;
}

export interface CarryPlan {
  /** 再送で付ける元の添付（元のチャットの台帳のID）。 */
  carry: { id: string; label: string }[];
  /** 付けられない添付と理由（黙って落とさず、画面に出して確認を取る）。 */
  lost: { label: string; reason: string }[];
}

/** 編集して再送で、元の発言の添付を付けられるかを分ける。付けられるのは、このアプリが保持していて使える（コピー済み）ものだけ。 */
export function classifyCarry(atts: EntryAttachment[], locals: AttachmentEntry[]): CarryPlan {
  const plan: CarryPlan = { carry: [], lost: [] };
  const seen = new Set<string>();
  for (const a of atts) {
    const m = chipModel(a);
    if (!a.name) { plan.lost.push({ label: m.label, reason: "履歴から内容を復元できないため、付けられません" }); continue; }
    const l = matchLocal(a.path, locals);
    if (!l) { plan.lost.push({ label: m.label, reason: "このアプリが保持するコピーが見つからないため、付けられません（外部で作られた会話の添付など）" }); continue; }
    if (l.state.kind !== "ready") { plan.lost.push({ label: m.label, reason: l.state.kind === "copying" ? "コピー中のため付けられません" : "コピーが見つからない（欠損）かコピーに失敗しているため、付けられません" }); continue; }
    if (seen.has(l.id)) continue;
    seen.add(l.id);
    plan.carry.push({ id: l.id, label: m.label });
  }
  return plan;
}

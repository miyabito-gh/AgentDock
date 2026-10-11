// 会話の折りたたみの純粋ロジック（DOMは扱わない）。
// 「発言」＝会話の項目のうちユーザーの発言と Codex の返答（作業の記録・承認カードなどは数えない）。

import { foldAscii } from "./find.ts";

/** 既定で表示する直近の発言の件数。 */
export const FOLD_KEEP = 20;
/** 長い依頼文を切る目安（これを超えたら切る）。 */
export const LONG_LINES = 8;
export const LONG_CHARS = 600;

interface TurnLike { entries: { kind: { kind: string } }[] }

const isMessage = (k: string): boolean => k === "userMessage" || k === "agentMessage";
const messageCount = (t: TurnLike): number => t.entries.filter((e) => isMessage(e.kind.kind)).length;

export interface FoldSplit {
  /** 折りたたむ先頭のturn数（0なら折りたたまない）。 */
  hiddenTurns: number;
  /** 折りたたまれる発言の件数。 */
  hiddenMessages: number;
}

/**
 * 直近 keep 件以上の発言を含む最小の末尾のturn群を表示側にし、それより古いturnを折りたたむ。
 * 分割点は必ずturnの境目（1つのturnの作業の記録・返答を途中で切らない）。そのため表示側は keep 件より1〜数件多いことがある。
 * 全体が keep 件以内、または折りたたまれる発言が0件なら折りたたまない。
 */
export function splitFold(turns: TurnLike[], keep: number = FOLD_KEEP): FoldSplit {
  const none: FoldSplit = { hiddenTurns: 0, hiddenMessages: 0 };
  let acc = 0;
  for (let i = turns.length - 1; i >= 0; i--) {
    acc += messageCount(turns[i]);
    if (acc >= keep) {
      if (i === 0) return none;
      const hiddenMessages = turns.slice(0, i).reduce((n, t) => n + messageCount(t), 0);
      return hiddenMessages > 0 ? { hiddenTurns: i, hiddenMessages } : none;
    }
  }
  return none;
}

/** 発言の本文（検索の対象と同じ種類）だけを取り出す。値が無い項目は飛ばす。 */
export function messageTexts(turns: { entries: { kind: { kind: string }; text: { kind: string; value?: string } }[] }[]): string[] {
  const out: string[] = [];
  for (const t of turns) for (const e of t.entries) {
    if (isMessage(e.kind.kind) && e.text.kind === "value" && typeof e.text.value === "string") out.push(e.text.value);
  }
  return out;
}

/** Markdown の装飾記号を除いた形（太字などの境目をまたぐ一致を見落とさないため）。 */
const stripMarks = (s: string): string => s.replace(/[*_~`]/g, "");

/**
 * 検索語が texts のどれかに含まれるか（折りたたみ・長文を自動で展開すべきか）。
 * 画面の検索は表示後の文字を探すので、原文と装飾を除いた文の両方で調べる。広めに判定し、展開しすぎる側に倒す。
 */
export function needsExpand(texts: string[], query: string): boolean {
  if (!query) return false;
  const q = foldAscii(query);
  const qs = stripMarks(q);
  return texts.some((t) => {
    const f = foldAscii(t);
    return f.includes(q) || (qs !== "" && stripMarks(f).includes(qs));
  });
}

/** 長い依頼文か（8行超または600文字超）。 */
export function isLongBody(text: string): boolean {
  return text.length > LONG_CHARS || text.split("\n").length > LONG_LINES;
}

/** チャットごとの展開状態（メモリ内のみ）。 */
export interface FoldMem { expanded: boolean; openMsgs: string[] }

/** チャットID→展開状態。画面を作り直しても（別チャットへ切り替えて戻っても）保たれる。直近 cap チャットまで（古いものから忘れる）。 */
export function createFoldMemory(cap: number = 50) {
  const m = new Map<string, FoldMem>();
  return {
    get(id: string): FoldMem {
      const v = m.get(id);
      if (v) { m.delete(id); m.set(id, v); }
      return v ?? { expanded: false, openMsgs: [] };
    },
    set(id: string, v: FoldMem): void {
      m.delete(id);
      m.set(id, v);
      while (m.size > cap) m.delete(m.keys().next().value as string);
    },
    size: (): number => m.size,
  };
}

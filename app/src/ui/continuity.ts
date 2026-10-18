// 継続チャットの表示用の純関数（DESIGN_CONT §7.4。判定規則はホストの rules/continuity.rs `parse_envelope` と同じ）。
import type { ChatLocalView } from "../ipc/types";

const END = "［AgentDock資料ここまで］";
const HEAD_PREFIX = "［AgentDock資料 v1";
const TRUNCATED_NOTE = "（一部省略）";

export interface SplitEnvelope {
  /** 資料ブロック全体（見出し行から終端行の末尾まで）。資料がなければ null。 */
  envelope: string | null;
  purposeRev: number | null;
  handoffRounds: number | null;
  truncated: boolean;
  /** 資料ブロックと区切りの空行を除いた、利用者の文。資料がなければ本文そのまま。 */
  userText: string;
}

const plain = (text: string): SplitEnvelope => ({ envelope: null, purposeRev: null, handoffRounds: null, truncated: false, userText: text });

function headFields(rest: string): { purposeRev: number | null; handoffRounds: number | null; truncated: boolean } {
  let purposeRev: number | null = null;
  let handoffRounds: number | null = null;
  let truncated = false;
  for (const f of rest.split("｜").filter((x) => x !== "")) {
    if (f.startsWith("目的メモ 版")) {
      const v = f.slice("目的メモ 版".length).trim();
      purposeRev = /^\d+$/.test(v) ? Number(v) : null;
    } else if (f.startsWith("引き継ぎ ")) {
      const v = f.slice("引き継ぎ ".length);
      const m = /^\d+/.exec(v);
      handoffRounds = m ? Number(m[0]) : null;
      truncated = v.includes(TRUNCATED_NOTE);
    }
  }
  return { purposeRev, handoffRounds, truncated };
}

/** 本文が資料ブロックで始まるときだけ分ける。先頭が見出し行で、終端行があること（なければ本文そのまま）。 */
export function splitEnvelope(text: string): SplitEnvelope {
  if (!text.startsWith(HEAD_PREFIX)) return plain(text);
  const headEnd = text.indexOf("\n");
  if (headEnd < 0) return plain(text);
  const headLine = text.slice(0, headEnd).replace(/\r+$/, "");
  if (!headLine.endsWith("］")) return plain(text);
  const fields = headLine.slice(HEAD_PREFIX.length, headLine.length - 1);
  if (!(fields === "" || fields.startsWith("｜"))) return plain(text);
  let pos = headEnd + 1;
  for (;;) {
    if (pos > text.length) return plain(text);
    const nl = text.indexOf("\n", pos);
    const lineEnd = nl < 0 ? text.length : nl;
    if (text.slice(pos, lineEnd).replace(/\r+$/, "") === END) {
      const end = pos + END.length;
      let user = text.slice(end);
      for (let i = 0; i < 2; i++) user = user.startsWith("\r\n") ? user.slice(2) : user.startsWith("\n") ? user.slice(1) : user;
      return { envelope: text.slice(0, end), ...headFields(fields), userText: user };
    }
    if (lineEnd >= text.length) return plain(text);
    pos = lineEnd + 1;
  }
}

/** 折りたたみ行の文字（「資料: 目的メモ（版3）・引き継ぎ 3往復（12,345字）」）。 */
export function envelopeLabel(s: SplitEnvelope): string {
  const parts: string[] = [];
  if (s.purposeRev !== null) parts.push(`目的メモ（版${s.purposeRev}）`);
  if (s.handoffRounds !== null) parts.push(`引き継ぎ ${s.handoffRounds}往復${s.truncated ? "（一部省略）" : ""}`);
  const chars = s.envelope ? [...s.envelope].length : 0;
  return `資料: ${parts.length ? parts.join("・") : "AgentDock資料"}（${chars.toLocaleString("en-US")}字）`;
}

/** 引き継ぎ先がある thread は、左一覧から隠す（隠すだけで削除・アーカイブはしない）。 */
export const isHiddenPredecessor = (l: ChatLocalView | undefined): boolean => !!l?.supersededBy;

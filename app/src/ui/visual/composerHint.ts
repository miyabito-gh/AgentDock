// 編集中の依頼欄の案内文（純粋ロジック）。送れない理由を最優先、次に作業中（送ると追加指示）、最後に選択の有無。
// 実行: node --test src/ui/visual/composerHint.test.ts （tscの対象外）
export function composerPlaceholder(lock: string | null, running: boolean, hasSelection: boolean): string {
  if (lock) return lock;
  if (running) return "作業中です。送ると追加指示になります";
  return hasSelection ? "この範囲に依頼…" : "全体への依頼…";
}

interface ReplyEntry { kind: { kind: string }; text: { kind: string; value?: string } }
/** 畳んでいる間に見せる最新のAIの返答。最後の返答の本文（空白をつぶして、max文字で切り詰め）。本文が取れていない返答は飛ばし、なければnull。 */
export function latestReplyText(turns: { entries: ReplyEntry[] }[], max = 200): string | null {
  for (let i = turns.length - 1; i >= 0; i--) {
    const es = turns[i].entries;
    for (let j = es.length - 1; j >= 0; j--) {
      const e = es[j];
      if (e.kind.kind !== "agentMessage" || e.text.kind !== "value") continue;
      const flat = (e.text.value ?? "").replace(/\s+/g, " ").trim();
      if (!flat) continue;
      const chars = Array.from(flat);
      return chars.length > max ? chars.slice(0, max).join("") + "…" : flat;
    }
  }
  return null;
}

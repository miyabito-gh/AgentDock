// 承認カードの選択肢の並べ方（純関数）。Codex が示した選択肢だけを、表示の場所に振り分ける。
// 規則: 主ボタン＝最初の「許可・1回だけ」、常時表示＝「許可・このセッション中」と「辞退」、
//       それ以外（永続の許可・ネットワーク規則・中止など）は「その他」へ。項目が1つだけなら「その他」を作らず直接出す。
// 実行: node --test src/ui/requestChoices.test.ts
import type { DecisionOption } from "../ipc/types";

export interface RequestChoices {
  primary: DecisionOption | null;
  session: DecisionOption | null;
  decline: DecisionOption | null;
  /** 「その他」の項目（元の並び順）。 */
  more: DecisionOption[];
  /** more が1項目だけのとき、メニューにせずカードに直接出す。 */
  inline: DecisionOption | null;
}

export function requestChoices(options: DecisionOption[]): RequestChoices {
  const primary = options.find((o) => o.effect === "allow" && o.scope === "once") ?? null;
  const session = options.find((o) => o !== primary && o.effect === "allow" && o.scope === "session") ?? null;
  const decline = options.find((o) => o.effect === "deny" && o.scope === "once") ?? null;
  const more = options.filter((o) => o !== primary && o !== session && o !== decline);
  return { primary, session, decline, more, inline: more.length === 1 ? more[0] : null };
}

/** 永続の効力を持つ選択肢（ルールの保存など）。「その他」の中で目立たせる。 */
export const isPersistent = (o: DecisionOption): boolean => o.scope === "persistent";

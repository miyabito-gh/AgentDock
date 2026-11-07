// 「この会話の以降の承認もすべて許可」モードの判定（純関数）。
// 規則: 利用者が承認カードで明示的に選んだ会話だけ。メモリだけ（再起動・会話の切り替えで引き継がない）。
//       対象はコマンド・ファイル変更・追加権限の承認だけ。質問とMCPの許可確認は対象外（利用者の判断が要る）。
//       許可は「1回だけ」の選択肢で返す（効力範囲を勝手に広げない）。無ければ自動では答えない。
// 実行: node --test src/ui/autoAllow.test.ts
import type { DecisionOption, PendingRequest } from "../ipc/types";

const AUTO_KINDS = new Set(["commandApproval", "fileChangeApproval", "permissionsApproval"]);

/** 自動で許可できる要求の、許可（1回だけ）の選択肢。出せない要求は null。 */
export function autoAllowOption(r: PendingRequest): DecisionOption | null {
  if (r.state.kind !== "pending" || !AUTO_KINDS.has(r.kind.kind)) return null;
  return r.options.find((o) => o.effect === "allow" && o.scope === "once") ?? null;
}

/** 応答済みの管理に使う要求のキー（source も含める）。 */
export const requestKey = (r: PendingRequest): string => `${r.key.source}/${r.key.requestId}`;

/** いま自動で許可する要求と選択肢。`on` はモードが入っている会話ID、`done` は応答済みの要求ID。 */
export function dueAutoAllows(requests: PendingRequest[], on: ReadonlySet<string>, done: ReadonlySet<string>, skipChat: (chatId: string) => boolean): { r: PendingRequest; option: DecisionOption }[] {
  const out: { r: PendingRequest; option: DecisionOption }[] = [];
  for (const r of requests) {
    if (!on.has(r.chat.id) || done.has(requestKey(r)) || skipChat(r.chat.id)) continue;
    const option = autoAllowOption(r);
    if (option) out.push({ r, option });
  }
  return out;
}

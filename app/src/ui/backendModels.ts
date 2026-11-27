// 担当AI（バックエンド）ごとのモデル一覧・チャットのキーの引き当て（純粋関数）。
import type { BackendKind, ChatKey } from "../ipc/types";

/**
 * id だけ持っている箇所用に、一覧から実際のチャットのキーを引く。
 * 見つからないときは従来どおり Codex として扱う（一覧に無い＝未取得のチャット）。
 */
export function chatKeyOfId(chats: ReadonlyArray<{ key: ChatKey }>, id: string): ChatKey {
  return chats.find((c) => c.key.id === id)?.key ?? { backend: "codex", id };
}

const AI_LABEL: Record<BackendKind, { name: string; full: string }> = {
  codex: { name: "Codex", full: "Codex" },
  antigravity: { name: "旧チャット", full: "旧チャット" },
};
export const aiName = (backend: BackendKind): string => AI_LABEL[backend].name;
export const aiFullName = (backend: BackendKind): string => AI_LABEL[backend].full;
/** 印（一覧・見出し・候補行）に添えるツールチップ。 */
export const aiTip = (backend: BackendKind): string => `${AI_LABEL[backend].full}の会話`;

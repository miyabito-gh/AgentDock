// 担当AI（バックエンド）ごとのモデル一覧・チャットのキーの引き当て（純粋関数）。
import type { BackendKind, ChatKey, ModelInfo } from "../ipc/types";

/** そのバックエンドのチャットに渡すモデル一覧。Antigravity は Codex の一覧を使わない。 */
export function modelsForBackend(backend: BackendKind, codex: ModelInfo[], antigravity: ModelInfo[]): ModelInfo[] {
  return backend === "antigravity" ? antigravity : codex;
}

/**
 * id だけ持っている箇所用に、一覧から実際のチャットのキーを引く。
 * 見つからないときは従来どおり Codex として扱う（一覧に無い＝未取得のチャット）。
 */
export function chatKeyOfId(chats: ReadonlyArray<{ key: ChatKey }>, id: string): ChatKey {
  return chats.find((c) => c.key.id === id)?.key ?? { backend: "codex", id };
}

/**
 * 担当AI（バックエンド）の表示名の正本。表記ゆれを防ぐため、画面に出す AI の名前はここから引く（BackendKind を足すと型で漏れを検出する）。
 * - name: 発話者・状態文・一覧／見出しの印に出す短い名前（Antigravity のチャットは Gemini）。
 * - full: 選択肢・案内文に出す正式な名前（Antigravity（Gemini））。
 */
const AI_LABEL: Record<BackendKind, { name: string; full: string }> = {
  codex: { name: "Codex", full: "Codex" },
  antigravity: { name: "Gemini", full: "Antigravity（Gemini）" },
};
export const aiName = (backend: BackendKind): string => AI_LABEL[backend].name;
export const aiFullName = (backend: BackendKind): string => AI_LABEL[backend].full;
/** 印（一覧・見出し・候補行）に添えるツールチップ。 */
export const aiTip = (backend: BackendKind): string => `${AI_LABEL[backend].full}の会話`;

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

/** 会話の発話者・状態文に出す担当AIの名前（Antigravity のチャットは Gemini。左一覧の「Gemini」の印と同じ語）。 */
const AI_NAME: Record<BackendKind, string> = { codex: "Codex", antigravity: "Gemini" };
export const aiName = (backend: BackendKind): string => AI_NAME[backend];

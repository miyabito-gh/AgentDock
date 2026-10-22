// snapshot から台帳の可否を組み立てる（commands.ts は純粋な定義、ここだけが derive に依存する）。
// 表示のために接続・再開・読取りを増やさない。
import type { Chat, HostSnapshot, ParityOp } from "../ipc/types";
import { ALL_COMMANDS, type CommandContext, type CommandView } from "./commands";
import { draftIneligible } from "./contUsage";
import { chatStops, isRunning, rootView, stopOpen } from "./derive";

/** snapshot から導けない、呼び出し側が渡す材料（省略時は「理由なし／すべてオフ」）。 */
export interface ContextExtra {
  cwdWhy?: string | null;
  folderCount?: number;
  checked?: Record<string, boolean>;
}

/** snapshot から可否の判断材料を作る（表示のために接続・再開・読取りを増やさない）。 */
export function commandContext(snap: HostSnapshot, chat: Chat | null, connected: boolean, op: ParityOp | null, gitMissing = false, extra: ContextExtra = {}): CommandContext {
  const stopped = chat ? chatStops(snap, chat).some(stopOpen) : false;
  const local = chat ? snap.chatLocals.find((l) => l.chat.id === chat.key.id) : undefined;
  const fresh = chat ? rootView(snap, chat)?.freshness : undefined;
  return {
    cap: op ? snap.opCapabilities.find((c) => c.op === op) : undefined,
    connected,
    hasChat: !!chat,
    busy: chat ? isRunning(snap, chat) || stopped : false,
    deletePending: !!local?.deletePending,
    externalRunning: !!chat && chat.origin === "external" && fresh !== "live",
    gitMissing,
    pinned: !!chat?.pinned,
    archived: !!chat && (local?.visibility.kind === "archived" || (chat.archived.kind === "value" && chat.archived.value === true)),
    isGeneral: chat?.kind === "general",
    isLive: fresh === "live",
    cwdWhy: extra.cwdWhy ?? null,
    folderCount: extra.folderCount ?? 0,
    checked: extra.checked ?? {},
    continuationOff: !snap.settings.continuation.enabled,
    draftBlock: draftIneligible({ orgOwned: !!local?.org && !local.org.released, supersededBy: !!local?.supersededBy, structuredOutput: snap.sources[0]?.capabilities.structuredOutput }),
  };
}

/** 3つの入口（メニュー・ヘッダー・Ctrl+K）が使う可否の一覧。台帳（同等性＋アプリ操作）の全件。能力が非対応のものは「このAIでは非対応」の理由つきで無効になる。 */
export const commandViews = (snap: HostSnapshot, chat: Chat | null, connected: boolean, gitMissing = false, marks: Record<string, string | undefined> = {}, extra: ContextExtra = {}): CommandView[] =>
  ALL_COMMANDS.map((c) => {
    const ctx = commandContext(snap, chat, connected, c.op, gitMissing, extra);
    return {
      cmd: c,
      state: c.enabled(ctx),
      mark: marks[c.id],
      label: c.dynamicLabel ? c.dynamicLabel(ctx) : c.label,
      act: c.dynamicAct ? c.dynamicAct(ctx) : c.act,
      checked: c.checked ? c.checked(ctx) : undefined,
    };
  });

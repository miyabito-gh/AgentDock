// 表示用の変換。型は ipc/types.ts のものだけを使う。
import type {
  AgentKey, AgentState, ChatKey, Freshness, HoldTarget, Known, StopSummary, EvidenceSource, QueueHold, QueueStopCause,
} from "../ipc/types";

export const keyStr = (k: ChatKey | AgentKey): string => `${k.backend}:${k.id}`;

export const STATE: Record<AgentState, { t: string; c: string }> = {
  running: { t: "実行中", c: "run" }, waiting: { t: "対応待ち", c: "wait" }, failed: { t: "失敗", c: "fail" },
  interrupted: { t: "中断", c: "int" }, done: { t: "完了", c: "done" }, idle: { t: "待機（idle）", c: "idle" },
  unknown: { t: "状態不明", c: "unk" }, initializing: { t: "初期化中", c: "run" }, closed: { t: "終了", c: "done" },
};

export const FRESH: Record<Freshness, { c: string; t: string }> = {
  live: { c: "live", t: "受信中" },
  historyOnly: { c: "stale", t: "履歴取得済み・live未復旧" },
  needsReconcile: { c: "check", t: "要照合" },
  disconnected: { c: "off", t: "切断" },
  unsupported: { c: "off", t: "非対応" },
};

/** 欠け方ごとの表示。notFetched=未確認、unsupported=非対応、missing=欠損。0や空文字で代用しない。 */
export function showKnown<T>(k: Known<T>, fmt: (v: T) => string = (v) => String(v)): string {
  switch (k.kind) {
    case "value": return k.basis === "inferred" ? `${fmt(k.value)}（推定）` : fmt(k.value);
    case "notFetched": return "未確認";
    case "unsupported": return "非対応";
    case "missing": return "欠損";
  }
}
export const knownValue = <T,>(k: Known<T>): T | null => (k.kind === "value" ? k.value : null);

export const SOURCE_LABEL: Record<EvidenceSource, string> = {
  liveEvent: "直接取得（ライブ）", historyRead: "保存履歴", statusQuery: "状態照会", response: "応答", hostReconcile: "ホスト照合",
};

export const STOP_LABEL: Record<StopSummary, string> = {
  notRequested: "中断を要求していません",
  interruptRequested: "中断要求済み・停止を未確認",
  turnEndConfirmed: "turn終端を確認しました",
  managedExecEndConfirmed: "管理実行の終了を確認しました",
  osGoneConfirmed: "OS上の実体の消滅を確認しました",
  unconfirmed: "停止を確認できていません",
  ownershipUnknown: "所有を確認できないため停止対象に含めていません",
};

/** 保留の対象の一覧（名前・状態・鮮度）。状態と鮮度は別軸のまま並べる。 */
const targetList = (ts: HoldTarget[], nameOf: (a: AgentKey) => string): string =>
  ts.map((t) => `${nameOf(t.agent)}（${STATE[t.state].t}／${FRESH[t.freshness].t}）`).join("、");

/** 自動送信を待っている理由。対象があるものは対象も示す（§3.7）。 */
export function holdText(h: QueueHold, nameOf: (a: AgentKey) => string): string {
  switch (h.kind) {
    case "parentWorking": return "親が作業中のため、完了を待っています";
    case "descendantsActive": return `子・孫が作業中・待機中のため保留: ${targetList(h.targets, nameOf)}`;
    case "stateUnknown": {
      const parts = [h.targets.length ? targetList(h.targets, nameOf) : "", h.descendantScanIncomplete ? "子孫の探索が完了していません" : ""].filter(Boolean);
      return `状態を確認できないため保留${parts.length ? `: ${parts.join("／")}` : ""}`;
    }
    case "notLive": return `受信（ライブ監視）が戻っていないため保留: ${targetList(h.targets, nameOf)}`;
    case "stopUnconfirmed": return "停止を確認できていないため保留";
    case "acceptanceUnknown": return "受理不明の依頼があるため保留（履歴との照合を待っています）";
    case "deletePending": return "削除保留中のため、送信を止めています";
    case "disconnected": return "接続が切れているため保留（再接続後に判断します）";
    case "quitting": return "完全終了の手順中のため、新しい送信を始めていません";
    case "saveFailed": return "送信前の保存に失敗したため、送っていません。保存を再試行して成功してから送ります";
  }
}

/** キューを止めた原因。 */
export function stopCauseText(c: QueueStopCause, nameOf: (a: AgentKey) => string): string {
  switch (c.kind) {
    case "parentFailed": return "親の作業が失敗したため、後続の送信を止めています";
    case "parentInterrupted": return "親の作業が中断されたため、後続の送信を止めています";
    case "descendantFailed": return `${nameOf(c.agent)} の作業が失敗したため、後続の送信を止めています`;
    case "descendantInterrupted": return `${nameOf(c.agent)} の作業が中断されたため、後続の送信を止めています`;
    case "sendRejected": return "依頼の送信が受け付けられなかったため、後続の送信を止めています";
    case "notAcceptedAfterReconcile": return "履歴に受理の痕跡がない依頼があるため、後続の送信を止めています";
    case "attachmentUnavailable": return `添付「${c.name}」を使えないため、依頼を送らずに後続の送信を止めています（送信は試みていません）`;
    case "appServerKilled": return "強制終了でCodexのプロセスが消えたため、送信済みの依頼の完了を確認できないまま後続の送信を止めています（完了とは扱っていません）";
  }
}

export const pad = (n: number) => String(n).padStart(2, "0");
export const hms = (ms: number) => { const d = new Date(ms); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`; };

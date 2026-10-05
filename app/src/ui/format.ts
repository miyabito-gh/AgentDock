// 表示用の変換。型は ipc/types.ts のものだけを使う。
import type {
  AgentKey, AgentState, ChatKey, Freshness, Known, StopSummary, EvidenceSource, QueueHoldReason,
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

export function holdText(r: QueueHoldReason): string {
  switch (r.kind) {
    case "notLive": return `live監視が未復旧のため保留（${FRESH[r.freshness].t}）`;
    case "descendantsNotFinished": return "子孫が作業中のため保留";
    case "stopUnconfirmed": return "停止未確認のため保留";
    case "acceptanceUnknown": return "受理不明の依頼があるため保留";
    case "userPaused": return "一時停止中";
    case "other": return r.message;
  }
}

export const pad = (n: number) => String(n).padStart(2, "0");
export const hms = (ms: number) => { const d = new Date(ms); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`; };

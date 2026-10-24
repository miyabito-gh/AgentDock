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
  interruptRequested: "停止を要求しました。止まったかどうかは、まだ確認できていません",
  turnEndConfirmed: "やりとりが終わったことを確認しました",
  managedExecEndConfirmed: "AgentDock が起動した実行が終わったことを確認しました",
  osGoneConfirmed: "プロセスが終わったことを確認しました",
  unconfirmed: "止まったかどうか確認できていません。プロセスが残っている可能性があります",
  ownershipUnknown: "AgentDock が起動したものか確認できないため、停止の対象に含めていません",
};

/** 「一部を保存できませんでした」の保存済み／未保存の文。取れていない側を「なし」で埋めず、確認できないと書く。 */
export function savedPartText(saved: string | null, unsaved: string | null): string {
  return `保存済みの内容: ${saved ?? "確認できません"}。未保存の内容: ${unsaved ?? "確認できません"}`;
}

/** 保留の対象の一覧（名前・状態・鮮度）。状態と鮮度は別軸のまま並べる。 */
export const targetList = (ts: HoldTarget[], nameOf: (a: AgentKey) => string): string =>
  ts.map((t) => `${nameOf(t.agent)}（${STATE[t.state].t}／${FRESH[t.freshness].t}）`).join("、");

/** 自動送信を待っている理由。対象があるものは対象も示す（§3.7）。 */
export function holdText(h: QueueHold, nameOf: (a: AgentKey) => string, rootId?: string): string {
  switch (h.kind) {
    case "parentWorking": return "親が作業中のため、完了を待っています";
    case "descendantsActive": return `子・孫が作業中・待機中のため保留: ${targetList(h.targets, nameOf)}`;
    case "stateUnknown": {
      // 詳細（名前・状態・鮮度）はツールチップ側に出す。ここは短く言い切る。
      const kids = h.targets.filter((t) => t.agent.id !== rootId).length;
      if (h.descendantScanIncomplete) return "子孫の探索が終わるまで、送らずに待っています";
      if (kids > 0 && kids === h.targets.length) return `子${kids}件の状態が不明のため、送らずに待っています`;
      return "親の状態を確認できないため、送らずに待っています";
    }
    case "notLive": return `受信（ライブ監視）が戻っていないため保留: ${targetList(h.targets, nameOf)}`;
    case "stopUnconfirmed": return "停止を確認できていないため保留";
    case "acceptanceUnknown": return "受理不明の依頼があるため保留（履歴との照合を待っています）";
    case "operationUnconfirmed": return "結果が未確認の操作（レビュー・圧縮）があるため保留（状態の確認を待っています）";
    case "deletePending": return "削除保留中のため、送信を止めています";
    case "disconnected": return "接続が切れているため保留（再接続後に判断します）";
    case "quitting": return "完全終了の手順中のため、新しい送信を始めていません";
    case "saveFailed": return "送信前の保存に失敗したため、送っていません。保存を再試行して成功してから送ります";
  }
}

/** 状態不明の子孫を残したまま送るときの注記（親の完了を確認済み）。 */
export const unconfirmedNote = (n: number): string => `子${n}件の状態は未確認のまま送ります`;

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

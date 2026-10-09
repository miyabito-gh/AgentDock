// 計画／実行・Goal・Codex の状態・memories の表示と操作（段階③ P3-3）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 取れていない値は「未取得／未確認」と書き、0や空で代用しない。選択値と受理値は分けて表示する。
//       トークン数・使用率は受け取った値をそのまま出し、料金・残量・回復時刻に換算しない。
//       操作の結果は「受け付けた」までしか書かない（目標の反映は上の表示、memoriesは読み直した値で確認する）。
import { useCallback, useEffect, useState } from "react";
import * as host from "../ipc/client";
import type {
  BackendStatus, Chat, ChatLocalView, ChatModelSettings, Goal, GoalStatus, GoalUpdate, Known, LimitWindowRole, MemoryStatus, ModelChoice, OpAck, OpCapability, ResetMemoryResult, WorkMode,
} from "../ipc/types";
import { chatName } from "./derive";
import { showKnown } from "./format";

const errText = (e: unknown) => host.asIpcError(e).message;

export const WORK_MODE_TEXT: Record<WorkMode, string> = { plan: "計画（Plan）", default: "実行（Default）" };

const GOAL_STATUS_TEXT: Record<Exclude<GoalStatus["kind"], "unknown">, string> = {
  active: "進行中", paused: "一時停止", blocked: "ブロック中", usageLimited: "使用量の上限", budgetLimited: "予算の上限", complete: "完了",
};
/** 設定で選べる状態（未知の値は選べない）。 */
const SETTABLE: Array<Exclude<GoalStatus["kind"], "unknown">> = ["active", "paused", "blocked", "usageLimited", "budgetLimited", "complete"];

export const goalStatusText = (s: GoalStatus): string => (s.kind === "unknown" ? `不明な状態（原文: ${s.raw}）` : GOAL_STATUS_TEXT[s.kind]);

/** OpAck を、結果を断定しない文にする（受け付けた／拒否された／受理を確認できない）。 */
export function ackText(a: OpAck, what: string): string {
  switch (a.kind) {
    case "accepted": return `${what}を要求し、Codex が受け付けました。反映は表示で確認してください。`;
    case "rejected": return `${what}は受け付けられませんでした（${a.message}）。`;
    case "unknown": return `${what}の受理を確認できません（${a.message}）。読み直した値を表示します。再送はしません。`;
  }
}

export function UnverifiedTag({ cap }: { cap: OpCapability | undefined }) {
  return cap?.verification === "unverified" ? <span className="tag unv">未確認</span> : null;
}

// ───────────────────────────── Goal ─────────────────────────────

/** 入力欄の上の常設欄。目標の取得は読取りだけで、表示のために会話を再開しない。 */
export function GoalBar({ goal, live, cap, busy, onEdit, onClear, onReload }: {
  goal: Known<Goal> | undefined;
  /** この会話が live 監視中か（未取得のときの表示の出し分け）。 */
  live: boolean;
  cap: OpCapability | undefined;
  busy: boolean;
  onEdit: () => void;
  onClear: () => void;
  onReload: () => void;
}) {
  if (!cap || cap.support !== "supported" || !goal) return null;
  if (goal.kind === "value") {
    const g = goal.value;
    return (
      <div className="goal" role="status" aria-label="Goal">
        <b>Goal</b><span className="tag">{goalStatusText(g.status)}</span>
        <span className="goal-obj" title={`${g.objective}\n使用 ${showKnown(g.tokensUsed)} トークン／予算 ${showKnown(g.tokenBudget, String)}／${showKnown(g.timeUsedSecs, (n) => `${n}秒`)}（Codex が示した値のまま表示しています。換算しません）`}>{g.objective}</span>
        <UnverifiedTag cap={cap} />
        <button className="small" style={{ height: 22 }} disabled={busy} onClick={onEdit}>変更</button>
        <button className="small" style={{ height: 22 }} disabled={busy} onClick={onClear}>解除</button>
      </div>
    );
  }
  // 値なし（目標が設定されていない）は常設欄を出さない。メニューの「Goal を設定…」から設定できる。
  if (goal.kind === "notFetched" && live) {
    return (
      <div className="goal" role="status" aria-label="Goal">
        <b>Goal</b><span className="muted" style={{ flex: 1 }}>未取得（読み取れませんでした。会話は再開していません）</span>
        <button className="small" style={{ height: 22 }} onClick={onReload}>再取得</button>
      </div>
    );
  }
  return null;
}

/** 目標の設定・変更。変更した項目だけを要求する。結果は「受け付けた」までしか言わない。 */
export function GoalBody({ chat, current, live, cap, save }: {
  chat: Chat | undefined;
  current: Known<Goal> | undefined;
  live: boolean;
  cap: OpCapability | undefined;
  /** 要求する。戻り値は表示する文（結果を断定しない）。 */
  save: (u: GoalUpdate) => Promise<string>;
}) {
  const g = current?.kind === "value" ? current.value : null;
  const [objective, setObjective] = useState(g?.objective ?? "");
  const [status, setStatus] = useState<string>(g && g.status.kind !== "unknown" ? g.status.kind : "");
  const [budget, setBudget] = useState(g && g.tokenBudget.kind === "value" ? String(g.tokenBudget.value) : "");
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (!live) return <div className="content"><p>この操作はモックでは動きません。</p></div>;
  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!cap || cap.support !== "supported") return <div className="content"><p>{cap?.note ?? "この Codex 接続では Goal を使えません。"}</p></div>;

  const update = (): GoalUpdate => {
    const u: GoalUpdate = { objective: null, status: null, tokenBudget: null };
    if (objective.trim() !== (g?.objective ?? "")) u.objective = objective.trim();
    if (status && (g?.status.kind !== status)) u.status = { kind: status as Exclude<GoalStatus["kind"], "unknown"> } as GoalStatus;
    const b = budget.trim();
    if (b !== "" && /^\d+$/.test(b) && (g?.tokenBudget.kind !== "value" || String(g.tokenBudget.value) !== b)) u.tokenBudget = Number(b);
    return u;
  };
  const u = update();
  const changed = u.objective !== null || u.status !== null || u.tokenBudget !== null;
  const budgetBad = budget.trim() !== "" && !/^\d+$/.test(budget.trim());
  const go = async () => {
    setBusy(true); setMsg(null);
    try { setMsg(await save(u)); } catch (e) { setMsg(errText(e)); } finally { setBusy(false); }
  };
  return (
    <div className="content">
      <p className="small muted">対象: {chat ? chatName(chat) : "（不明）"}　<UnverifiedTag cap={cap} /></p>
      <div className="field"><span>目標</span><textarea rows={3} style={{ width: "100%" }} value={objective} onChange={(e) => setObjective(e.target.value)} aria-label="Goal の目標" /></div>
      <div className="field"><span>状態</span>
        <select value={status} onChange={(e) => setStatus(e.target.value)} aria-label="Goal の状態">
          <option value="">変更しない{g ? `（現在: ${goalStatusText(g.status)}）` : ""}</option>
          {SETTABLE.map((k) => <option key={k} value={k}>{GOAL_STATUS_TEXT[k]}</option>)}
        </select></div>
      <div className="field"><span>トークンの予算</span>
        <input type="text" inputMode="numeric" value={budget} onChange={(e) => setBudget(e.target.value)} placeholder="空欄＝変更しない" aria-label="トークンの予算" />
        <span className="note">Codex が数える値のままで、料金や残量には換算しません。{budgetBad ? "（数字で入力してください）" : ""}</span></div>
      {msg ? <div className="why" style={{ marginBottom: 8 }}>{msg}</div> : null}
      <div className="acts">
        <button className="btn-main" disabled={busy || !changed || budgetBad} onClick={() => void go()}>{busy ? "要求しています…" : "設定を要求"}</button>
      </div>
      <p className="small muted">必要なら、この会話を再開してから要求します（ユーザー操作のときだけ）。Goal の状態は作業の完了・失敗を意味しません。</p>
    </div>
  );
}

// ───────────────────────────── Codex の状態 ─────────────────────────────

const ROLE_TEXT: Record<LimitWindowRole, string> = { primary: "短い期間の枠", secondary: "長い期間の枠" };

function LimitsView({ k }: { k: BackendStatus["rateLimits"] }) {
  if (k.kind !== "value") return <>{showKnown(k)}</>;
  if (k.value.length === 0) return <>枠の情報はありません</>;
  return (
    <ul className="quit-list">
      {k.value.map((l, i) => (
        <li key={i}><b>{showKnown(l.name)}</b>
          {l.windows.length === 0 ? <span className="sub">使用率は取得できていません</span> : l.windows.map((w, j) => (
            <span className="sub" key={j}>{ROLE_TEXT[w.role]}: 使用率 {w.usedPercent}%（期間 {showKnown(w.windowMinutes, (n) => `${n}分`)}）</span>
          ))}
        </li>
      ))}
    </ul>
  );
}

export interface StatusProps {
  live: boolean;
  chat: Chat | undefined;
  settings: ChatModelSettings | undefined;
  local: ChatLocalView | undefined;
  caps: OpCapability[];
}

const choiceText = (c: ModelChoice | null | undefined): string => (c ? `${c.model}／${c.effort ?? "既定"}／速度 ${c.speedTier ?? "指定なし"}` : "未選択");

/** Codex の状態（読取りのみ）・選択チャットの設定・personality・memories。取れない項目は未取得のまま。 */
export function StatusBody({ live, chat, settings, local, caps }: StatusProps) {
  const [status, setStatus] = useState<BackendStatus | null>(null);
  const [statusErr, setStatusErr] = useState<string | null>(null);
  const [mem, setMem] = useState<MemoryStatus | null>(null);
  const [memErr, setMemErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [memMode, setMemMode] = useState<string>(local?.memoryMode ?? "");
  const [memMsg, setMemMsg] = useState<string | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [resetResult, setResetResult] = useState<ResetMemoryResult | null>(null);
  const [busy, setBusy] = useState(false);
  const cap = (op: OpCapability["op"]) => caps.find((c) => c.op === op);
  const memCap = cap("memory");
  const memUsable = memCap?.support === "supported";

  const load = useCallback(async () => {
    if (!live) return;
    setLoading(true); setStatusErr(null); setMemErr(null);
    try { setStatus(await host.getBackendStatus()); } catch (e) { setStatus(null); setStatusErr(errText(e)); }
    if (memUsable) { try { setMem(await host.getMemoryStatus()); } catch (e) { setMem(null); setMemErr(errText(e)); } }
    setLoading(false);
  }, [live, memUsable]);
  useEffect(() => { void load(); }, [load]);

  if (!live) return <div className="content"><p>この操作はモックでは動きません。</p></div>;

  const accepted = settings?.accepted.kind === "value" ? settings.accepted.value : null;
  const requestMode = async () => {
    if (!chat || !memMode) return;
    setBusy(true); setMemMsg(null);
    try { setMemMsg(ackText(await host.setMemoryMode(chat.key, memMode), "memories の設定")); } catch (e) { setMemMsg(errText(e)); } finally { setBusy(false); }
  };
  const doReset = async () => {
    setBusy(true); setMemMsg(null); setResetResult(null);
    try {
      const r = await host.resetMemory(true);
      setResetResult(r);
      if (r.statusAfter) setMem(r.statusAfter);
    } catch (e) { setMemMsg(errText(e)); } finally { setBusy(false); setConfirmReset(false); }
  };

  return (
    <div className="content">
      <div className="acts" style={{ marginBottom: 8 }}><button className="btn-line" disabled={loading} onClick={() => void load()}>{loading ? "取得しています…" : "再取得"}</button></div>

      <h4 style={{ margin: "4px 0" }}>接続と設定 <UnverifiedTag cap={cap("backendStatus")} /></h4>
      {statusErr ? <div className="why err">取得できませんでした: {statusErr}</div> : null}
      <div className="field"><span>版</span><span>{status ? showKnown(status.version) : "未取得"}</span></div>
      <div className="field"><span>実行ファイル</span><span className="mono">{status ? showKnown(status.executable) : "未取得"}</span></div>
      <div className="field"><span>experimental API</span><span>{status ? showKnown(status.experimentalEnabled, (b) => (b ? "有効" : "無効")) : "未取得"}</span></div>
      <div className="field"><span>アカウント</span><span>{status ? `${showKnown(status.accountKind)}／プラン ${showKnown(status.plan)}` : "未取得"}</span></div>
      <div className="field"><span>利用上限</span><div>{status ? <LimitsView k={status.rateLimits} /> : "未取得"}<span className="note">Codex が示した使用率のままで、残量・回復時刻には換算しません。</span></div></div>
      <div className="field"><span>累計の使用量</span><span>{status ? (status.usage.kind === "value"
        ? `累計 ${showKnown(status.usage.value.lifetimeTokens)} トークン／1日の最大 ${showKnown(status.usage.value.peakDailyTokens)} トークン` : showKnown(status.usage)) : "未取得"}</span></div>
      <div className="field"><span>設定の警告</span><div>{status ? (status.warnings.length ? <ul className="quit-list">{status.warnings.map((w, i) => <li key={i}>{w}</li>)}</ul> : "受信していません（警告がないことの確認ではありません）") : "未取得"}</div></div>

      <h4 style={{ margin: "10px 0 4px" }}>選択中のチャットの設定 <UnverifiedTag cap={cap("speedTier")} /></h4>
      {chat ? (
        <>
          <div className="field"><span>チャット</span><span>{chatName(chat)}</span></div>
          <div className="field"><span>モデル・推論・速度（選択）</span><span>{choiceText(settings?.selected)}</span></div>
          <div className="field"><span>モデル・推論・速度（受理済み）</span><span>{accepted ? choiceText(accepted) : `${settings ? showKnown(settings.accepted) : "未確認"}（受け取るまで確認不可）`}</span></div>
          <div className="field"><span>計画／実行（選択）</span><span>{settings?.workMode ? WORK_MODE_TEXT[settings.workMode] : "未選択"} <UnverifiedTag cap={cap("workMode")} /></span></div>
          <div className="field"><span>計画／実行（受理済み）</span><span>{settings ? showKnown(settings.acceptedWorkMode, (m) => WORK_MODE_TEXT[m]) : "未確認"}</span></div>
        </>
      ) : <p className="small muted">チャットを選ぶと、そのチャットの設定を表示します。</p>}

      <h4 style={{ margin: "10px 0 4px" }}>personality</h4>
      <div className="field"><span>状態</span><span>{cap("personality")?.note ?? "非推奨（このCodex版では選べない）"}（操作は置いていません）</span></div>

      <h4 style={{ margin: "10px 0 4px" }}>memories <UnverifiedTag cap={memCap} /></h4>
      {!memUsable ? <p className="small muted">{memCap?.note ?? "この Codex 接続では使えません。"}</p> : (
        <>
          {memErr ? <div className="why err">取得できませんでした: {memErr}</div> : null}
          <div className="field"><span>状態</span><span>{mem ? showKnown(mem.summary) : "未取得"}</span></div>
          <div className="field"><span>このチャットの設定</span>
            <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <select value={memMode} onChange={(e) => setMemMode(e.target.value)} aria-label="このチャットの memories の設定" disabled={!chat}>
                <option value="">変更しない{local?.memoryMode ? `（要求済み: ${local.memoryMode === "enabled" ? "有効" : local.memoryMode === "disabled" ? "無効" : local.memoryMode}）` : ""}</option>
                <option value="enabled">有効</option><option value="disabled">無効</option>
              </select>
              <button className="btn-line" disabled={busy || !chat || !memMode} onClick={() => void requestMode()}>設定を要求</button>
            </div>
            <span className="note">必要なら会話を再開してから要求します。受け付けた値を記録しますが、Codex が実際に適用したかは確認できません。</span></div>
          {memMsg ? <div className="why" style={{ marginBottom: 8 }}>{memMsg}</div> : null}
          <div className="field"><span>リセット</span>
            <div>
              {!confirmReset ? <button className="btn-danger" disabled={busy} onClick={() => setConfirmReset(true)}>記憶データをリセット…</button> : (
                <div className="why err" role="alertdialog" aria-label="memories のリセットの確認">
                  <p>Codex に保存されている記憶データを消します。<b>元に戻せません</b>。Codex の設定ファイルや認証には触れませんが、この Codex を使うすべての会話に影響する可能性があります。</p>
                  <div className="acts">
                    <button className="btn-danger" disabled={busy} onClick={() => void doReset()}>{busy ? "要求しています…" : "影響を理解した。リセットする"}</button>
                    <button className="btn-line" disabled={busy} onClick={() => setConfirmReset(false)}>やめる</button>
                  </div>
                </div>
              )}
              {resetResult ? (
                <p className="small" style={{ marginTop: 6 }}>
                  {ackText(resetResult.ack, "リセット")}
                  {resetResult.statusAfter ? `　実行後に読み直した状態: ${showKnown(resetResult.statusAfter.summary)}` : "　実行後の状態は読み直せませんでした（結果は未確認です）。"}
                </p>
              ) : null}
            </div></div>
        </>
      )}
      <p className="small muted">すべて読み取った値の表示です。取得できない項目は「未取得」と表示し、0 や空で代用しません。</p>
    </div>
  );
}

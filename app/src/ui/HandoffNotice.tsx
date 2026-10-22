// 継続チャット（DESIGN_CONT §7）の入力欄まわり: 使用率チップ、目的メモの行、引き継ぎの案内行。
// 規則: 引き継ぎは利用者の送信操作の中だけ（ここは案内と状態の表示。AgentDock から勝手に送らない）。
//       案内は「資料を確認できた」「止める理由がない」ときだけ送信ボタンを「引き継いで送信」にする。確認できない・止める理由があるときは通常の「送信」のまま。
//       閾値の案内は Known かつ live の使用率のときだけ。「このthreadで続ける」はメモリだけ（再起動で戻る）。
import { useEffect, useRef, useState } from "react";
import type { Chat, ChatKey, ChatLocalView, ContinuationSettings } from "../ipc/types";
import type { HandoffPreview } from "../ipc/types";
import * as host from "../ipc/client";
import { handoffIneligible, noticeText, purposeFirstLine, shouldSuggest, statsText, usageChip } from "./contUsage";

export type PreviewState = { kind: "loading" } | { kind: "error"; message: string } | { kind: "ok"; p: HandoffPreview };

export interface HandoffUi {
  /** 案内行を出すか。 */
  shown: boolean;
  mode: "threshold" | "manual" | null;
  /** 引き継ぎの操作を出せない理由（出せるときは null）。 */
  ineligible: string | null;
  preview: PreviewState;
  /** 送信ボタン・送信キーを「引き継いで送信」にするか（資料を確認でき、止める理由がなく、作業中でないとき）。 */
  armed: boolean;
  percent: number | null;
  rounds: number;
  keep: () => void;
  close: () => void;
  reload: () => void;
}

const LOADING: PreviewState = { kind: "loading" };

/** 入力欄の引き継ぎ案内の状態。チャットごとに「続ける」「手動で開いた」を覚える（メモリだけ）。 */
export function useHandoff(a: { chat: Chat; local: ChatLocalView | undefined; settings: ContinuationSettings; running: boolean; worktree: boolean; load: ((chat: ChatKey) => Promise<HandoffPreview>) | undefined }): HandoffUi {
  const { chat, local, settings, running, worktree, load } = a;
  const id = chat.key.id;
  const [declined, setDeclined] = useState<Record<string, number>>({});
  const [manual, setManual] = useState<Record<string, boolean>>({});
  const [tick, setTick] = useState(0);
  // 作業が終わるたびに増やす（送信・実行の完了後は資料が古くなるので、1回だけ取り直す）
  const [doneCount, setDoneCount] = useState(0);
  const wasRunning = useRef(running);
  useEffect(() => { if (wasRunning.current && !running) setDoneCount((n) => n + 1); wasRunning.current = running; }, [running]);
  const [pv, setPv] = useState<{ id: string; key: string; state: PreviewState }>({ id: "", key: "", state: LOADING });
  const usage = local?.contextUsage ?? { kind: "notFetched" as const };
  const ineligible = handoffIneligible({ orgOwned: !!local?.org && !local.org.released, supersededBy: !!local?.supersededBy, worktree });
  // 設定「継続チャット」がオフのあいだは、案内・導線・資料の読み込みをすべて止める
  const on = settings.enabled;
  const suggest = on && ineligible === null && shouldSuggest(usage, settings.suggestAtPercent, declined[id] ?? null);
  // 手動の案内は、理由があっても出す（理由を見せる）。閾値の案内は、引き継げないチャットでは出さない
  const mode: HandoffUi["mode"] = !on ? null : suggest ? "threshold" : manual[id] ? "manual" : null;
  const active = mode !== null && ineligible === null && !!load;
  // 「…」メニュー「新しい thread へ引き継ぐ…」（App が "agentdock:handoff-open" を送る）
  useEffect(() => {
    const h = () => setManual((m) => ({ ...m, [id]: true }));
    window.addEventListener("agentdock:handoff-open", h);
    return () => window.removeEventListener("agentdock:handoff-open", h);
  }, [id]);
  const purposeRev = local?.conversation?.purpose?.revision ?? 0;
  useEffect(() => {
    // 作業中は読まない（結果を使わず、履歴の全件読みが重い）。終わったときに1回だけ読む
    if (!active || !load || running) return;
    // 同じ入力（チャット・目的メモの版・往復数の設定・再確認の回数）では取り直さない。使用率の更新は起点にしない
    const key = `${id}|${purposeRev}|${settings.rounds}|${tick}|${doneCount}`;
    if (pv.id === id && pv.key === key && pv.state.kind !== "loading") return;
    let alive = true;
    // 前の確認結果がある間は、それを見せたまま取り直す（ボタンの文言を点滅させない）。取り直しに失敗したら失敗として扱う
    setPv((cur) => (cur.id === id && cur.state.kind === "ok" ? cur : { id, key, state: LOADING }));
    load(chat.key).then((p) => { if (alive) setPv({ id, key, state: { kind: "ok", p } }); }).catch((e) => { if (alive) setPv({ id, key, state: { kind: "error", message: host.asIpcError(e).message } }); });
    return () => { alive = false; };
    // 資料が古くなる条件（チャット切替・目的メモ変更・往復数の設定変更・送信の完了＝作業の終了・「再確認」）だけで取り直す（読み取りのみ）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, id, purposeRev, settings.rounds, running, tick, doneCount]);
  const preview: PreviewState = pv.id === id ? pv.state : LOADING;
  const armed = active && !running && preview.kind === "ok" && preview.p.blockers.length === 0;
  const percent = usage.kind === "known" ? usage.percent : null;
  return {
    shown: on && (mode !== null || (ineligible !== null && worktreeReasonOnly(usage, settings, ineligible, worktree))),
    mode, ineligible, preview, armed, percent,
    rounds: preview.kind === "ok" ? preview.p.roundsIncluded : settings.rounds,
    keep: () => { setDeclined((d) => ({ ...d, [id]: percent ?? 0 })); setManual((m) => ({ ...m, [id]: false })); },
    close: () => setManual((m) => ({ ...m, [id]: false })),
    reload: () => setTick((t) => t + 1),
  };
}

/** 閾値に達しているが worktree のため引き継げないときだけ、理由を1行で見せる（操作は出さない）。 */
function worktreeReasonOnly(usage: ChatLocalView["contextUsage"], settings: ContinuationSettings, ineligible: string | null, worktree: boolean): boolean {
  return worktree && ineligible !== null && shouldSuggest(usage, settings.suggestAtPercent, null);
}

/** 入力欄の上の案内行。 */
export function HandoffNotice({ ui, onPreview, onDraft }: { ui: HandoffUi; onPreview: () => void; onDraft?: () => void }) {
  if (!ui.shown) return null;
  if (ui.ineligible !== null) {
    return <div className="handoff-note muted" role="status">{ui.mode === null ? `文脈が ${ui.percent}% です。` : ""}{ui.ineligible}。{ui.mode === "manual" ? <button className="small" aria-label="案内を閉じる" onClick={ui.close}>×</button> : null}</div>;
  }
  const mode = ui.mode ?? "manual";
  const pv = ui.preview;
  const blockers = pv.kind === "ok" ? pv.p.blockers : [];
  return (
    <div className="handoff-note" role="status">
      <div className="hn-line">
        <span className="hn-text">{noticeText(mode, ui.percent, ui.rounds)}</span>
        <span className="hn-acts">
          <button className="small" onClick={onPreview}>資料を見る</button>
          {mode === "threshold" && onDraft ? <button className="small" title="更新案のダイアログを開きます（作成は、ダイアログの「作成する」を押したときだけ）" onClick={onDraft}>目的メモを更新</button> : null}
          {mode === "threshold" ? <button className="small" onClick={ui.keep}>このthreadで続ける</button> : <button className="small" aria-label="案内を閉じる" title="案内を閉じる" onClick={ui.close}>×</button>}
        </span>
      </div>
      {pv.kind === "loading" ? <div className="hn-sub muted">資料を確認しています。確認できるまで、送信は通常の送信です。</div> : null}
      {pv.kind === "error" ? <div className="hn-sub why">資料を確認できませんでした（{pv.message}）。確認できるまで、送信は通常の送信です。<button className="small" onClick={ui.reload}>再確認</button></div> : null}
      {blockers.length > 0 ? (
        <div className="hn-sub why">
          いまは引き継げません。送信は通常の送信です（引き継がずに送るか、待つかを選べます）。
          <ul>{blockers.map((b, i) => <li key={i}>{b.message}</li>)}</ul>
          <button className="small" onClick={ui.reload}>再確認</button>
        </div>
      ) : null}
      {pv.kind === "ok" ? (
        <>
          {pv.p.notes.length > 0 ? <ul className="hn-sub muted">{pv.p.notes.map((t, i) => <li key={i}>{t}</li>)}</ul> : null}
          <div className="hn-sub muted">{statsText(pv.p)}</div>
        </>
      ) : null}
    </div>
  );
}

/** 補助行の「文脈 62%」チップ。ツールチップに使用量と観測時刻。 */
export function UsageChip({ local, settings }: { local: ChatLocalView | undefined; settings: ContinuationSettings }) {
  if (!settings.enabled) return null;
  const c = usageChip(local?.contextUsage ?? { kind: "notFetched" }, settings.suggestAtPercent);
  return <span className={`small ctxchip ${c.tone}`} title={c.tip} aria-label={`${c.text}。${c.tip}`}>{c.text}</span>;
}

/** 入力欄の下の目的メモの行（設定済みのときだけ）。押すと編集。 */
export function PurposeLine({ local, onEdit, enabled }: { local: ChatLocalView | undefined; onEdit: () => void; enabled: boolean }) {
  if (!enabled) return null;
  const note = local?.conversation?.purpose;
  if (!note) return null;
  const first = purposeFirstLine(note.text);
  return (
    <div className="purposeline small">
      <button className="linkish" title="目的メモを編集" onClick={onEdit}>目的メモ: {first ?? "（空）"}</button>
      {local?.conversation?.purposeDue ? <span className="muted" title="この thread で最初の送信・メモの変更後・圧縮の観測後の、手動の送信に添えます">次の送信に目的メモを添えます</span> : null}
    </div>
  );
}

// side相談の専用パネル（#10、段階③ P3-4）。主会話の最後の終端turnまでを、読取り専用の一時の分岐として開いた相談。
// 規則: 一覧・主会話のキュー・子孫には入らない（主会話の作業はそのまま続く）。承認・質問はここで回答する。
//       引渡しは選んだ発言を主会話の入力欄へ入れるだけ（自動送信しない）。送信は再送しない（受理不明は「確認できない」と示す）。
//       閉じた後・再起動後は「終了（再開不可）」の記録として読み取り専用で開く。
import { useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { HostSnapshot, OpCapability, PendingRequest, RequestAnswer, SideEntry, SideSessionMeta } from "../ipc/types";
import { RequestCard } from "./CenterPane";
import { Icon } from "./Icon";
import { UnverifiedTag } from "./PrefsDialogs";
import { STATE } from "./format";

const errText = (e: unknown) => host.asIpcError(e).message;

export interface SidePanelProps {
  snap: HostSnapshot;
  side: SideSessionMeta;
  live: boolean;
  cap: OpCapability | undefined;
  onClose: () => void;
  onRespond: (r: PendingRequest, a: RequestAnswer) => void;
  /** 主会話の入力欄へ文章を入れる（送信はしない）。 */
  onInsertMain: (text: string) => void;
  say: (text: string) => void;
}

export function SidePanel({ snap, side, live, cap, onClose, onRespond, onInsertMain, say }: SidePanelProps) {
  const [entries, setEntries] = useState<SideEntry[]>([]);
  const [running, setRunning] = useState(false);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [picked, setPicked] = useState<number[]>([]);
  const open = side.state.kind === "open";
  const view = snap.agents.find((a) => a.agent.key.id === side.thread.id);
  const reqs = snap.requests.filter((r) => r.chat.id === side.thread.id && r.state.kind === "pending");
  // 状態・活動・要求が変わったら取り直す（確定した発言の記録は、ホストが作る）。
  const stamp = `${side.state.kind}|${view?.status.state ?? ""}|${view?.status.turn ?? ""}|${view?.currentActivity?.key.itemId ?? ""}|${reqs.length}`;

  const load = () => {
    if (!live) return;
    host.readSide(side.id).then((t) => { setEntries(t.entries); setRunning(t.running); setLoadErr(null); }).catch((e) => setLoadErr(errText(e)));
  };
  useEffect(() => { load(); }, [side.id, stamp, live]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { setPicked([]); setNote(null); }, [side.id]);

  const send = async () => {
    const text = draft.trim();
    if (!text || busy || !open) return;
    setBusy(true); setNote(null);
    try {
      const ack = await host.sendSide(side.id, text);
      if (ack.kind === "accepted") { setDraft(""); }
      else if (ack.kind === "rejected") setNote(`送れませんでした: ${ack.message}`);
      else setNote(`${ack.message}（入力欄の内容は残してあります）`);
    } catch (e) { setNote(`送れませんでした: ${errText(e)}`); } finally { setBusy(false); load(); }
  };

  const close = async () => {
    if (!open) { onClose(); return; }
    const interrupt = running || view?.status.state === "running" || view?.status.state === "waiting";
    if (interrupt && !window.confirm("相談が実行中です。中断を要求して閉じますか？\n中断は要求であり、停止はまだ確認できていない状態になります（確認できるまで終了・削除を止めます）。")) return;
    try {
      await host.closeSide(side.id, interrupt);
      say(interrupt ? "相談を閉じ、中断を要求しました。停止は確認中です。記録は残ります。" : "相談を閉じました。記録は残ります（再開はできません）。");
      onClose();
    } catch (e) { setNote(`閉じられませんでした: ${errText(e)}`); }
  };

  const handoff = async () => {
    if (picked.length === 0) return;
    try {
      const r = await host.handoffSide(side.id, [...picked].sort((a, b) => a - b));
      onInsertMain(r.text);
      setPicked([]);
      say(`選んだ発言を主会話の入力欄へ入れました（${r.chars.toLocaleString("ja-JP")} 文字。送信はしていません）。`);
    } catch (e) { setNote(`渡せませんでした: ${errText(e)}`); }
  };

  const stateText = open
    ? (view ? STATE[view.status.state].t : "状態不明")
    : `終了（再開不可）${side.state.kind === "ended" ? `：${side.state.reason}` : ""}`;
  const toggle = (i: number) => setPicked((p) => (p.includes(i) ? p.filter((x) => x !== i) : [...p, i]));

  return (
    <aside className="sidepanel" role="complementary" aria-label="side相談">
      <header>
        <b className="grow">side相談 <UnverifiedTag cap={cap} /></b>
        <button className="small" onClick={() => void close()} title={open ? "相談を閉じます（記録は残ります）" : "この記録を閉じます"}>{open ? "相談を閉じる" : "閉じる"}</button>
        <button aria-label="パネルを隠す" title="パネルを隠します（相談は続きます）" onClick={onClose}><Icon name="x" /></button>
      </header>
      <div className="small muted sp-note">
        読み取り専用の一時的な相談です。主会話の作業・送信待ちには影響せず、一覧にも出ません。{open ? "アプリの終了や接続の切断で終了し、再開はできません。" : "読み取り専用の記録です。"}
      </div>
      <div className="small sp-state">状態: {stateText}{running ? "（作業中）" : ""}</div>
      <div className="sp-body">
        {loadErr ? <div className="why err">記録を取得できませんでした: {loadErr}</div> : null}
        {entries.length === 0 && !loadErr ? <p className="small muted">まだ発言がありません。</p> : null}
        {entries.map((e, i) => (
          <div key={i} className={`sp-msg ${e.role === "user" ? "u" : "a"}`}>
            <label className="small muted"><input type="checkbox" checked={picked.includes(i)} onChange={() => toggle(i)} aria-label="主会話へ渡す発言として選ぶ" /> {e.role === "user" ? "あなた" : "side のエージェント"}{e.truncated ? "（短縮されている可能性）" : ""}</label>
            <div className="sp-text">{e.text}</div>
          </div>
        ))}
        {reqs.map((r) => <RequestCard key={`${r.key.source}:${r.key.requestId}`} r={r} onRespond={onRespond} />)}
      </div>
      {picked.length > 0 ? (
        <div className="sp-act"><button className="btn-line" onClick={() => void handoff()}>選んだ{picked.length}件を主会話の入力欄へ（送信しません）</button></div>
      ) : null}
      {open ? (
        <div className="sp-in">
          <textarea aria-label="sideへのメッセージ" rows={3} value={draft} disabled={busy} onChange={(e) => setDraft(e.target.value)} placeholder="この相談に質問する（主会話には送られません）"
            onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey && !e.nativeEvent.isComposing) { e.preventDefault(); void send(); } }} />
          <div className="sp-btn"><span className="small muted grow">Ctrl＋Enter で送信</span><button className="btn-main send" aria-label="sideへ送信" disabled={busy || !draft.trim() || running} title={running ? "実行中です。完了を待つか、閉じて中断してください" : undefined} onClick={() => void send()}><Icon name="send" /></button></div>
        </div>
      ) : null}
      {note ? <div className="small sp-note why err">{note}</div> : null}
    </aside>
  );
}

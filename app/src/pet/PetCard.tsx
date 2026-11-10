// ペットの小さなカード（簡易チャット・承認/質問への回答・右クリックメニュー）。通常チャットの部品は使わず、やわらかい見た目で作る。
// 回答・送信は通常画面と同じホストのコマンド（respond_request / send_message）を、利用者が押したときだけ呼ぶ。自動回答・自動再送はしない。
import { useEffect, useState } from "react";
import type { Chat, ChatKey, Known, HostSnapshot, PendingRequest, PetEntry, PetSettings, QuestionAnswer, RequestAnswer, TurnRecord } from "../ipc/types";
import * as host from "../ipc/client";
import { chatName, isRunning } from "../ui/derive";
import { sendLockReason } from "../ui/CenterPane";
import { GAP_TEXT } from "../ui/org";
import { answersAtOnce, answersComplete, ARM_MS, clip, isArmed, latestReply, optionTone, pauseLeftText, recentChats } from "./cardLogic";

const known = (k: Known<string> | undefined): string | null => (k && k.kind === "value" && k.value.trim() !== "" ? k.value : null);

function useTick(active: boolean, ms: number): number {
  const [n, setN] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setN(Date.now()), ms);
    return () => window.clearInterval(t);
  }, [active, ms]);
  return n;
}

// ── 承認・質問への回答 ──
export function RequestPane({ r, chats, onOpenChat, onFinished, onToChat }: {
  r: PendingRequest; chats: Chat[]; onOpenChat: (k: ChatKey) => void; onFinished: (text: string) => void; onToChat: () => void;
}) {
  const id = `${r.key.source}/${r.key.requestId}`;
  const [shown, setShown] = useState<{ id: string; at: number }>(() => ({ id, at: Date.now() }));
  const [busy, setBusy] = useState(false);
  const [locked, setLocked] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [picked, setPicked] = useState<Record<string, QuestionAnswer | undefined>>({});
  // 別の要求に替わったら、表示時刻・入力・ロックを作り直す（0.5秒は押せない）。
  useEffect(() => {
    setShown({ id, at: Date.now() }); setBusy(false); setLocked(false); setNote(null); setPicked({});
  }, [id]);
  const at = shown.id === id ? shown.at : null;
  const now = useTick(!isArmed(at, Date.now()), 100);
  const armed = isArmed(at, now);
  const can = armed && !busy && !locked;

  const d = r.detail;
  const chat = chats.find((c) => c.key.id === r.chat.id);
  const isQuestion = r.kind.kind === "userInput" && d.questions.length > 0;
  const title = r.kind.kind === "userInput" ? "質問があります" : r.kind.kind === "toolElicitation" ? "MCPの許可の確認です" : "許可のお願いです";

  const respond = async (a: RequestAnswer) => {
    if (!can) return;
    setBusy(true);
    try {
      const out = await host.respondRequest(r.key, a);
      if (out.kind === "delivered") onFinished("回答を送りました");
      else if (out.kind === "alreadyResolved") onFinished("すでに解決済みでした（回答は送っていません）");
      else if (out.kind === "notDeliverable") { setLocked(true); setNote(GAP_TEXT[out.cause]); }
      else { setLocked(true); setNote(`回答の受理を確認できません（${out.message}）。再送はしません。チャットで確認してください。`); }
    } catch (e) { setNote(`回答を送れませんでした: ${host.asIpcError(e).message}`); }
    finally { setBusy(false); }
  };
  const decide = (optionId: string) => void respond({ kind: "decision", optionId });
  const submitAnswers = () => void respond({ kind: "answers", answers: d.questions.map((q) => picked[q.id]!) });
  const pick = (qid: string, a: QuestionAnswer) => {
    setPicked((p) => ({ ...p, [qid]: a }));
    if (answersAtOnce(d.questions)) void respond({ kind: "answers", answers: [a] });
  };

  const rows: Array<[string, string, boolean]> = [];
  const add = (label: string, k: Known<string> | undefined, mono = false) => { const v = known(k); if (v) rows.push([label, v, mono]); };
  add("理由", d.reason);
  add("コマンド", d.command, true);
  add("作業フォルダ", d.cwd, true);
  const files = d.files.kind === "value" && d.files.value.length > 0 ? d.files.value.join("\n") : null;
  if (files) rows.push(["ファイル", files, true]);
  add("追加の権限", d.extraPermissions);
  add("URL", d.url, true);
  add("サーバー", d.server);

  return (
    <div className="pc-req" role="group" aria-label={title}>
      <div className="pc-head">
        <span className="pc-title">{title}</span>
        {chat ? <button type="button" className="pc-chip" title="このチャットを通常画面で開く" onClick={() => onOpenChat(r.chat)}>{clip(chatName(chat), 18)}</button> : null}
      </div>
      <div className="pc-scroll" tabIndex={0} aria-label="要求の内容">
        <div className="pc-summary">{d.summary}</div>
        {rows.map(([label, v, mono]) => (
          <div key={label} className="pc-row"><span className="pc-label">{label}</span><span className={mono ? "pc-val mono" : "pc-val"}>{v}</span></div>
        ))}
        {isQuestion ? d.questions.map((q) => (
          <div key={q.id} className="pc-q">
            {q.header ? <div className="pc-label">{q.header}</div> : null}
            <div className="pc-qtext">{q.text}</div>
            <div className="pc-opts">
              {q.options.map((o) => (
                <button key={o.id} type="button" className={`pc-pill ${picked[q.id]?.optionId === o.id ? "picked" : "quiet"}`} disabled={!can} title={o.description ?? undefined}
                  onClick={() => pick(q.id, { questionId: q.id, optionId: o.id, text: null })}>{o.label}</button>
              ))}
            </div>
            {q.allowsFreeText ? (
              <input className="pc-input" type={q.secret ? "password" : "text"} autoComplete="off" placeholder="ここに入力" aria-label={`${q.text}への回答`} disabled={!can}
                value={picked[q.id]?.text ?? ""} onChange={(e) => setPicked((p) => ({ ...p, [q.id]: { questionId: q.id, optionId: null, text: e.target.value } }))} />
            ) : null}
          </div>
        )) : null}
      </div>
      {note ? <div className="pc-note warn" role="alert">{note}</div> : null}
      {isQuestion ? (
        answersAtOnce(d.questions) ? null : (
          <div className="pc-actions"><button type="button" className="pc-pill primary" disabled={!can || !answersComplete(d.questions, picked)} onClick={submitAnswers}>回答する</button></div>
        )
      ) : (
        <div className="pc-actions" title="選択肢は Codex が示したものです">
          {r.options.map((o) => (
            <button key={o.id} type="button" className={`pc-pill ${optionTone(o)}`} disabled={!can} title={[o.description, o.scope === "session" ? "この会話のあいだ" : o.scope === "persistent" ? "今後も（保存されます）" : null].filter(Boolean).join("／") || undefined}
              onClick={() => decide(o.id)}>{o.label}</button>
          ))}
          {r.options.length === 0 ? <span className="pc-hint">この要求はここでは答えられません。チャットで開いてください。</span> : null}
        </div>
      )}
      {!armed && !locked ? <div className="pc-hint">内容を確認してください（{Math.ceil(ARM_MS / 100) / 10}秒ほどで押せます）</div> : null}
      <div className="pc-foot"><button type="button" className="pc-link" onClick={onToChat}>チャットに送る</button></div>
    </div>
  );
}

// ── 簡易チャット ──
/** 受理を確認できなかった送り先。確認できるまでここからは送らない（再送しない）。ウィンドウの存続中だけ持つ。 */
const unknownSent = new Set<string>();

export function ChatPane({ snap, turns, targetId, onPick, onOpenChat, pendingCount, onToRequest }: {
  snap: HostSnapshot; turns: Record<string, TurnRecord[]>; targetId: string | null; onPick: (id: string) => void; onOpenChat: (k: ChatKey) => void;
  pendingCount: number; onToRequest: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ tone: "ok" | "warn"; text: string } | null>(null);
  const chat = snap.chats.find((c) => c.key.id === targetId);
  const options = recentChats(snap.chats, targetId);
  const running = chat ? isRunning(snap, chat) : false;
  const lock = chat ? (sendLockReason(snap, chat, snap.chatLocals.find((l) => l.chat.id === chat.key.id))
    ?? (unknownSent.has(chat.key.id) ? "前の送信を受理したか確認できていません。再送はしません。チャットで確認してください" : null)) : "送り先のチャットを選んでください";
  const reply = latestReply(targetId ? turns[targetId] : undefined);
  useEffect(() => { setStatus(null); }, [targetId]);

  const send = async () => {
    const body = text.trim();
    if (!chat || !body || busy || lock) return;
    setBusy(true);
    setStatus(null);
    try {
      const a = await host.sendMessage(chat.key, body, running ? "steer" : "newTurn", [], null);
      const s = a.state;
      if (s.kind === "accepted") { setText(""); setStatus({ tone: "ok", text: running ? "追加指示を送りました" : "送信しました" }); }
      else if (s.kind === "rejected") setStatus({ tone: "warn", text: `送れませんでした: ${s.message}` });
      else { unknownSent.add(chat.key.id); setStatus({ tone: "warn", text: "受理されたか確認できません。再送はしません。チャットで確認してください" }); }
    } catch (e) { setStatus({ tone: "warn", text: `送れませんでした: ${host.asIpcError(e).message}` }); }
    finally { setBusy(false); }
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
    const ctrl = snap.settings.sendKey === "ctrlEnter";
    if (ctrl ? e.ctrlKey : !e.shiftKey && !e.ctrlKey) { e.preventDefault(); void send(); }
  };

  return (
    <div className="pc-chat" role="group" aria-label="ペットから送る">
      <div className="pc-head">
        <span className="pc-title">送り先</span>
        <select className="pc-select" aria-label="送り先のチャット" value={targetId ?? ""} onChange={(e) => onPick(e.target.value)}>
          {!chat ? <option value="">（選んでください）</option> : null}
          {options.map((c) => <option key={c.key.id} value={c.key.id}>{clip(chatName(c), 24)}</option>)}
        </select>
      </div>
      {pendingCount > 0 ? <button type="button" className="pc-banner" onClick={onToRequest}>回答待ちが{pendingCount}件あります</button> : null}
      <div className="pc-reply" aria-label="最新の返答">
        {reply ? <><div className="pc-reply-text">{clip(reply.text, 280)}</div>{reply.inProgress ? <div className="pc-hint">書いている途中です</div> : null}</>
          : <div className="pc-hint">{chat ? "このウィンドウで受け取った返答はまだありません" : "送り先を選ぶと、返答がここに出ます"}</div>}
        {chat ? <button type="button" className="pc-link" onClick={() => onOpenChat(chat.key)}>チャットで開く</button> : null}
      </div>
      <textarea className="pc-input pc-text" rows={2} value={text} disabled={!!lock || busy} placeholder={lock ?? (running ? "追加の指示を書く" : "ひとこと送る")} aria-label="送る内容"
        onChange={(e) => setText(e.target.value)} onKeyDown={onKey} />
      <div className="pc-actions">
        {running && !lock ? <span className="pc-hint">作業中なので、追加指示として送ります</span> : <span />}
        <button type="button" className="pc-pill primary" disabled={!chat || !text.trim() || busy || !!lock} onClick={() => void send()}>{running ? "追加で送る" : "送る"}</button>
      </div>
      {status ? <div className={`pc-note ${status.tone}`} role="status">{status.text}</div> : null}
    </div>
  );
}

// ── 右クリックメニュー ──
const SIZES = [50, 75, 100, 125, 150];

export function MenuPane({ pet, pausedUntil, onSet, onPause, onHide, onOpenMain }: {
  pet: PetSettings; pausedUntil: number | null; onSet: (p: PetSettings) => void; onPause: (minutes: number | null) => void; onHide: () => void; onOpenMain: () => void;
}) {
  const [list, setList] = useState<PetEntry[] | null>(null);
  useEffect(() => { host.petList().then(setList).catch(() => setList([])); }, []);
  const now = useTick(pausedUntil !== null, 15_000);
  const left = pauseLeftText(pausedUntil, now);
  return (
    <div className="pc-menu" role="menu" aria-label="ペットのメニュー">
      <div className="pc-scroll">
        <button type="button" role="menuitem" className="pc-item" onClick={onOpenMain}>AgentDockを開く</button>
        <div className="pc-sec">通知</div>
        {left ? <div className="pc-hint">一時停止中（{left}）。承認・質問は届きます</div> : null}
        <div className="pc-opts">
          <button type="button" role="menuitem" className="pc-pill quiet" onClick={() => onPause(30)}>30分止める</button>
          <button type="button" role="menuitem" className="pc-pill quiet" onClick={() => onPause(60)}>1時間止める</button>
          {left ? <button type="button" role="menuitem" className="pc-pill primary" onClick={() => onPause(null)}>再開</button> : null}
        </div>
        <div className="pc-sec">大きさ</div>
        <div className="pc-opts">
          {SIZES.map((s) => <button key={s} type="button" role="menuitemradio" aria-checked={pet.scalePercent === s} className={`pc-pill ${pet.scalePercent === s ? "picked" : "quiet"}`} onClick={() => onSet({ ...pet, scalePercent: s })}>{s}%</button>)}
        </div>
        <div className="pc-sec">目線</div>
        <label className="pc-check"><input type="checkbox" checked={pet.followGaze} onChange={(e) => onSet({ ...pet, followGaze: e.target.checked })} />ときどきカーソルを見る</label>
        <div className="pc-sec">ペットを変える</div>
        {list === null ? <div className="pc-hint">読み込み中…</div> : list.length === 0 ? <div className="pc-hint">ペットがありません</div> : (
          <div className="pc-list">
            {list.map((p) => (
              <button key={p.folder} type="button" role="menuitemradio" aria-checked={pet.selected === p.folder} disabled={!p.supported} title={p.unsupportedReason ?? undefined}
                className={`pc-item ${pet.selected === p.folder ? "picked" : ""}`} onClick={() => onSet({ ...pet, selected: p.folder })}>
                {p.displayName}{p.supported ? "" : "（非対応）"}
              </button>
            ))}
          </div>
        )}
        <button type="button" role="menuitem" className="pc-item quiet" onClick={onHide}>ペットを隠す</button>
      </div>
    </div>
  );
}

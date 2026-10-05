import { useState } from "react";
import type {
  Attachment, Chat, ChatModelSettings, DecisionOption, HostSnapshot, ModelInfo, PendingRequest, QueueItem, RequestAnswer,
  SaveState, StopRecord, TurnRecord,
} from "../ipc/types";
import { Icon } from "./Icon";
import { chatAgents, chatName, chatRequests, chatStatusText, chatStops, isRunning, rootView, stopOpen } from "./derive";
import { FRESH, STOP_LABEL, hms, holdText, keyStr, knownValue, showKnown } from "./format";

const SCOPE_TEXT = { once: "1回だけ", session: "このセッション中", persistent: "以後ずっと（永続）", unknown: "効力範囲は不明" } as const;

export interface CenterProps {
  snap: HostSnapshot;
  chat: Chat;
  turns: TurnRecord[];
  attachments: Attachment[];
  settings: ChatModelSettings | undefined;
  models: ModelInfo[];
  save: SaveState | null;
  externalLabel: string | undefined;
  enterMode: "ctrl" | "enter";
  draft: string;
  setDraft: (v: string) => void;
  onAct: (a: string) => void;
  onRespond: (r: PendingRequest, a: RequestAnswer) => void;
  /** 依頼を受け付けたら true（下書きを消す）。拒否・前提不足なら false（下書きを残す）。 */
  onSend: (text: string, steer: boolean) => boolean | Promise<boolean>;
  /** モデル・推論の強さの選択（次のターンから適用。受理済みは別に表示）。 */
  onModel: (model: string, effort: string) => void;
  /** 直近の送信の受理状態の案内（受理なし・受理不明・照合の結果）。 */
  notice: SendNotice | null;
  onRetrySend: () => void;
}

export interface SendNotice { cls: "warn" | "err"; title: string; body: string; canRetry: boolean }

function Header({ snap, chat, onAct, running }: { snap: HostSnapshot; chat: Chat; onAct: (a: string) => void; running: boolean }) {
  const root = rootView(snap, chat);
  const fresh = FRESH[root?.freshness ?? "needsReconcile"];
  const cwd = knownValue(chat.cwd);
  return (
    <div className="chead">
      <button className="only-narrow" aria-label="チャット一覧を開く" onClick={() => onAct("toggleLeft")}><Icon name="list" /></button>
      <div className="grow">
        <div className="ttl">{chatName(chat)}</div>
        <div className="meta">
          <span className="tag ai" title="この会話のAI">Codex</span>
          {cwd ? <span className="mono" title="作業フォルダ">{cwd}</span> : <span>一般チャット（作業フォルダなし）</span>}
          {/* 鮮度と状態は別表示。片方から他方を導かない */}
          <span className={`fresh ${fresh.c}`} title="収集の鮮度。エージェントの状態とは別">{fresh.t}</span>
          <span>{chatStatusText(snap, chat)}</span>
        </div>
      </div>
      {running ? <button className="btn-line" title="Esc" onClick={() => onAct("interrupt")}><Icon name="stop" />中断</button> : null}
      <button aria-label="差分を確認" onClick={() => onAct("diff")}><Icon name="diff" /><span className="hide-s">差分</span></button>
      <button className="only-narrow" aria-label="エージェントのドックを開く" onClick={() => onAct("toggleRight")}><Icon name="dock" /></button>
    </div>
  );
}

function targetName(snap: HostSnapshot, c: Chat, t: StopRecord["targets"][number]): string {
  const ref = t.target;
  if (ref.kind === "turn") {
    const v = chatAgents(snap, c).find((x) => keyStr(x.agent.key) === keyStr(ref.turn.agent));
    return v ? showKnown(v.agent.displayName) : ref.turn.agent.id;
  }
  if (ref.kind === "managedExec") return `${showKnown(chatAgents(snap, c).find((x) => keyStr(x.agent.key) === keyStr(ref.agent))?.agent.displayName ?? { kind: "notFetched" })} の管理実行（${ref.execId}）`;
  return `OSプロセス PID ${ref.pid}`;
}

function StopBanner({ snap, chat, rec, onAct }: { snap: HostSnapshot; chat: Chat; rec: StopRecord; onAct: (a: string) => void }) {
  return (
    <div className="cbanner warn" role="alert">
      <h4>停止を確認できていない対象があります（中断要求後・ホストの判定）</h4>
      <ul className="targets">
        {rec.targets.map((t, i) => (
          <li key={i}><b>{targetName(snap, chat, t)}</b><span>{STOP_LABEL[t.summary]}</span></li>
        ))}
      </ul>
      <div style={{ marginTop: 4 }}>時間の経過だけで停止や失敗とは判断しません。停止が確認できるまで、削除と新しい送信は保留します。</div>
      <div className="acts">
        <button className="btn-line" onClick={() => onAct("stub")}>待つ</button>
        <button className="btn-line" onClick={() => onAct("interrupt")}>中断を再試行</button>
        <button className="btn-danger" onClick={() => onAct("force")}>強制終了…</button>
      </div>
    </div>
  );
}

function Banners({ p }: { p: CenterProps }) {
  const { snap, chat, onAct } = p;
  const root = rootView(snap, chat);
  const stops = chatStops(snap, chat).filter(stopOpen);
  return (
    <>
      {p.notice ? (
        <div className={`cbanner ${p.notice.cls}`} role="alert">
          <h4>{p.notice.title}</h4>
          {p.notice.body}
          {p.notice.canRetry ? <div className="acts"><button className="btn-line" onClick={p.onRetrySend}>もう一度送る</button></div> : null}
        </div>
      ) : null}
      {chat.origin === "external" ? (
        <div className="cbanner warn">
          <h4>{p.externalLabel ?? "外部"} で作成された会話です（閲覧のみ）</h4>
          外部でまだ実行中かどうか確認できていません。外部での実行が終わったことを確認してから、このアプリで続きを送れます。元の作業フォルダやファイルは参照のみで、削除の対象にしません。
          <div className="acts"><button className="btn-line" onClick={() => onAct("stub")}>実行状態を確認</button><button className="btn-line" onClick={() => onAct("stub")}>終了を確認したので再開する</button></div>
        </div>
      ) : null}
      {root && root.freshness === "historyOnly" && chat.origin !== "external" ? (
        <div className="cbanner info" role="alert">
          <h4>接続を回復しましたが、ライブ監視はまだ戻っていません</h4>
          保存履歴から取得した状態です（{hms(root.status.evidence.observedAt)}）。実行中の途中経過は表示されません。送信待ちは照合が終わるまで保留します。
          <div className="acts"><button className="btn-line" onClick={() => onAct("stub")}>状態を再照合</button><button className="btn-line" onClick={() => onAct("stub")}>保存履歴を再取得</button></div>
        </div>
      ) : null}
      {stops.map((r) => <StopBanner key={r.id} snap={snap} chat={chat} rec={r} onAct={onAct} />)}
      {p.save && p.save.kind === "saveFailed" ? (
        <div className="cbanner err" role="alert">
          <h4>一部を保存できませんでした</h4>
          保存済み: {p.save.savedPart ?? "なし"}。未保存: {p.save.unsavedPart ?? "不明"}（{p.save.message}）。保存できていない内容を保存済みとは表示しません。
          <div className="acts"><button className="btn-line" onClick={() => onAct("stub")}>保存を再試行</button><button className="btn-line" onClick={() => onAct("settings:storage")}>容量を確認</button></div>
        </div>
      ) : null}
    </>
  );
}

function Messages({ turns, reqs, running, onRespond, onAct }: {
  turns: TurnRecord[]; reqs: PendingRequest[]; running: boolean; onRespond: CenterProps["onRespond"]; onAct: (a: string) => void;
}) {
  return (
    <div className="msgs" id="msgs" aria-live="polite">
      {turns.map((t) => {
        const others = t.entries.filter((e) => e.kind.kind !== "userMessage" && e.kind.kind !== "agentMessage");
        return (
          <div key={keyStr(t.key.agent) + t.key.turnId}>
            {t.entries.filter((e) => e.kind.kind === "userMessage" || e.kind.kind === "agentMessage").map((e) => {
              const text = showKnown(e.text);
              if (e.kind.kind === "userMessage") {
                return <div className="msg user" key={e.key.itemId}><div className="who" style={{ justifyContent: "flex-end" }}>あなた</div><div className="bubble">{text}</div></div>;
              }
              return (
                <div className="msg ai" key={e.key.itemId}>
                  <div className="who">Codex</div>
                  <div className="bubble">{text.split("\n\n").map((para, i) => para.startsWith("reconnect()") ? <pre key={i}>{para}</pre> : <p key={i}>{para}</p>)}</div>
                  <div className="acts"><button onClick={() => onAct("stub")}><Icon name="copy" />コピー</button><button onClick={() => onAct("stub")}><Icon name="branch" />ここから分岐</button></div>
                </div>
              );
            })}
            {others.length ? (
              <div className="msg ai">
                <details className="activity"><summary>作業の記録（{others.length}件）</summary>
                  <ul style={{ margin: "4px 0", paddingLeft: 18 }}>{others.map((e) => <li key={e.key.itemId}>{showKnown(e.text)}</li>)}</ul>
                </details>
              </div>
            ) : null}
          </div>
        );
      })}
      {reqs.map((r) => <RequestCard key={keyStr({ backend: r.key.backend, id: r.key.requestId })} r={r} onRespond={onRespond} />)}
      {running && reqs.length === 0 ? <div className="msg ai"><div className="who">Codex</div><div className="activity">作業中です。右のドックで各エージェントの状況を確認できます。</div></div> : null}
    </div>
  );
}

function optClass(o: DecisionOption, first: boolean): string {
  if (o.effect === "deny" || o.effect === "cancel") return "btn-danger";
  return first ? "btn-main" : "btn-line";
}

function RequestCard({ r, onRespond }: { r: PendingRequest; onRespond: CenterProps["onRespond"] }) {
  const isQuestion = r.kind.kind === "userInput" || r.kind.kind === "toolElicitation";
  const d = r.detail;
  return (
    <div className="req" role="group" aria-label={isQuestion ? "質問" : "承認要求"}>
      <div className="h"><Icon name={isQuestion ? "chat" : "alert"} />{d.summary}</div>
      <div className="t">要求元: {r.agent.id}　作業フォルダ: <span className="mono">{showKnown(d.cwd)}</span></div>
      {d.command.kind === "value" ? <code>{d.command.value}</code> : null}
      {d.files.kind === "value" ? <code>{d.files.value.join("\n")}</code> : null}
      {isQuestion ? d.questions.map((q) => (
        <div key={q.id}>
          <div className="t">{q.text}</div>
          <div className="opts">
            {q.options.map((o, i) => (
              <button key={o.id} className={optClass(o, i === 0)} onClick={() => onRespond(r, { kind: "answers", answers: [{ questionId: q.id, optionId: o.id, text: null }] })}>{o.label}</button>
            ))}
          </div>
        </div>
      )) : (
        <div className="opts">
          {r.options.map((o, i) => (
            <button key={o.id} className={optClass(o, i === 0)} title={SCOPE_TEXT[o.scope]} onClick={() => onRespond(r, { kind: "decision", optionId: o.id })}>{o.label}</button>
          ))}
        </div>
      )}
      <div className="scope">選択肢と効力範囲は Codex が示したものを表示します。{!isQuestion ? `（${r.options.map((o) => `${o.label}: ${SCOPE_TEXT[o.scope]}`).join("／")}）` : ""}</div>
    </div>
  );
}

function Queue({ items, onAct }: { items: QueueItem[]; onAct: (a: string) => void }) {
  if (!items.length) return null;
  const held = items.some((q) => q.state.kind !== "waiting");
  return (
    <div className="queue">
      <details open>
        <summary><Icon name="list" /><b>完了後に送る依頼 {items.length}件</b><span className="muted">{held ? "自動送信を止めています" : "親と子孫の作業が終わると、登録順に1件ずつ送ります"}</span></summary>
        {items.map((q, i) => {
          const s = q.state;
          const why = s.kind === "held" ? `保留: ${holdText(s.reason)}`
            : s.kind === "acceptanceUnknown" ? "受理不明: 送信中に接続が切れ、受理されたか不明です。履歴と照合するまで再送しません"
            : s.kind === "sending" ? "送信中" : s.kind === "cancelled" ? "取り消し済み" : s.kind === "sent" ? "送信済み" : null;
          return (
            <div className="qitem" key={q.id}>
              <span className="qnum">{i + 1}</span>
              <div><div>{q.text}</div>{why ? <div className={`why ${s.kind === "acceptanceUnknown" ? "err" : ""}`}>{why}</div> : null}</div>
              <div style={{ display: "flex", gap: 2 }}>
                {/* acceptanceUnknown の間は再送ボタンを出さない。照合だけ */}
                {s.kind === "acceptanceUnknown" ? <button className="small" onClick={() => onAct("stub")}>履歴と照合</button> : <button className="small" onClick={() => onAct("stub")}>編集</button>}
                <button className="small" onClick={() => onAct("stub")}>取消</button>
              </div>
            </div>
          );
        })}
      </details>
    </div>
  );
}

function Composer({ p, running, lock }: { p: CenterProps; running: boolean; lock: string | null }) {
  const { settings, models, attachments } = p;
  const dflt = models.find((m) => m.isDefault) ?? models[0];
  const [model, setModel] = useState(settings?.selected?.model ?? dflt?.id ?? "");
  const [effort, setEffort] = useState(settings?.selected?.effort ?? dflt?.defaultEffort ?? "");
  const [steer, setSteer] = useState(false);
  const accepted = settings?.accepted.kind === "value" ? settings.accepted.value : null;
  const changed = !!accepted && (accepted.model !== model || (accepted.effort ?? "") !== effort);
  const efforts = models.find((m) => m.id === model)?.efforts ?? [];
  const send = async () => {
    if (!p.draft.trim() || lock) return;
    const ok = await p.onSend(p.draft, running && steer);
    if (ok) p.setDraft("");
  };
  const pickModel = (id: string) => {
    const e = models.find((m) => m.id === id)?.defaultEffort ?? "";
    setModel(id); setEffort(e); p.onModel(id, e);
  };
  const pickEffort = (e: string) => { setEffort(e); p.onModel(model, e); };
  return (
    <div className="composer">
      <div className={`shell ${lock ? "locked" : ""}`}>
        {attachments.length ? (
          <div className="chips">
            {attachments.map((f) => {
              const name = f.originalPath.split("\\").pop();
              const bad = f.exists.kind === "value" && !f.exists.value;
              return (
                <span key={f.id} className={`chip ${bad ? "bad" : ""}`}>
                  {f.kind === "image" ? <span className="thumb" title="画像プレビュー（例）" /> : <Icon name="clip" />}
                  <span>{name}</span><span className="st">{bad ? "元ファイルが見つかりません" : "コピー済み"}　{hms(f.attachedAt)}</span>
                  <button aria-label={`${name}を取り外す`} onClick={() => p.onAct("stub")}><Icon name="x" /></button>
                </span>
              );
            })}
          </div>
        ) : null}
        {lock ? <div className="lockmsg">{lock}</div> : null}
        <textarea aria-label="メッセージ" disabled={!!lock} value={p.draft} onChange={(e) => p.setDraft(e.target.value)}
          placeholder={running ? (steer ? "現在の作業への追加指示" : "完了後に送る次の依頼") : "メッセージを入力"}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
            if ((p.enterMode === "ctrl" && e.ctrlKey) || (p.enterMode === "enter" && !e.shiftKey)) { e.preventDefault(); void send(); }
          }} />
        <div className="ctrl">
          <button aria-label="ファイル・画像・参考情報を追加" disabled={!!lock} onClick={() => p.onAct("attach")}><Icon name="clip" /></button>
          <select aria-label="モデル" value={model} onChange={(e) => pickModel(e.target.value)}>{models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select>
          <select aria-label="推論の強さ" value={effort} onChange={(e) => pickEffort(e.target.value)}>{efforts.map((x) => <option key={x.id} value={x.id}>{x.id}</option>)}</select>
          {running ? (
            <span className="seg" role="group" aria-label="実行中の送信方法">
              <button aria-pressed={steer} onClick={() => setSteer(true)}>追加指示</button>
              <button aria-pressed={!steer} onClick={() => setSteer(false)}>完了後に送信</button>
            </span>
          ) : null}
          <span className="grow" />
          <button className="btn-main send" aria-label={running ? (steer ? "追加指示を送る" : "完了後に送る依頼として登録") : "送信"} disabled={!!lock} onClick={() => void send()}><Icon name="send" /></button>
        </div>
      </div>
      <div className="hint">
        <span>{p.enterMode === "ctrl" ? "Ctrl＋Enter で送信、Enter で改行" : "Enter で送信、Shift＋Enter で改行"}</span>
        <span>
          {changed ? `選択: ${model}／${effort}（未受理。次のターンから適用）　` : ""}
          受理済み: {settings ? (accepted ? `${accepted.model}／${accepted.effort ?? "既定"}` : showKnown(settings.accepted)) : "未確認"}
        </span>
      </div>
    </div>
  );
}

export function CenterPane(p: CenterProps) {
  const { snap, chat } = p;
  const running = isRunning(snap, chat);
  const reqs = chatRequests(snap, chat);
  const hasOpenStop = chatStops(snap, chat).some(stopOpen);
  const lock = hasOpenStop ? "停止を確認できるまで、このチャットへの新しい送信は止めています。"
    : chat.origin === "external" ? "外部で実行中かどうか確認できるまで、この会話には送信できません。" : null;
  return (
    <>
      <Header snap={snap} chat={chat} onAct={p.onAct} running={running} />
      <Banners p={p} />
      <Messages turns={p.turns} reqs={reqs} running={running} onRespond={p.onRespond} onAct={p.onAct} />
      <Queue items={snap.queue.filter((q) => keyStr(q.chat) === keyStr(chat.key))} onAct={p.onAct} />
      <Composer key={`${chat.key.id}:${p.models.length}`} p={p} running={running} lock={lock} />
    </>
  );
}

export function EmptyCenter({ onAct }: { onAct: (a: string) => void }) {
  return (
    <div className="msgs" style={{ display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div style={{ maxWidth: 420 }}>
        <h2 style={{ fontSize: 17, margin: "0 0 8px" }}>最初のチャットを始めましょう</h2>
        <p className="muted" style={{ margin: "0 0 12px" }}>作業フォルダを選ばずに、相談や文章作成から始められます。コードを扱うときはフォルダを指定します。</p>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button className="btn-main" onClick={() => onAct("newChat")}><Icon name="chat" />一般チャットを始める</button>
          <button className="btn-line" onClick={() => onAct("newChat")}><Icon name="folder" />フォルダを指定して始める</button>
        </div>
        <p className="small muted" style={{ marginTop: 14 }}>CLI や VS Code で作った会話は、左の一覧から開けます（外部で実行中なら閲覧のみ）。</p>
      </div>
    </div>
  );
}

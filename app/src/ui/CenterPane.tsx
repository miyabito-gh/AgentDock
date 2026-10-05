import { useEffect, useRef, useState } from "react";
import type {
  AgentKey, Attachment, Chat, ChatLocalView, ChatModelSettings, ChatQueue, DecisionOption, HostSnapshot, ModelInfo, PendingRequest,
  PermissionPreset, QueueEntry, RequestAnswer, ActivityKind, SaveState, StopRecord, TurnRecord,
} from "../ipc/types";
import { Icon } from "./Icon";
import { TENTATIVE_HINT, TENTATIVE_STYLE, chatAgents, chatName, chatRequests, chatTitle, chatStatusText, chatStops, isRunning, rootView, stopOpen } from "./derive";
import { FRESH, STOP_LABEL, hms, holdText, keyStr, knownValue, showKnown, stopCauseText } from "./format";

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
  /** 完了後に送る依頼（キュー）。undefined＝まだ登録がない。 */
  queue: ChatQueue | undefined;
  /** チャット別の補足情報（権限・次のturnの作業フォルダ）。 */
  local: ChatLocalView | undefined;
  qact: QueueActions;
  onPermission: (p: PermissionPreset) => void;
  onCwd: (cwd: string, confirmed: boolean) => Promise<CwdResult>;
}

export interface SendNotice { cls: "warn" | "err"; title: string; body: string; canRetry: boolean; alt?: { label: string; run: () => void } }

/** キュー項目への操作（ホストが判断・保存する。UIは意図を伝えるだけ）。 */
export interface QueueActions {
  edit: (id: string, text: string) => Promise<boolean>;
  cancel: (id: string) => void;
  /** 「キューを再開」。「確認済み」とは別の操作。 */
  resume: () => void;
  /** 「履歴と照合」。読み取りのみで再送しない。 */
  reconcile: (attempt: string) => void;
  /** 受理なしが確定した依頼を、ユーザーの確認のうえで再送する。 */
  retry: (attempt: string) => void;
}

export type CwdResult = { kind: "ok" } | { kind: "needsConfirm"; waiting: number } | { kind: "error"; message: string };

const PERMISSION_LABEL: Record<PermissionPreset, string> = {
  workspaceWriteOnRequest: "作業領域内の書込み＋必要時に確認（初期値）",
  readOnly: "読み取りのみ",
  fullAccess: "フルアクセス（確認なしで実行）",
};

/** 作業フォルダを変えられない理由（変えられるときは null）。ホストも同じ規則で拒否する。 */
const cwdLockReason = (chat: Chat, running: boolean, stopped: boolean): string | null =>
  chat.kind === "general" ? "一般チャットの作業フォルダは変更できません"
    : running ? "作業中は変更できません。完了・停止後に変更してください"
    : stopped ? "停止を確認できるまで変更できません" : null;

function Header({ snap, chat, onAct, running, local, onCwdToggle, cwdLock }: {
  snap: HostSnapshot; chat: Chat; onAct: (a: string) => void; running: boolean; local: ChatLocalView | undefined;
  onCwdToggle: () => void; cwdLock: string | null;
}) {
  const root = rootView(snap, chat);
  const fresh = FRESH[root?.freshness ?? "needsReconcile"];
  const cwd = knownValue(chat.cwd);
  return (
    <div className="chead">
      <button className="only-narrow" aria-label="チャット一覧を開く" onClick={() => onAct("toggleLeft")}><Icon name="list" /></button>
      <div className="grow">
        <div className="ttl" style={chatTitle(chat).confirmed ? undefined : TENTATIVE_STYLE} title={chatTitle(chat).confirmed ? undefined : TENTATIVE_HINT}>{chatName(chat)}</div>
        <div className="meta">
          <span className="tag ai" title="この会話のAI">Codex</span>
          {cwd ? <span className="mono" title="作業フォルダ">{cwd}</span> : <span>一般チャット（作業フォルダなし）</span>}
          {local?.nextCwd ? <span className="mono" title="次の送信から使う作業フォルダ（まだ適用していません）">次のturnから: {local.nextCwd}</span> : null}
          {chat.kind === "development" ? <button className="small" disabled={!!cwdLock} title={cwdLock ?? "次の送信から使う作業フォルダを変更します"} onClick={onCwdToggle}>フォルダ変更</button> : null}
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
          {p.notice.canRetry || p.notice.alt ? (
            <div className="acts">
              {p.notice.canRetry ? <button className="btn-line" onClick={p.onRetrySend}>もう一度送る</button> : null}
              {p.notice.alt ? <button className="btn-line" onClick={p.notice.alt.run}>{p.notice.alt.label}</button> : null}
            </div>
          ) : null}
        </div>
      ) : null}
      {chat.origin === "external" && root?.freshness === "live" ? (
        <div className="cbanner info">
          <h4>{p.externalLabel ?? "外部"} で作成された会話を、このアプリで再開済みです</h4>
          ユーザーの確認のうえで再開しました。続きを送れます。元の作業フォルダやファイルは参照のみで、削除の対象にしません。
        </div>
      ) : chat.origin === "external" ? (
        <div className="cbanner warn">
          <h4>{p.externalLabel ?? "外部"} で作成された会話です（閲覧のみ）</h4>
          外部でまだ実行中かどうか、このアプリでは確認できません。外部での実行が終わったことを確認してから再開すると、このアプリで続きを送れます。元の作業フォルダやファイルは参照のみで、削除の対象にしません。
          {root && (root.status.state === "running" || root.status.state === "waiting") ? <div style={{ marginTop: 4 }}><b>最新のturnが実行中として記録されています。外部で実行中の可能性があります。</b></div> : null}
          <div className="acts"><button className="btn-main" onClick={() => onAct("resumeExternal")}>外部での実行は終わっています。この会話を再開する</button></div>
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
          <div className="acts"><button className="btn-line" onClick={() => onAct("retrySave")}>保存を再試行</button><button className="btn-line" onClick={() => onAct("settings:storage")}>容量を確認</button></div>
        </div>
      ) : null}
    </>
  );
}

const KIND_LABEL: Record<string, string> = {
  command: "コマンド", fileChange: "ファイル変更", toolCall: "ツール", webSearch: "Web検索", subAgent: "サブエージェント",
  reasoning: "推論", plan: "計画", agentMessage: "応答", userMessage: "依頼",
};
const kindLabel = (k: ActivityKind): string => (k.kind === "other" ? k.raw : KIND_LABEL[k.kind] ?? k.kind);
/** 作業の記録1件の表示。値が本当に無いときだけ「記載なし」とする（取得できた属性はホストが文にして渡す）。 */
const entryText = (t: TurnRecord["entries"][number]): string => (t.text.kind === "value" ? t.text.value : "（内容の記載なし）");
const NEAR_BOTTOM_PX = 120;

function Messages({ turns, reqs, running, onRespond, onAct }: {
  turns: TurnRecord[]; reqs: PendingRequest[]; running: boolean; onRespond: CenterProps["onRespond"]; onAct: (a: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [away, setAway] = useState(false);
  const toBottom = () => { const el = ref.current; if (el) el.scrollTop = el.scrollHeight; stick.current = true; setAway(false); };
  const onScroll = () => {
    const el = ref.current;
    if (!el) return;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
    stick.current = near;
    if (near) setAway(false);
  };
  // 更新（新しい本文・逐次表示）のたびに、最下部付近にいるときだけ追従する。過去を読んでいるときは動かさず「最新へ」を出す。
  useEffect(() => {
    if (stick.current) toBottom(); else setAway(true);
  }, [turns, reqs.length, running]);
  return (
    <div className="msgs-wrap">
    <div className="msgs" id="msgs" aria-live="polite" ref={ref} onScroll={onScroll}>
      {turns.map((t) => {
        const others = t.entries.filter((e) => e.kind.kind !== "userMessage" && e.kind.kind !== "agentMessage");
        return (
          <div key={keyStr(t.key.agent) + t.key.turnId}>
            {t.entries.filter((e) => e.kind.kind === "userMessage" || e.kind.kind === "agentMessage").map((e) => {
              const text = e.text.kind === "value" ? e.text.value : "…";
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
                  <ul style={{ margin: "4px 0", paddingLeft: 18 }}>{others.map((e) => <li key={e.key.itemId}><b>{kindLabel(e.kind)}</b>　{entryText(e)}</li>)}</ul>
                </details>
              </div>
            ) : null}
          </div>
        );
      })}
      {reqs.map((r) => <RequestCard key={keyStr({ backend: r.key.backend, id: r.key.requestId })} r={r} onRespond={onRespond} />)}
      {running && reqs.length === 0 ? <div className="msg ai"><div className="who">Codex</div><div className="activity">作業中です。右のドックで各エージェントの状況を確認できます。</div></div> : null}
    </div>
    {away ? <button className="jump" onClick={toBottom}>最新へ</button> : null}
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

function EntryRow({ n, e, qact }: { n: number; e: QueueEntry; qact: QueueActions }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(e.text);
  const s = e.state;
  const why = s.kind === "acceptanceUnknown" ? "受理不明: 送信したか確認できません。履歴と照合して確定するまで、再送しません"
    : s.kind === "sending" ? "送信中"
    : s.kind === "notAccepted" ? `受理されていません（${s.message}）。自動では再送しません`
    : null;
  const save = async () => { if (await qact.edit(e.id, text)) setEditing(false); };
  return (
    <div className="qitem">
      <span className="qnum">{n}</span>
      <div>
        {editing ? <textarea aria-label="依頼を編集" value={text} onChange={(ev) => setText(ev.target.value)} style={{ width: "100%" }} /> : <div>{e.text}</div>}
        {why ? <div className={`why ${s.kind === "waiting" ? "" : "err"}`}>{why}</div> : null}
      </div>
      <div style={{ display: "flex", gap: 2 }}>
        {s.kind === "waiting" && !editing ? <button className="small" onClick={() => setEditing(true)}>編集</button> : null}
        {editing ? <><button className="small" onClick={() => void save()}>保存</button><button className="small" onClick={() => { setEditing(false); setText(e.text); }}>やめる</button></> : null}
        {/* 受理不明の間は再送ボタンを出さない。照合だけ */}
        {s.kind === "acceptanceUnknown" ? <button className="small" onClick={() => qact.reconcile(s.attempt)}>履歴と照合</button> : null}
        {s.kind === "notAccepted" ? <button className="small" title="受理されていないことを確認済みの依頼だけ、新しい送信として送り直します" onClick={() => qact.retry(s.attempt)}>もう一度送る</button> : null}
        {s.kind === "waiting" || s.kind === "notAccepted" ? <button className="small" onClick={() => qact.cancel(e.id)}>取消</button> : null}
      </div>
    </div>
  );
}

function Queue({ q, nameOf, qact }: { q: ChatQueue | undefined; nameOf: (a: AgentKey) => string; qact: QueueActions }) {
  if (!q) return null;
  const items = q.entries.filter((e) => e.state.kind !== "sent" && e.state.kind !== "cancelled");
  if (!items.length) return null;
  const unknown = items.some((e) => e.state.kind === "acceptanceUnknown");
  const stopped = q.run.kind !== "active";
  return (
    <div className="queue">
      {stopped ? (
        <div className="cbanner warn" role="alert" style={{ margin: "0 0 6px" }}>
          <h4>{q.run.kind === "pausedAfterRestart" ? "再起動後のため、自動送信を止めています" : "自動送信を止めています"}</h4>
          {q.run.kind === "stopped" ? stopCauseText(q.run.cause, nameOf) : "アプリを再起動したため、送信待ちの依頼は自動では送りません。内容を確認して再開してください。"}
          <div style={{ marginTop: 4 }}>失敗した依頼そのものは再実行しません。「確認済み」にしてもキューは再開しません。</div>
          <div className="acts">
            <button className="btn-main" disabled={unknown} title={unknown ? "受理不明の依頼を照合で確定するまで再開できません" : undefined} onClick={qact.resume}>キューを再開</button>
          </div>
        </div>
      ) : null}
      <details open>
        <summary><Icon name="list" /><b>完了後に送る依頼 {items.length}件</b>
          <span className="muted">{stopped ? "停止中" : q.hold ? holdText(q.hold, nameOf) : "親と子孫の作業が終わると、登録順に1件ずつ送ります"}</span></summary>
        <div className="muted small" style={{ padding: "2px 10px" }}>送信待ちの依頼は、送信する時点のモデル・権限・作業フォルダで送ります。</div>
        {items.map((e, i) => <EntryRow key={e.id + e.state.kind} n={i + 1} e={e} qact={qact} />)}
      </details>
    </div>
  );
}

function CwdPanel({ current, onCwd, onClose }: { current: string; onCwd: CenterProps["onCwd"]; onClose: () => void }) {
  const [path, setPath] = useState(current);
  const [need, setNeed] = useState<number | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const apply = async (confirmed: boolean) => {
    const r = await onCwd(path.trim(), confirmed);
    if (r.kind === "ok") onClose(); else if (r.kind === "needsConfirm") setNeed(r.waiting); else setMsg(r.message);
  };
  return (
    <div className="cbanner info">
      <h4>次の送信から使う作業フォルダを変更</h4>
      <input aria-label="作業フォルダ" className="mono" style={{ width: "100%" }} value={path} onChange={(e) => { setPath(e.target.value); setNeed(null); }} />
      {need !== null ? <div style={{ marginTop: 4 }}><b>送信待ちの依頼 {need}件も、新しいフォルダが対象になります。</b></div> : null}
      {msg ? <div className="why err" style={{ marginTop: 4 }}>{msg}</div> : null}
      <div className="acts">
        <button className="btn-main" disabled={!path.trim()} onClick={() => void apply(need !== null)}>{need !== null ? "確認して変更" : "変更"}</button>
        <button className="btn-line" onClick={onClose}>やめる</button>
      </div>
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
              <button aria-pressed={steer} title="実行中のturnへ対象を照合して送ります。モデル・権限・フォルダの変更には使えません" onClick={() => setSteer(true)}>追加指示（現在のturnへ）</button>
              <button aria-pressed={!steer} title="親と子孫の作業が終わってから、次の依頼として送ります" onClick={() => setSteer(false)}>完了後に送信（次の依頼）</button>
            </span>
          ) : null}
          <select aria-label="権限" title="次の送信から適用。送信待ちの依頼にも送信時に適用されます" value={p.local?.permission ?? "workspaceWriteOnRequest"} onChange={(e) => p.onPermission(e.target.value as PermissionPreset)}>
            {(Object.keys(PERMISSION_LABEL) as PermissionPreset[]).map((k) => <option key={k} value={k}>{PERMISSION_LABEL[k]}</option>)}
          </select>
          <span className="grow" />
          {running ? <button className="btn-line" aria-label="中断" title="実行中の作業に中断を要求します（停止の確認は別に行います）" onClick={() => p.onAct("interrupt")}><Icon name="stop" />中断</button> : null}
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
  const [cwdOpen, setCwdOpen] = useState(false);
  const cwdLock = cwdLockReason(chat, running, hasOpenStop);
  const nameOf = (a: AgentKey): string => {
    const v = chatAgents(snap, chat).find((x) => keyStr(x.agent.key) === keyStr(a));
    return v ? showKnown(v.agent.displayName) : a.id;
  };
  const lock = hasOpenStop ? "停止を確認できるまで、このチャットへの新しい送信は止めています。"
    : chat.origin === "external" && rootView(snap, chat)?.freshness !== "live" ? "外部で実行中かどうか確認できないため、再開するまでこの会話には送信できません。上のボタンから、外部側の終了を確認して再開してください。" : null;
  return (
    <>
      <Header snap={snap} chat={chat} onAct={p.onAct} running={running} local={p.local} onCwdToggle={() => setCwdOpen((o) => !o)} cwdLock={cwdLock} />
      {cwdOpen && !cwdLock ? <CwdPanel current={p.local?.nextCwd ?? knownValue(chat.cwd) ?? ""} onCwd={p.onCwd} onClose={() => setCwdOpen(false)} /> : null}
      <Banners p={p} />
      <Messages key={chat.key.id} turns={p.turns} reqs={reqs} running={running} onRespond={p.onRespond} onAct={p.onAct} />
      <Queue q={snap.queues.find((q) => keyStr(q.chat) === keyStr(chat.key))} nameOf={nameOf} qact={p.qact} />
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

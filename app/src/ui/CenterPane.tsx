import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import type {
  AgentKey, ArtifactEntry, AttachmentEntry, Chat, ChatKey, ChatLocalView, ChatModelSettings, ChatQueue, DecisionOption, FileRef, Goal, HostSnapshot, Known, ModelInfo, PendingRequest,
  PermissionPreset, QueueEntry, RequestAnswer, ActivityKind, SaveState, StopRecord, TurnRecord, WorkMode, WorkModeInfo,
} from "../ipc/types";
import { Icon } from "./Icon";
import { PENDING_TEXT } from "./ManageDialogs";
import { TENTATIVE_HINT, TENTATIVE_STYLE, chatAgents, chatName, chatRequests, chatTitle, chatStatusText, chatStops, isRunning, rootView, stopOpen } from "./derive";
import { FRESH, STOP_LABEL, hms, holdText, keyStr, knownValue, showKnown, stopCauseText, targetList } from "./format";
import { baseName, clipboardImageName, mimeOf } from "./attach";
import type { CommandView } from "./commands";
import { GoalBar, UnverifiedTag, WORK_MODE_TEXT } from "./PrefsDialogs";

const SCOPE_TEXT = { once: "1回だけ", session: "このセッション中", persistent: "以後ずっと（永続）", unknown: "効力範囲は不明" } as const;

export interface CenterProps {
  snap: HostSnapshot;
  chat: Chat;
  turns: TurnRecord[];
  /** 送信前の添付（下書きに付いているもの）。 */
  attachments: AttachmentEntry[];
  /** 添付・成果物の操作（ホストが判断・保存する。UIは意図を伝えるだけ）。 */
  files: FileActions;
  /** 入力欄（選択範囲の貼り付けでカーソル位置を使う）。 */
  inputRef: RefObject<HTMLTextAreaElement>;
  settings: ChatModelSettings | undefined;
  models: ModelInfo[];
  save: SaveState | null;
  externalLabel: string | undefined;
  /** このアプリの起動中に、この会話が一度でも live 監視になったか。 */
  wasLive: boolean;
  enterMode: "ctrl" | "enter";
  draft: string;
  setDraft: (v: string) => void;
  onAct: (a: string) => void;
  /** 操作台帳から作った可否の一覧（主要ボタン・メニュー・Ctrl+K で共通）。 */
  commands: CommandView[];
  onRespond: (r: PendingRequest, a: RequestAnswer) => void;
  /** 依頼を受け付けたら true（下書きを消す）。拒否・前提不足なら false（下書きを残す）。 */
  onSend: (text: string, steer: boolean, attachments: string[]) => boolean | Promise<boolean>;
  /** モデル・推論の強さ・速度の選択（次のターンから適用。受理済みは別に表示）。speed が空なら指定なし。 */
  onModel: (model: string, effort: string, speed: string) => void;
  /** 計画／実行の選択（null＝選択を外す）。次のターンから適用。受理済みは別に表示する。 */
  onWorkMode: (m: WorkMode | null) => void;
  /** バックエンドが示した計画／実行の選択肢（取得できていなければ空）。 */
  workModes: WorkModeInfo[];
  /** 目標（Goal）の表示。undefined＝まだ読んでいない。 */
  goal: Known<Goal> | undefined;
  goalBusy: boolean;
  onGoalReload: () => void;
  onGoalClear: () => void;
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

export interface FileActions {
  /** 関連アプリで開く（ユーザー操作のときだけ）。 */
  open: (t: FileRef) => void;
  /** 保存先を選んで保存する（同名は上書き確認）。 */
  save: (t: FileRef, name: string) => void;
  /** 画像のアプリ内プレビュー用のバイト列。 */
  preview: (t: FileRef) => Promise<ArrayBuffer>;
  /** 送信前の添付を取り外す。 */
  remove: (id: string) => void;
  /** クリップボード画像の貼り付け。 */
  pasteImage: (name: string, bytes: ArrayBuffer) => void;
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
  /** 「状態を再確認」。保留中の子孫の履歴を読み直す（読み取りのみ。resumeしない）。 */
  recheck: () => void;
  /** 「確認して今すぐ送る」。確認ダイアログで承認した後にだけ呼ぶ。先頭の1件だけを送る（以後は通常の判定）。 */
  sendNow: (entry: string) => Promise<boolean>;
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

/** 元の会話の名前（一覧にないときはID）。 */
const origName = (snap: HostSnapshot, k: ChatKey): string => {
  const c = snap.chats.find((x) => x.key.id === k.id);
  return c ? chatName(c) : k.id;
};

function Header({ snap, chat, onAct, commands, running, local, onCwdToggle, cwdLock }: {
  snap: HostSnapshot; chat: Chat; onAct: (a: string) => void; commands: CommandView[]; running: boolean; local: ChatLocalView | undefined;
  onCwdToggle: () => void; cwdLock: string | null;
}) {
  const root = rootView(snap, chat);
  const fresh = FRESH[root?.freshness ?? "needsReconcile"];
  const cwd = knownValue(chat.cwd);
  return (
    <div className="chead">
      <button className="only-narrow-l" aria-label="チャット一覧を開く" onClick={() => onAct("toggleLeft")}><Icon name="list" /></button>
      <div className="grow">
        <div className="ttl" style={chatTitle(chat).confirmed ? undefined : TENTATIVE_STYLE} title={chatTitle(chat).confirmed ? undefined : TENTATIVE_HINT}>{chatName(chat)}</div>
        <div className="meta">
          <span className="tag ai" title="この会話のAI">Codex</span>
          {cwd ? <span className="mono" title="作業フォルダ">{cwd}</span> : <span>一般チャット（作業フォルダなし）</span>}
          {local?.nextCwd ? <span className="mono" title="次の送信から使う作業フォルダ（まだ適用していません）">次のturnから: {local.nextCwd}</span> : null}
          {local?.reviewOf ? <span className="tag" title="このチャットはレビュー用に作られました">レビュー元: {origName(snap, local.reviewOf)}</span> : null}
          {local?.forkOf ? <span className="tag" title="このチャットは分岐で作られました（AgentDockの記録）">分岐元: {origName(snap, local.forkOf.chat)}{local.forkOf.throughTurn ? `（turn ${local.forkOf.throughTurn} まで）` : ""}</span> : null}
          {chat.kind === "development" ? <button className="small" disabled={!!cwdLock} title={cwdLock ?? "次の送信から使う作業フォルダを変更します"} onClick={onCwdToggle}>フォルダ変更</button> : null}
          {/* 鮮度と状態は別表示。片方から他方を導かない */}
          <span className={`fresh ${fresh.c}`} title="収集の鮮度。エージェントの状態とは別">{fresh.t}</span>
          <span>{chatStatusText(snap, chat)}</span>
        </div>
      </div>
      {running ? <button className="btn-line" title="Esc" onClick={() => onAct("interrupt")}><Icon name="stop" />中断</button> : null}
      {/* 主要操作は台帳から作る。無効のときは押せず、理由をツールチップで示す（文字のみ） */}
      <div className="chead-ops" role="group" aria-label="主要な操作">
        {commands.filter((v) => v.cmd.header).map((v) => {
          const st = v.state;
          return (
            <button key={v.cmd.id} disabled={st.kind === "disabled"} aria-label={v.cmd.label.replace(/…$/, "")}
              title={st.kind === "disabled" ? st.reason : st.unverified ? "未確認の操作です（結果は観測した事実だけを表示します）" : undefined}
              onClick={() => onAct(v.cmd.act)}>
              {v.cmd.header}{st.kind === "enabled" && st.unverified ? <span className="tag unv">未確認</span> : null}
            </button>
          );
        })}
        <button aria-label="コマンドメニューを開く" title="すべての操作を検索（Ctrl+K）" onClick={() => onAct("palette")}>操作…</button>
      </div>
      <button className="only-narrow-r" aria-label="エージェントのドックを開く" onClick={() => onAct("toggleRight")}><Icon name="dock" /></button>
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
        <button className="btn-line" onClick={() => onAct("noop")}>待つ</button>
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
      {chat.noHistory ? (
        <div className="cbanner info">
          <h4>Codex に、この会話の履歴がありません</h4>
          発話前のチャットなどは、Codex の一覧に載りません。このアプリの記録（ピン・下書き・モデル選択）だけを表示しています。履歴を作る送信は、再開できる状態になってから行えます。
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
          <h4>{p.wasLive ? "接続を回復しましたが、ライブ監視はまだ戻っていません" : "アプリの再起動後のため、保存履歴から表示しています"}</h4>
          {p.wasLive
            ? "保存履歴から取得した状態です（"
            : "送信すると続きから再開します（再開するまでライブ監視はしません）。保存履歴から取得した状態です（"}{hms(root.status.evidence.observedAt)}）。実行中の途中経過は表示されません。送信待ちは照合が終わるまで保留します。
          <div className="acts"><button className="btn-line" title="保存履歴を読み直して状態を取り直します（読み取りのみ）" onClick={() => onAct("reloadHistory")}>状態を再照合</button><button className="btn-line" onClick={() => onAct("reloadHistory")}>保存履歴を再取得</button></div>
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

/** 画像のアプリ内プレビュー（読み込めなければ印だけ。モデルが読めるかとは別）。クリックで拡大。 */
function Thumb({ target, name, load }: { target: FileRef; name: string; load: FileActions["preview"] }) {
  const [url, setUrl] = useState<string | null>(null);
  const [big, setBig] = useState(false);
  const loadRef = useRef(load);
  loadRef.current = load;
  const id = target.kind + ":" + target.id;
  useEffect(() => {
    let alive = true;
    let made: string | null = null;
    loadRef.current(target).then((buf) => {
      if (!alive) return;
      made = URL.createObjectURL(new Blob([buf], { type: mimeOf(name) }));
      setUrl(made);
    }).catch(() => { /* 読めないときはプレビューなし（開く・保存は使える） */ });
    return () => { alive = false; if (made) URL.revokeObjectURL(made); };
  }, [id, name]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!url) return <span className="thumb" title="画像（プレビューを読み込めていません）" />;
  return (
    <>
      <button className="thumbbtn" aria-label={`${name}のプレビューを拡大`} onClick={() => setBig(true)}><img className="thumbimg" src={url} alt="" /></button>
      {big ? (
        <div className="lightbox" role="dialog" aria-modal="true" aria-label={`${name}のプレビュー`} onClick={() => setBig(false)}>
          <img src={url} alt={name} />
          <div className="small">{name}　（画面で表示できることと、モデルが内容を読めることは別です。クリックで閉じる）</div>
        </div>
      ) : null}
    </>
  );
}

/** 添付の状態の表示。コピーできていないもの・実体がないものは、使えると言わない。 */
function attachmentStatus(a: AttachmentEntry): { text: string; bad: boolean } {
  const s = a.state;
  // Skillの指定はコピーしない参照。コピー済みとは書かない。
  if (a.source.kind === "skill") {
    if (s.kind === "ready") return { text: "Skill・コピーなし（送信時に明示的に呼び出します）", bad: false };
    if (s.kind === "missing") return { text: "Skillの定義ファイルが見つかりません（欠損）", bad: true };
  }
  switch (s.kind) {
    case "copying": return { text: "コピー中…", bad: false };
    case "ready": return { text: `コピー済み　${hms(a.attachedAt)}`, bad: false };
    case "copyFailed": return { text: `コピーできません（${s.message}）`, bad: true };
    case "missing": return { text: "コピーが見つかりません（欠損）", bad: true };
  }
}

const isImageName = (n: string): boolean => mimeOf(n).startsWith("image/");

/** 会話内のファイル項目（送信した添付と、実在を確認した成果物）。開く・保存はユーザーの操作だけ。 */
function FilesBlock({ sent, artifacts, files }: { sent: AttachmentEntry[]; artifacts: ArtifactEntry[]; files: FileActions }) {
  if (!sent.length && !artifacts.length) return null;
  return (
    <div className="msg ai files">
      <details open>
        <summary><Icon name="clip" /><b>ファイル（添付 {sent.length}件・成果物 {artifacts.length}件）</b></summary>
        <div className="small muted" style={{ margin: "2px 0 4px" }}>ファイルを表示できることと、モデルが内容を読めることは別です。外部アプリでは、「開く」を押したときだけ開きます。</div>
        {sent.map((a) => {
          const st = attachmentStatus(a);
          const target: FileRef = { kind: "attachment", chat: a.chat, id: a.id };
          return (
            <div className="frow" key={a.id}>
              {a.kind === "image" && a.state.kind === "ready" ? <Thumb target={target} name={a.displayName} load={files.preview} /> : <Icon name="clip" />}
              <div className="grow"><b>{a.displayName}</b>　<span className="st">添付・{st.text}</span></div>
              <button className="small" disabled={st.bad || a.state.kind !== "ready" || a.kind === "skill"} onClick={() => files.open(target)}>開く</button>
              <button className="small" disabled={st.bad || a.state.kind !== "ready" || a.kind === "skill"} onClick={() => files.save(target, a.displayName)}>名前を付けて保存…</button>
            </div>
          );
        })}
        {artifacts.map((a) => {
          const target: FileRef = { kind: "artifact", chat: a.chat, id: a.id };
          const name = baseName(a.path);
          const gone = a.exists.kind === "value" && !a.exists.value;
          const unsure = a.exists.kind !== "value";
          return (
            <div className="frow" key={a.id}>
              {isImageName(name) && !gone && !unsure ? <Thumb target={target} name={name} load={files.preview} /> : <Icon name="diff" />}
              <div className="grow">
                <b>{name}</b>　<span className={`st ${gone ? "bad" : ""}`}>成果物・{gone ? "見つかりません（欠損）。移動または削除された可能性があります" : unsure ? "実在を確認できていません" : `実在を確認済み${a.checkedAt !== null ? `（${hms(a.checkedAt)}）` : ""}`}</span>
                <div className="mono small muted">{a.path}{a.inChatArea ? "" : "　（作業フォルダ側のファイル。チャットを削除しても消しません）"}</div>
              </div>
              <button className="small" disabled={gone} onClick={() => files.open(target)}>開く</button>
              <button className="small" disabled={gone} onClick={() => files.save(target, name)}>名前を付けて保存…</button>
            </div>
          );
        })}
      </details>
    </div>
  );
}

const KIND_LABEL: Record<string, string> = {
  command: "コマンド", fileChange: "ファイル変更", toolCall: "ツール", webSearch: "Web検索", subAgent: "サブエージェント",
  reasoning: "推論", plan: "計画", agentMessage: "応答", userMessage: "依頼",
  reviewStarted: "レビュー開始", reviewResult: "レビュー結果", contextCompaction: "文脈の圧縮",
};
const kindLabel = (k: ActivityKind): string => (k.kind === "other" ? k.raw : KIND_LABEL[k.kind] ?? k.kind);
/** 作業の記録1件の表示。値が本当に無いときだけ「記載なし」とする（取得できた属性はホストが文にして渡す）。 */
const entryText = (t: TurnRecord["entries"][number]): string => (t.text.kind === "value" ? t.text.value : "（内容の記載なし）");
const NEAR_BOTTOM_PX = 120;

function Messages({ turns, reqs, running, onRespond, onAct, extra }: {
  turns: TurnRecord[]; reqs: PendingRequest[]; running: boolean; onRespond: CenterProps["onRespond"]; onAct: (a: string) => void; extra?: ReactNode;
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
                  <div className="acts"><button onClick={() => onAct("stub")}><Icon name="copy" />コピー</button><button onClick={() => onAct(`fork:${t.key.turnId}`)}><Icon name="branch" />ここから分岐</button></div>
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
      {extra}
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

export function RequestCard({ r, onRespond }: { r: PendingRequest; onRespond: CenterProps["onRespond"] }) {
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
        {e.attachments.length ? <div className="small muted">添付 {e.attachments.length}件（このチャット用のコピーを送ります）</div> : null}
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

/** 状態不明の子孫で保留しているときの出口（再確認／確認して今すぐ送る）。親の状態が原因の保留には出さない。 */
function HoldExit({ q, head, nameOf, qact }: { q: ChatQueue; head: QueueEntry | undefined; nameOf: (a: AgentKey) => string; qact: QueueActions }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const h = q.hold;
  if (q.run.kind !== "active" || !h || (h.kind !== "stateUnknown" && h.kind !== "notLive")) return null;
  const targets = h.targets;
  // 親自身の状態・鮮度が原因のときは、子孫の確認では解けない（ホストも送らない）。
  const rootBlocked = targets.some((t) => t.agent.id === q.chat.id);
  const canSend = !!head && head.state.kind === "waiting" && !rootBlocked;
  return (
    <div style={{ padding: "2px 10px" }}>
      <div className="acts">
        <button className="small" title="保留中の子孫の履歴を読み直します（読み取りのみ。再開はしません）。終端を確認できれば、自動で送信に進みます" onClick={qact.recheck}>状態を再確認</button>
        {canSend ? <button className="small" onClick={() => setConfirming(true)}>確認して今すぐ送る…</button> : null}
      </div>
      {confirming && head ? (
        <div className="cbanner warn" role="alert" style={{ margin: "6px 0" }}>
          <h4>状態を確認できないまま、先頭の依頼を送ります</h4>
          <div>次の対象は、完了・失敗・中断を確認できていません。作業が続いている可能性があります。</div>
          <ul style={{ margin: "4px 0 4px 18px", padding: 0 }}>
            {targets.length ? targets.map((t) => <li key={t.agent.id}>{targetList([t], nameOf)}</li>) : null}
            {h.kind === "stateUnknown" && h.descendantScanIncomplete ? <li>子孫の探索が完了していません（未発見の子孫がいる可能性）</li> : null}
          </ul>
          <div>送るのは先頭の1件（「{head.text.slice(0, 40)}{head.text.length > 40 ? "…" : ""}」）だけです。以後の依頼は、通常どおり条件を判定します。</div>
          <div className="acts">
            <button className="btn-main" disabled={busy} onClick={() => { setBusy(true); void qact.sendNow(head.id).then(() => { setBusy(false); setConfirming(false); }); }}>確認して送る</button>
            <button className="btn-line" disabled={busy} onClick={() => setConfirming(false)}>やめる</button>
          </div>
        </div>
      ) : null}
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
        <HoldExit q={q} head={items[0]} nameOf={nameOf} qact={qact} />
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
  const [speed, setSpeed] = useState(settings?.selected?.speedTier ?? "");
  const [steer, setSteer] = useState(false);
  const accepted = settings?.accepted.kind === "value" ? settings.accepted.value : null;
  const changed = !!accepted && (accepted.model !== model || (accepted.effort ?? "") !== effort || (accepted.speedTier ?? "") !== (speed === "default" ? "" : speed));
  const efforts = models.find((m) => m.id === model)?.efforts ?? [];
  const tiers = models.find((m) => m.id === model)?.speedTiers ?? [];
  const caps = p.snap.opCapabilities;
  const wmCap = caps.find((c) => c.op === "workMode");
  const stCap = caps.find((c) => c.op === "speedTier");
  const wmUsable = wmCap?.support === "supported";
  const wmSelected = settings?.workMode ?? null;
  const wmAccepted = settings && settings.acceptedWorkMode.kind === "value" ? settings.acceptedWorkMode.value : null;
  const wmOptions: WorkModeInfo[] = p.workModes.length ? p.workModes : [{ mode: "plan", label: WORK_MODE_TEXT.plan }, { mode: "default", label: WORK_MODE_TEXT.default }];
  // コピー中・失敗・欠損の添付があるうちは送らない（ホストも同じ規則で止める。不完全なコピーは送れない）。
  const attachBlock = attachments.some((a) => a.state.kind !== "ready") ? "コピー中・コピー失敗・欠損の添付があります。取り外すか、もう一度添付してから送信してください。" : null;
  const send = async () => {
    if (!p.draft.trim() || lock || attachBlock) return;
    const ok = await p.onSend(p.draft, running && steer, attachments.map((a) => a.id));
    if (ok) p.setDraft("");
  };
  const pickModel = (id: string) => {
    const e = models.find((m) => m.id === id)?.defaultEffort ?? "";
    // 速度の選択肢はモデルごとに違うので、モデルを変えたら指定なしに戻す。
    setModel(id); setEffort(e); setSpeed(""); p.onModel(id, e, "");
  };
  const pickEffort = (e: string) => { setEffort(e); p.onModel(model, e, speed); };
  const pickSpeed = (s: string) => { setSpeed(s); p.onModel(model, effort, s); };
  return (
    <div className="composer">
      <div className={`shell ${lock ? "locked" : ""}`}>
        {attachments.length ? (
          <div className="chips">
            {attachments.map((f) => {
              const st = attachmentStatus(f);
              const target: FileRef = { kind: "attachment", chat: f.chat, id: f.id };
              return (
                <span key={f.id} className={`chip ${st.bad ? "bad" : ""}`}>
                  {f.kind === "image" && f.state.kind === "ready" ? <Thumb target={target} name={f.displayName} load={p.files.preview} /> : <Icon name="clip" />}
                  <span>{f.displayName}</span><span className="st">{st.text}</span>
                  {f.state.kind === "ready" && f.kind !== "skill" ? <button className="small" aria-label={`${f.displayName}を関連アプリで開く`} onClick={() => p.files.open(target)}>開く</button> : null}
                  <button aria-label={`${f.displayName}を取り外す`} disabled={f.state.kind === "copying"} onClick={() => p.files.remove(f.id)}><Icon name="x" /></button>
                </span>
              );
            })}
            <div className="small muted" style={{ width: "100%" }}>送信前に確認・取り外せます。このチャット用にコピーした分を送ります（元のファイルは変更しません）。画像は表示できますが、モデルが内容を読めるかは未確認です。</div>
          </div>
        ) : null}
        {lock ? <div className="lockmsg">{lock}</div> : null}
        {attachBlock ? <div className="lockmsg">{attachBlock}</div> : null}
        {wmSelected && !wmUsable ? (
          <div className="lockmsg">計画／実行（{WORK_MODE_TEXT[wmSelected]}）を選んだままですが、この Codex 接続では使えないため、送信は止まります。<button className="small" onClick={() => p.onWorkMode(null)}>選択を外す</button></div>
        ) : null}
        <textarea aria-label="メッセージ" disabled={!!lock} value={p.draft} onChange={(e) => p.setDraft(e.target.value)} ref={p.inputRef}
          placeholder={running ? (steer ? "現在の作業への追加指示" : "完了後に送る次の依頼") : "メッセージを入力"}
          onPaste={(e) => {
            // クリップボードの画像は添付にする（生バイトでホストへ）。文章だけの貼り付けは通常どおり。
            const img = Array.from(e.clipboardData.files).find((f) => f.type.startsWith("image/"));
            if (!img) return;
            e.preventDefault();
            void img.arrayBuffer().then((buf) => p.files.pasteImage(clipboardImageName(new Date()), buf));
          }}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
            if ((p.enterMode === "ctrl" && e.ctrlKey) || (p.enterMode === "enter" && !e.shiftKey)) { e.preventDefault(); void send(); }
          }} />
        <div className="ctrl">
          <button aria-label="ファイル・画像・参考情報を追加" disabled={!!lock} onClick={() => p.onAct("attach")}><Icon name="clip" /></button>
          <select aria-label="モデル" value={model} onChange={(e) => pickModel(e.target.value)}>{models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select>
          <select aria-label="推論の強さ" value={effort} onChange={(e) => pickEffort(e.target.value)}>{efforts.map((x) => <option key={x.id} value={x.id}>{x.id}</option>)}</select>
          <select aria-label="速度" value={speed} disabled={tiers.length === 0}
            title={tiers.length === 0 ? "このモデルが示す速度の選択肢はありません（Codex から取得した一覧）" : "次の送信から適用。実際に適用されたかは、受理済みの表示で確認します"}
            onChange={(e) => pickSpeed(e.target.value)}>
            <option value="">速度: 指定なし（変更しない）</option>
            <option value="default" title="標準の速度へ戻す指定を送ります（schemaが標準速度の値として示す default）。結果は受理済みの表示で確認してください">速度: 標準（default）</option>
            {tiers.filter((t) => t.id !== "default").map((t) => <option key={t.id} value={t.id} title={t.description ?? undefined}>{t.name}</option>)}
          </select>
          <select aria-label="計画／実行" value={wmSelected ?? ""} disabled={!wmUsable && wmSelected === null}
            title={wmUsable ? "次の送信から適用。実際に適用されたかは、受理済みの表示で確認します（experimental）" : (wmCap?.note ?? "この Codex 接続では使えません")}
            onChange={(e) => p.onWorkMode(e.target.value === "" ? null : (e.target.value as WorkMode))}>
            <option value="">計画／実行: 指定なし</option>
            {wmOptions.map((m) => <option key={m.mode} value={m.mode} disabled={!wmUsable}>{m.label}</option>)}
          </select>
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
          <button className="btn-main send" aria-label={running ? (steer ? "追加指示を送る" : "完了後に送る依頼として登録") : "送信"} disabled={!!lock || !!attachBlock} onClick={() => void send()}><Icon name="send" /></button>
        </div>
      </div>
      <div className="hint">
        <span>{p.enterMode === "ctrl" ? "Ctrl＋Enter で送信、Enter で改行" : "Enter で送信、Shift＋Enter で改行"}</span>
        <span title={!accepted ? "会話の再開・開始の応答か Codex の設定通知でモデルを受け取るまで、受理は確認できません。受け取っていない間は、選択値が使われたとは言えません。" : undefined}>
          {changed ? `選択: ${model}／${effort}${speed ? `／速度 ${speed}` : ""}（未受理。次のターンから適用）　` : ""}
          受理済み: {settings ? (accepted ? `${accepted.model}／${accepted.effort ?? "既定"}${accepted.speedTier ? `／速度 ${accepted.speedTier}` : ""}` : showKnown(settings.accepted)) : "未確認"}
          {!accepted ? "（受け取るまで確認不可）" : ""}
        </span>
        {wmSelected || wmAccepted ? (
          <span title="計画／実行は experimental の設定で、設定の更新通知を受け取るまで受理は確認できません。">
            計画／実行: 選択 {wmSelected ? WORK_MODE_TEXT[wmSelected] : "未選択"}／受理済み {wmAccepted ? WORK_MODE_TEXT[wmAccepted] : "未確認"}
            {wmSelected && wmAccepted !== wmSelected ? "（未受理。次のターンから適用）" : ""} <UnverifiedTag cap={wmCap} />
          </span>
        ) : null}
        {accepted?.speedTier && !speed ? <span className="why">速度は「指定なし」ですが、Codex 側は速度 {accepted.speedTier} のままです。標準へ戻すには「標準（default）」を選んで送信し、受理済みの表示で確認してください。</span> : null}
        {speed === "default" ? <span className="small muted">標準へ戻す指定を送ります。戻ったかは、送信後の受理済みの表示で確認してください（未確認）。</span> : null}
        {stCap?.verification === "unverified" && speed ? <span><UnverifiedTag cap={stCap} /> 速度</span> : null}
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
  const delPending = p.local?.deletePending ?? null;
  const lock = delPending ? `削除を保留中のため、送信できません。${PENDING_TEXT(delPending.reason)}`
    : hasOpenStop ? "停止を確認できるまで、このチャットへの新しい送信は止めています。"
    : chat.origin === "external" && rootView(snap, chat)?.freshness !== "live" ? "外部で実行中かどうか確認できないため、再開するまでこの会話には送信できません。上のボタンから、外部側の終了を確認して再開してください。" : null;
  return (
    <>
      <Header snap={snap} chat={chat} onAct={p.onAct} commands={p.commands} running={running} local={p.local} onCwdToggle={() => setCwdOpen((o) => !o)} cwdLock={cwdLock} />
      {cwdOpen && !cwdLock ? <CwdPanel current={p.local?.nextCwd ?? knownValue(chat.cwd) ?? ""} onCwd={p.onCwd} onClose={() => setCwdOpen(false)} /> : null}
      <Banners p={p} />
      <Messages key={chat.key.id} turns={p.turns} reqs={reqs} running={running} onRespond={p.onRespond} onAct={p.onAct}
        extra={<FilesBlock sent={snap.attachments.filter((a) => keyStr(a.chat) === keyStr(chat.key) && a.usedBy.length > 0)} artifacts={snap.artifacts.filter((a) => keyStr(a.chat) === keyStr(chat.key))} files={p.files} />} />
      <Queue q={snap.queues.find((q) => keyStr(q.chat) === keyStr(chat.key))} nameOf={nameOf} qact={p.qact} />
      <GoalBar goal={p.goal} live={rootView(snap, chat)?.freshness === "live"} cap={snap.opCapabilities.find((o) => o.op === "goal")} busy={p.goalBusy}
        onEdit={() => p.onAct("goal")} onClear={p.onGoalClear} onReload={p.onGoalReload} />
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

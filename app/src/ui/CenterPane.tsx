import { multiFolderMark } from "./workFolders";
import { ORG_SEND_LOCK } from "./OrgPane";
import { RequestGapLine } from "./RequestGap";
import { chatGapRequests } from "./org";
import { Fragment, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import type {
  AgentKey, AgentView, ArtifactEntry, AttachmentEntry, EntryAttachment, Chat, ChatKey, ChatLocalView, ChatModelSettings, ChatQueue, DecisionOption, FileRef, Goal, HostSnapshot, Known, ModelInfo, PendingRequest, PathInfo, SkillInfo, ThreadSegmentView,
  PermissionPreset, QueueEntry, RequestAnswer, ActivityKind, SaveState, StopRecord, TurnRecord, WorkMode, WorkModeInfo,
} from "../ipc/types";
import { Icon } from "./Icon";
import { FindBar } from "./FindBar";
import { Popover } from "./Popover";
import { WorkFoldersPanel, type WorkFoldersApi } from "./WorkFoldersUi";
import { Markdown, PathText, COPY_LABEL, useCopy } from "./Markdown";
import { PENDING_TEXT } from "./ManageDialogs";
import { TENTATIVE_HINT, TENTATIVE_STYLE, chatAgents, chatDisplay, chatName, chatRequests, chatTitle, chatStatusText, chatStops, isRunning, rootView, stopOpen } from "./derive";
import { FRESH, STATE, STOP_LABEL, hms, holdText, keyStr, knownValue, showKnown, stopCauseText, targetList, unconfirmedNote } from "./format";
import { baseName, clipboardImageName, mimeOf } from "./attach";
import { decisionLine, placeDecisions, slotKey, type DecisionItem } from "./decisions";
import { createFoldMemory, isLongBody, messageTexts, needsExpand, workFold, workFoldLabel } from "./fold";
import { assignSends, chipKey, chipModel, matchLocal, splitChips, type SentLocal } from "./chips";
import type { CommandView } from "./commands";
import { HEADER_MORE, resolveEntries } from "./menuModel";
import { MenuList } from "./MenuList";
import { shortcutById, tipWithKey } from "./shortcuts";
import { GoalBar, UnverifiedTag, WORK_MODE_TEXT } from "./PrefsDialogs";
import { resendCtxOf, resendTargetOf } from "./ResendDialog";
import { SkillSuggest, type SkillPickApi } from "./SkillPicker";
import { filterSkills, isDisabled, originLabel, queryTerms, removeTrigger, skillTrigger } from "./skillPick";
import { PendingSendRow } from "./PendingSendRow";
import { SendStatusBar } from "./SendStatusBar";
import { queueCounts } from "./sendStatus";
import { HandoffNotice, PurposeLine, UsageChip, useHandoff, type HandoffUi } from "./HandoffNotice";
import { IntakeSection } from "./OrgIntake";
import { worktreeOf } from "./folderGroups";
import type { HandoffPreview } from "../ipc/types";
import { OrgInstructionBubble, OrgReportView } from "./OrgChatParts";
import { isOrgInstruction, parseOrgReport } from "./orgChat";
import { appearsInTurns, firstSendLockReason, type PendingSend } from "./pendingSend";
import { EnvelopeFold, PreviousThreads, usePrevThreads } from "./PreviousThreads";
import { splitEnvelope } from "./continuity";
import { PURPOSE_TAG, resendAvailability, type ResendCtx } from "./resend";

const NO_THREADS: ThreadSegmentView[] = [];
const SCOPE_TEXT = { once: "1回だけ", session: "このセッション中", persistent: "以後ずっと（永続）", unknown: "効力範囲は不明" } as const;

export interface CenterProps {
  /** 左（チャット一覧）・右（ドック）が今見えているか（狭い画面では一時表示の開閉）。ヘッダーの開閉ボタンの見た目に使う。 */
  leftShown: boolean;
  rightShown: boolean;
  snap: HostSnapshot;
  chat: Chat;
  turns: TurnRecord[];
  /** 送信前の添付（下書きに付いているもの）。 */
  attachments: AttachmentEntry[];
  /** 添付・成果物の操作（ホストが判断・保存する。UIは意図を伝えるだけ）。 */
  files: FileActions;
  /** 入力欄の `$` 候補の窓口（実接続のときだけ。モックは null）。 */
  skillPick: SkillPickApi | null;
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
  /** 作業フォルダの確認・変更（実接続のときだけ。モックは null）。 */
  wf: WorkFoldersApi | null;
  /** このチャットで送った発言のローカルの記録（送信直後の添付チップ用）。 */
  sends: SentLocal[];
  /** ヘッダーの作業フォルダの実在確認（読取りのメタデータ）。未取得は undefined。 */
  cwdInfo: PathInfo | undefined;
  /** 応答した承認・質問・MCP許可の記録（AgentDockが保存したもの。Codexの履歴には載らない）。 */
  decisions: DecisionItem[];
  /** 送信ボタンを押した直後からの依頼（送信中・未送信・受理不明・受理後の本文待ち）。 */
  pending: PendingSend[];
  onPendingRestore: (p: PendingSend) => void;
  onPendingDismiss: (p: PendingSend) => void;
  /** 引き継ぎ資料の確認（読取りのみ。モックでは仮の資料）。 */
  loadHandoffPreview?: (chat: ChatKey) => Promise<HandoffPreview>;
  /** 「引き継いで送信」。受け付けたら true（下書きを消す）。止まったら false。 */
  onHandoffSend?: (text: string, attachments: string[]) => boolean | Promise<boolean>;
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
  /** キュー項目を追加指示として今送る（押したとき1回だけ）。受理されたら元の項目を消す。 */
  steerEntry: (e: QueueEntry) => Promise<void>;
  /** 追加指示として送って受理不明のままの項目（項目ID→試行ID）。 */
  steerUnknown: Record<string, string>;
  /** 受理不明の追加指示を履歴と照合する（読み取りのみ。再送しない）。 */
  steerReconcile: (entryId: string) => void;
}

/** 追加指示にできない理由（できるときは null）。追加指示はモデル・権限・作業フォルダの変更には使えず、実行中のturnにだけ送れる。 */
function steerBlockReason(running: boolean, changed: boolean, local: ChatLocalView | undefined): string | null {
  if (!running) return "実行中のturnがないため、追加指示にはできません";
  if (changed) return "モデル・推論の強さ・速度を変更中のため、追加指示にはできません（変更は次のturnから適用されます）";
  if (local?.nextCwd) return "作業フォルダの変更が予定されているため、追加指示にはできません";
  return null;
}

/** 選択したモデル設定が、Codex が受理した値と違うか（受理を受け取っていなければ false）。 */
function modelChanged(s: ChatModelSettings | undefined): boolean {
  const sel = s?.selected;
  const acc = s?.accepted.kind === "value" ? s.accepted.value : null;
  if (!sel || !acc) return false;
  return acc.model !== sel.model || (acc.effort ?? "") !== (sel.effort ?? "") || (acc.speedTier ?? "") !== ((sel.speedTier ?? "") === "default" ? "" : (sel.speedTier ?? ""));
}


const PERMISSION_LABEL: Record<PermissionPreset, string> = {
  workspaceWriteOnRequest: "作業領域＋要確認",
  readOnly: "読み取りのみ",
  workspaceWriteUntrusted: "作業領域＋都度確認（組織）",
  readOnlyUntrusted: "読み取りのみ＋都度確認（組織）",
  fullAccess: "フルアクセス",
};
/** 短い表示名の全文（title とポップアップの項目 title に使う）。 */
const PERMISSION_FULL: Record<PermissionPreset, string> = {
  workspaceWriteOnRequest: "作業領域内の書込み＋必要時に確認（初期値）",
  readOnly: "読み取りのみ",
  workspaceWriteUntrusted: "作業領域内の書込み＋信頼できない操作は確認（組織の役割チャット）",
  readOnlyUntrusted: "読み取りのみ＋信頼できない操作は確認（組織の役割チャット）",
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

/** 「?」（説明の移し先）。押すと小さなポップオーバーで説明を読める（ホバーは title）。 */
function Help({ text }: { text: string }) {
  // summary の中に置いても、押して details を開閉しないようにする
  return <span className="helpwrap" onClick={(e) => e.preventDefault()}><Popover trigger="?" label="説明" title={text} triggerClassName="help" textual>{text}</Popover></span>;
}

/** 会話ヘッダーの状態チップの色種別（表示だけ。判定はホストの状態から）。 */
export function chatChipClass(snap: HostSnapshot, chat: Chat): string {
  if (chatRequests(snap, chat).length) return "wait";
  const root = rootView(snap, chat);
  if (chat.noHistory || !root || chat.origin === "external") return "unk";
  if (chatDisplay(snap, chat) === "childFail") return "fail";
  return STATE[root.status.state].c;
}

const NO_HISTORY_TEXT = "発話前のチャットなどは、Codex の一覧に載りません。このアプリの記録（ピン・下書き・モデル選択）だけを表示しています。履歴を作る送信は、再開できる状態になってから行えます。";
const EXTERNAL_RESUMED_TEXT = "ユーザーの確認のうえで再開しました。続きを送れます。元の作業フォルダやファイルは参照のみで、削除の対象にしません。";
const EXTERNAL_REF_TEXT = "元の作業フォルダやファイルは参照のみで、削除の対象にしません。";
const STALE_TAIL = "実行中の途中経過は表示されません。送信待ちは照合が終わるまで保留します。";

/** 鮮度の説明（live以外）。再起動後・接続回復後の文言は旧帯のものをそのまま使う。 */
function freshnessText(root: AgentView | undefined, wasLive: boolean): string {
  if (root?.freshness !== "historyOnly") return "収集の鮮度。エージェントの状態とは別";
  const t = hms(root.status.evidence.observedAt);
  return wasLive
    ? `接続を回復しましたが、ライブ監視はまだ戻っていません。保存履歴から取得した状態です（${t}）。${STALE_TAIL}`
    : `アプリの再起動後のため、保存履歴から表示しています。送信すると続きから再開します（再開するまでライブ監視はしません）。保存履歴から取得した状態です（${t}）。${STALE_TAIL}`;
}

function Header({ snap, chat, onAct, commands, local, onCwdToggle, wasLive, externalLabel, cwdInfo, leftShown, rightShown }: {
  leftShown: boolean; rightShown: boolean;
  snap: HostSnapshot; chat: Chat; onAct: (a: string) => void; commands: CommandView[]; local: ChatLocalView | undefined; cwdInfo: PathInfo | undefined;
  onCwdToggle: () => void; wasLive: boolean; externalLabel: string | undefined;
}) {
  const root = rootView(snap, chat);
  const fresh = FRESH[root?.freshness ?? "needsReconcile"];
  const isLive = root?.freshness === "live";
  const cwd = knownValue(chat.cwd);
  const ttl = chatTitle(chat);
  const dev = chat.kind === "development";
  const freshTip = freshnessText(root, wasLive);
  return (
    <div className="chead">
      <button className="btn subtle icon panebtn" aria-label="チャット一覧" aria-pressed={leftShown} title={tipWithKey(`チャット一覧を${leftShown ? "閉じる" : "開く"}`, "toggleLeft")} onClick={() => onAct("toggleLeft")}><Icon name="list" /></button>
      <span className="ttl" style={ttl.confirmed ? undefined : TENTATIVE_STYLE} title={`${ttl.text}（Codex）${ttl.confirmed ? "" : `\n${TENTATIVE_HINT}`}`}>{ttl.text}</span>
      {/* 状態と鮮度は別表示。片方から他方を導かない */}
      <span className={`st ${chatChipClass(snap, chat)}`}><i />{chatStatusText(snap, chat)}</span>
      {isLive ? (
        <span className={`fresh ${fresh.c}`} title="収集の鮮度。エージェントの状態とは別">{fresh.t}</span>
      ) : (
        <Popover trigger={fresh.t} title={freshTip} triggerClassName={`fresh ${fresh.c} fbtn`} textual>
          {(close) => (
            <>
              <div>{freshTip}</div>
              <div className="pop-acts"><button className="btn sm" title="保存履歴を読み直して状態を取り直します（読み取りのみ）" onClick={() => { close(); onAct("reloadHistory"); }}>状態を再照合</button></div>
            </>
          )}
        </Popover>
      )}
      {cwd ? (
        dev
          ? <button className="cwd mono" title={`作業フォルダ: ${cwd}\n押すと、作業フォルダの確認と変更を開きます`} onClick={onCwdToggle}>{cwd}</button>
          : <span className="cwd mono" title={`作業フォルダ: ${cwd}`}>{cwd}</span>
      ) : <span className="cwd" title="一般チャット（作業フォルダなし）">一般</span>}
      {cwd ? (() => {
        // 押したときだけ開く。ネットワークパス・実在しないフォルダは押せない（理由はツールチップ）
        const why = /^[\\/]{2}[^?.]/.test(cwd.trim()) ? "ネットワークパスは開きません（接続するとWindowsの認証情報が相手に送信されることがあります）"
          : cwdInfo?.kind === "missing" ? "フォルダが見つかりません。移動または削除された可能性があります"
          : cwdInfo?.kind === "unconfirmed" ? "フォルダの有無を確認できていません" : null;
        return <button className="btn subtle icon cwdopen" aria-label="作業フォルダを開く" aria-disabled={why ? true : undefined}
          title={why ?? `作業フォルダをエクスプローラーで開く\n${cwd}`} onClick={() => { if (!why) onAct("openCwd"); }}><Icon name="folderOpen" /></button>;
      })() : null}
      {(() => { const m = multiFolderMark(local); return m ? <span className="tag" title={m.title}>{m.text}</span> : null; })()}
      {local?.nextCwd ? <span className="tag mono" title={`次の送信から使う作業フォルダ（まだ適用していません）: ${local.nextCwd}`}>次: {baseName(local.nextCwd)}</span> : null}
      {local?.reviewOf ? <span className="tag" title={`このチャットはレビュー用に作られました（レビュー元: ${origName(snap, local.reviewOf)}）`}>レビュー</span> : null}
      {local?.forkOf ? <span className="tag" title={`このチャットは${local.forkOf.purpose === "editResend" ? "依頼を編集して送り直すために" : local.forkOf.purpose === "regenerate" ? "応答を作り直すために" : ""}分岐で作られました（AgentDockの記録）。分岐元: ${origName(snap, local.forkOf.chat)}${local.forkOf.throughTurn ? `（turn ${local.forkOf.throughTurn} まで）` : ""}`}>{PURPOSE_TAG[local.forkOf.purpose ?? "fork"]}</span> : null}
      {chat.noHistory ? <span className="tag" title={`Codex に、この会話の履歴がありません。${NO_HISTORY_TEXT}`}>履歴なし</span> : null}
      {chat.origin === "external" && isLive ? <span className="tag" title={`${externalLabel ?? "外部"} で作成された会話を、このアプリで再開済みです。${EXTERNAL_RESUMED_TEXT}`}>{externalLabel ?? "外部"}で作成</span> : null}
      <span className="grow" />
      {/* このチャットの近道。項目・名前・無効理由は上部メニューと同じ台帳（menuModel の HEADER_MORE）から作る */}
      <Popover trigger={<Icon name="more" />} label="このチャットの操作" title={`変更・会話・このチャットの操作と、すべての操作${commands.some((v) => v.mark) ? "\n記録できない変更があります（「変更を確認…」から案内を表示）" : ""}`} role="menu" align="right" triggerClassName="btn subtle icon">
        {(close) => <MenuList entries={resolveEntries(HEADER_MORE, commands)} onPick={(a) => { close(); onAct(a); }} />}
      </Popover>
      <button className="btn subtle icon panebtn" aria-label="エージェントのドック" aria-pressed={rightShown} title={tipWithKey(`エージェントのドックを${rightShown ? "閉じる" : "開く"}`, "toggleRight")} onClick={() => onAct("toggleRight")}><Icon name="dock" /></button>
    </div>
  );
}

function targetName(snap: HostSnapshot, c: Chat, t: StopRecord["targets"][number]): string {
  const ref = t.target;
  if (ref.kind === "turn") {
    const v = chatAgents(snap, c).find((x) => keyStr(x.agent.key) === keyStr(ref.turn.agent));
    return v ? showKnown(v.agent.displayName) : "エージェント";
  }
  if (ref.kind === "managedExec") return `${showKnown(chatAgents(snap, c).find((x) => keyStr(x.agent.key) === keyStr(ref.agent))?.agent.displayName ?? { kind: "notFetched" })} の管理実行（${ref.execId}）`;
  return `OSプロセス PID ${ref.pid}`;
}

function StopBanner({ snap, chat, rec, onAct }: { snap: HostSnapshot; chat: Chat; rec: StopRecord; onAct: (a: string) => void }) {
  return (
    <div className="cbanner warn" role="alert">
      <h4>停止を確認できていない対象があります（中断要求後・ホストの判定）<Help text="時間の経過だけで停止や失敗とは判断しません。停止が確認できるまで、削除と新しい送信は保留します。" /></h4>
      <ul className="targets">
        {rec.targets.map((t, i) => (
          <li key={i}><b>{targetName(snap, chat, t)}</b><span>{STOP_LABEL[t.summary]}</span></li>
        ))}
      </ul>
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
      {chat.origin === "external" && root?.freshness !== "live" ? (
        <div className="cbanner warn">
          <h4>{p.externalLabel ?? "外部"} で作成された会話です（閲覧のみ）</h4>
          外部でまだ実行中かどうか、このアプリでは確認できません。外部での実行が終わったことを確認してから再開すると、このアプリで続きを送れます。<Help text={EXTERNAL_REF_TEXT} />
          {root && (root.status.state === "running" || root.status.state === "waiting") ? <div style={{ marginTop: 4 }}><b>最新のturnが実行中として記録されています。外部で実行中の可能性があります。</b></div> : null}
          <div className="acts"><button className="btn-main" onClick={() => onAct("resumeExternal")}>外部での実行は終わっています。この会話を再開する</button></div>
        </div>
      ) : null}
      {root && root.freshness === "historyOnly" && chat.origin !== "external" && p.wasLive ? (
        <div className="cbanner info slim" role="alert" title={freshnessText(root, true)}>
          <span className="grow">ライブ監視は未復帰（履歴 {hms(root.status.evidence.observedAt)}）</span>
          <button className="btn sm" title="保存履歴を読み直して状態を取り直します（読み取りのみ）" onClick={() => onAct("reloadHistory")}>状態を再照合</button>
        </div>
      ) : null}
      {stops.map((r) => <StopBanner key={r.id} snap={snap} chat={chat} rec={r} onAct={onAct} />)}
      {p.save && p.save.kind === "saveFailed" ? (
        <div className="cbanner err" role="alert">
          <h4>一部を保存できませんでした</h4>
          <span title="保存できていない内容を保存済みとは表示しません。">保存済み: {p.save.savedPart ?? "なし"}。未保存: {p.save.unsavedPart ?? "不明"}（{p.save.message}）。</span>
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

/** 「ファイル」欄をユーザーが開いたかをチャットごとに覚える（メモリ内のみ。既定は閉じる）。 */
const filesOpen = new Map<string, boolean>();

const isImageName =(n: string): boolean => mimeOf(n).startsWith("image/");

/** 会話内のファイル項目（送信した添付と、実在を確認した成果物）。開く・保存はユーザーの操作だけ。 */
function FilesBlock({ chatId, sent, artifacts, files }: { chatId: string; sent: AttachmentEntry[]; artifacts: ArtifactEntry[]; files: FileActions }) {
  if (!sent.length && !artifacts.length) return null;
  // 注意が要るもの（欠損・コピー失敗・実在を確認できていない成果物）があれば、閉じていても見出しに印を残す。
  const attention = sent.some((a) => attachmentStatus(a).bad || a.state.kind !== "ready") || artifacts.some((a) => a.exists.kind !== "value" || !a.exists.value);
  return (
    <div className="msg ai files">
      <details open={filesOpen.get(chatId) ?? false} onToggle={(e) => { filesOpen.set(chatId, e.currentTarget.open); }}>
        <summary><Icon name="clip" /><b>ファイル（添付 {sent.length}件・成果物 {artifacts.length}件）</b>{attention ? <span className="tag" title="欠損・コピー失敗・実在未確認のファイルがあります。開いて確認してください">要確認</span> : null}<Help text="ファイルを表示できることと、モデルが内容を読めることは別です。外部アプリでは、「開く」を押したときだけ開きます。" /></summary>
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
                <div className="mono small muted"><PathText path={a.path} />{a.inChatArea ? "" : <>　<span className="tag" title="作業フォルダ側のファイル。チャットを削除しても消しません">作業フォルダ側</span></>}</div>
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

/** 応答の原文（Markdownのまま）をコピーするボタン。結果は2秒表示し読み上げる。 */
function CopyAct({ text }: { text: string }) {
  const { state, copy } = useCopy();
  return <button onClick={() => copy(text)} aria-live="polite"><Icon name="copy" />{state === "idle" ? COPY_LABEL.idle : COPY_LABEL[state]}</button>;
}

interface FindState { open: boolean; query: string; setQuery: (q: string) => void; tick: number; partial: boolean; close: () => void }

/** 依頼・応答の操作「編集して分岐」「もう一度」。無効でも押せて（aria-disabled）、押すと理由をダイアログで示す。 */
interface ResendView { ctx: ResendCtx; rootAgentId: string | undefined }

/** 画像のプレビューは、画面に入るまで読み込まない（古い発言のチップで重い読込みをしない）。 */
function LazyThumb(props: Parameters<typeof Thumb>[0]) {
  const [seen, setSeen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (seen || !el) return;
    if (typeof IntersectionObserver === "undefined") { setSeen(true); return; }
    const io = new IntersectionObserver((es) => { if (es.some((e) => e.isIntersecting)) { setSeen(true); io.disconnect(); } });
    io.observe(el);
    return () => io.disconnect();
  }, [seen]);
  return seen ? <Thumb {...props} /> : <span ref={ref} className="thumb" title="画像（読み込み待ち）" />;
}

/**
 * 発言に付いていた添付のチップ（最大3件＋「＋N」）。履歴の入力項目から復元したもので、受理の根拠ではない。
 * 成功の印は付けない。復元できないものは中立の「添付あり（詳細未確認）」。
 */
function MsgChips({ atts, locals, files }: { atts: EntryAttachment[]; locals: AttachmentEntry[]; files: FileActions }) {
  const [all, setAll] = useState(false);
  const models = atts.map((a, i) => ({ k: chipKey(a, i), m: chipModel(a), local: matchLocal(a.path, locals) }));
  const { shown, more } = splitChips(models, all);
  return (
    <div className="uchips" role="group" aria-label={`添付 ${atts.length}件`}>
      {shown.map(({ k, m, local }) => {
        const target: FileRef | null = local ? { kind: "attachment", chat: local.chat, id: local.id } : null;
        const gone = !!local && (local.state.kind === "missing" || local.state.kind === "copyFailed");
        const ready = !!local && local.state.kind === "ready";
        const isImg = m.kind === "image";
        const lead = isImg && target && ready ? <LazyThumb target={target} name={m.label} load={files.preview} /> : <Icon name="clip" />;
        const inner = (
          <>
            {lead}
            <span className={`uname${m.neutral ? " neutral" : ""}`}>{m.label}</span>
            {gone ? <span className="tag" title="このアプリが保持していた添付のコピーが見つかりません">欠損</span> : null}
          </>
        );
        const title = gone ? `${m.title}\nコピーが見つかりません（欠損）` : m.title;
        // 画像はプレビュー（拡大）、その他の保持済みファイルは「開く」と同じ操作。それ以外は表示のみ。
        return !isImg && target && ready && m.kind !== "skill"
          ? <button key={k} type="button" className="uchip" title={title} aria-label={`${m.label}を関連アプリで開く`} onClick={() => files.open(target)}>{inner}</button>
          : <span key={k} className={`uchip${m.neutral ? " neutral" : ""}${gone ? " bad" : ""}`} title={title}>{inner}</span>;
      })}
      {more > 0 || (all && models.length > 3)
        ? <button type="button" className="uchip more" aria-expanded={all} title={all ? "3件だけ表示" : `残り${more}件を表示`} onClick={() => setAll((v) => !v)}>{all ? "閉じる" : `＋${more}`}</button>
        : null}
    </div>
  );
}

/** チャットごとの展開状態（メモリ内。別チャットへ切り替えて戻っても保つ）。 */
const foldMem = createFoldMemory(50);

/** ユーザーの吹き出し。長い依頼文（8行超または600文字超）は数行で切り、「展開」で全文を出す。全文はDOMに残るので検索の件数は変わらない。 */
function UserBubble({ text, forceOpen, open, onToggle }: { text: string; forceOpen: boolean; open: boolean; onToggle: () => void }) {
  const long = isLongBody(text);
  const id = useId();
  const shown = open || forceOpen;
  return (
    <>
      <div id={id} className={`bubble${long && !shown ? " clamp" : ""}`} data-find="body">{text}</div>
      {long ? <button type="button" className="foldmsg" aria-expanded={shown} aria-controls={id} onClick={onToggle}>{shown ? "折りたたむ" : "展開"}</button> : null}
    </>
  );
}

function Messages({ orgBound, headKey, threads, chatId, turns, reqs, gaps, running, onRespond, onAct, extra, find, resend, locals, files, sends, agentName, decisions, pending, onPendingRestore, onPendingDismiss }: {
  orgBound: boolean; headKey: ChatKey; threads: ThreadSegmentView[]; agentName?: (a: AgentKey) => string; decisions: DecisionItem[];
  pending: PendingSend[]; onPendingRestore: (p: PendingSend) => void; onPendingDismiss: (p: PendingSend) => void;
  turns: TurnRecord[]; reqs: PendingRequest[]; /** 回答を届けられなかった要求（理由を常設表示する） */ gaps: PendingRequest[]; running: boolean; onRespond: CenterProps["onRespond"]; onAct: (a: string) => void; extra?: ReactNode; find: FindState; resend: ResendView;
  locals: AttachmentEntry[]; files: FileActions; sends: SentLocal[]; chatId: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const prev = usePrevThreads(headKey, threads);
  const [away, setAway] = useState(false);
  // 長い依頼文の展開状態（依頼の項目単位。チャット別に覚える）
  const [openMsgs, setOpenMsgs] = useState<string[]>(() => foldMem.get(chatId).openMsgs);
  // 完了したturnの「作業の経過」の展開状態（turn単位。チャット別に覚える）
  const [openWork, setOpenWork] = useState<string[]>(() => foldMem.get(chatId).openWork);
  const memo = useRef({ openMsgs, openWork });
  const remember = (p: Partial<typeof memo.current>) => { memo.current = { ...memo.current, ...p }; foldMem.set(chatId, memo.current); };
  const toggleMsg = (id: string) => setOpenMsgs((o) => { const n = o.includes(id) ? o.filter((x) => x !== id) : [...o, id]; remember({ openMsgs: n }); return n; });
  const toggleWork = (id: string) => setOpenWork((o) => { const n = o.includes(id) ? o.filter((x) => x !== id) : [...o, id]; remember({ openWork: n }); return n; });
  // 送った直後（履歴・live通知がまだ添付を運ばない間）は、送信時のローカルの記録から添付を出す
  const sentMap = useMemo(() => assignSends(turns, sends), [turns, sends]);
  const placed = useMemo(() => placeDecisions(turns, decisions), [turns, decisions]);
  // 組織チャットの最初の利用者発話（組織が自動送信した指示かの判定対象。ここだけを畳む）
  const firstUserId = useMemo(() => {
    if (!orgBound) return null;
    for (const t of turns) for (const e of t.entries) if (e.kind.kind === "userMessage") return e.key.itemId;
    return null;
  }, [orgBound, turns]);
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
  }, [turns, reqs.length, running, pending.length]);
  return (
    <div className="msgs-wrap">
    {find.open ? <FindBar root={ref} query={find.query} setQuery={find.setQuery} focusTick={find.tick} partial={find.partial} unloadedPrev={prev.unloaded} onClose={find.close} /> : null}
    <div className="msgs" id="msgs" aria-live="polite" ref={ref} onScroll={onScroll}>
      <PreviousThreads threads={threads} prev={prev} />
      {placed.head.map((it) => <DecisionRow key={it.record.id} it={it} agentName={agentName} />)}
      {turns.map((t) => {
        // 完了したturnの途中経過と作業の記録は「作業の経過」に畳む（実行中・失敗・中断・phase不明は畳まない）
        const wf = workFold(t);
        const wfSet = new Set(wf?.ids ?? []);
        const wfKey = keyStr(t.key.agent) + t.key.turnId;
        const wfEntries = t.entries.filter((e) => wfSet.has(e.key.itemId));
        // 検索の語が畳み内の途中経過にあれば自動で展開する
        const wfForce = !!wf && find.open && !!find.query && needsExpand(messageTexts([{ entries: wfEntries.filter((e) => e.kind.kind === "agentMessage") }]), find.query);
        const wfOpen = openWork.includes(wfKey) || wfForce;
        let wfDone = false;
        const wfBlock = wf ? (
          <div className="workfold" key="workfold">
            <div className="foldrow">
              <button type="button" className="foldbtn" aria-expanded={wfOpen} disabled={wfForce && !openWork.includes(wfKey)}
                title={wfForce && !openWork.includes(wfKey) ? "検索の一致が作業の経過にあるため、展開しています" : undefined} onClick={() => toggleWork(wfKey)}>
                {workFoldLabel(wf)} {wfOpen ? "‹" : "›"}
              </button>
            </div>
            {wfOpen ? wfEntries.map((e) => e.kind.kind === "agentMessage" ? (
              <div className="msg ai" key={e.key.itemId}>
                <div className="who">Codex</div>
                <div className="bubble" data-find="body"><Markdown text={e.text.kind === "value" ? e.text.value : "…"} streaming={false} /></div>
              </div>
            ) : (
              <div className="msg ai" key={e.key.itemId}><div className="activity"><b>{kindLabel(e.kind)}</b>　{entryText(e)}</div></div>
            )) : null}
          </div>
        ) : null;
        const others = wf ? [] : t.entries.filter((e) => e.kind.kind !== "userMessage" && e.kind.kind !== "agentMessage");
        // 編集・再生成の対象はメインのエージェントのturnだけ。
        const mine = resend.rootAgentId === undefined || t.key.agent.id === resend.rootAgentId;
        const blocked = (purpose: "editResend" | "regenerate", itemId: string | null) =>
          resendAvailability(resend.ctx, resendTargetOf(turns, t.key.turnId, purpose, itemId)).kind === "blocked";
        return (
          <div key={keyStr(t.key.agent) + t.key.turnId}>
            {t.entries.filter((e) => (e.kind.kind === "userMessage" || e.kind.kind === "agentMessage") && !wfSet.has(e.key.itemId)).map((e, _i, all) => {
              const text = e.text.kind === "value" ? e.text.value : "…";
              const lastAi = all.filter((x) => x.kind.kind === "agentMessage").pop();
              // 畳み見出しは、最初の返答（依頼の後）の直前に置く
              const lead = !wfDone && e.kind.kind !== "userMessage" ? (wfDone = true, wfBlock) : null;
              if (e.kind.kind === "userMessage") {
                // 添付だけの依頼は本文の項目がない。空の吹き出しにせず、中立の表示にする（コピーは本文があるときだけ）。
                // 資料つきの依頼は、利用者の文だけを吹き出しにし、資料は折りたたみの行へ（コピー・再送の基準も利用者の文）
                const env = e.text.kind === "value" ? splitEnvelope(e.text.value) : null;
                const utext = env ? env.userText : text;
                const noBody = e.text.kind !== "value" || !utext.trim();
                const atts = (e.attachments?.length ? e.attachments : sentMap.get(e.key.itemId)) ?? [];
                return (
                  <div className="msg user" key={e.key.itemId}>
                    {e.key.itemId === firstUserId && !noBody && isOrgInstruction(utext)
                      ? <OrgInstructionBubble text={utext} forceOpen={find.open && needsExpand([utext], find.query)} open={openMsgs.includes(e.key.itemId)} onToggle={() => toggleMsg(e.key.itemId)} />
                      : <>
                    <div className="sr">あなた</div>
                    {env?.envelope ? <EnvelopeFold env={env} /> : null}
                    {noBody
                      ? (atts.length ? null : <div className="bubble" data-find="body"><span className="muted">（本文なし）</span></div>)
                      : <UserBubble text={utext} forceOpen={find.open && needsExpand([utext], find.query)} open={openMsgs.includes(e.key.itemId)} onToggle={() => toggleMsg(e.key.itemId)} />}
                    </>}
                    {atts.length ? <MsgChips atts={atts} locals={locals} files={files} /> : null}
                    <div className="acts">
                      {noBody ? null : <CopyAct text={utext} />}
                      {mine ? <button aria-disabled={blocked("editResend", e.key.itemId) || undefined} onClick={() => onAct(`resend:editResend|${t.key.turnId}|${e.key.itemId}`)}><Icon name="branch" />編集して分岐…</button> : null}
                    </div>
                  </div>);
              }
              return (
                <Fragment key={e.key.itemId}>
                {lead}
                <div className="msg ai">
                  <div className="who">Codex</div>
                  <div className="bubble" data-find="body">{(() => { const rep = orgBound && !(t.end === null && e === lastAi) ? parseOrgReport(text) : null; return rep ? <OrgReportView report={rep} raw={text} /> : <Markdown text={text} streaming={t.end === null && e === lastAi} />; })()}</div>
                  <div className="acts"><CopyAct text={text} /><button onClick={() => onAct(`fork:${t.key.turnId}`)}><Icon name="branch" />ここから分岐</button>
                    {mine && e === lastAi ? <button aria-disabled={blocked("regenerate", null) || undefined} onClick={() => onAct(`resend:regenerate|${t.key.turnId}|`)}><Icon name="refresh" />もう一度…</button> : null}</div>
                </div>
                </Fragment>
              );
            })}
            {wfBlock && !wfDone ? wfBlock : null}
            {others.length ? (
              <div className="msg ai">
                <details className="activity"><summary>作業の記録（{others.length}件）</summary>
                  <ul style={{ margin: "4px 0", paddingLeft: 18 }}>{others.map((e) => <li key={e.key.itemId}><b>{kindLabel(e.kind)}</b>　{entryText(e)}</li>)}</ul>
                </details>
              </div>
            ) : null}
            {(placed.byTurn.get(slotKey(t.key.agent.id, t.key.turnId)) ?? []).map((it) => <DecisionRow key={it.record.id} it={it} agentName={agentName} />)}
          </div>
        );
      })}
      {pending.filter((x) => !appearsInTurns(x, turns)).map((x) => <PendingSendRow key={x.id} p={x} onRestore={onPendingRestore} onDismiss={onPendingDismiss} />)}
      {extra}
      {gaps.map((r) => <RequestGapLine key={`gap:${r.key.source}/${r.key.requestId}`} r={r} guide={orgBound ? "org" : "plain"} />)}
      {reqs.map((r) => <RequestCard key={keyStr({ backend: r.key.backend, id: r.key.requestId })} r={r} onRespond={onRespond} agentName={agentName} />)}
      {running && reqs.length === 0 ? <div className="msg ai"><div className="who">Codex</div><div className="activity">作業中…</div></div> : null}
    </div>
    {away ? <button className="jump" onClick={toBottom}>最新へ</button> : null}
    </div>
  );
}

/** 応答した承認・質問・MCP許可の小さな行。応答の事実だけを出す（再実行・再送のボタンは置かない）。 */
function DecisionRow({ it, agentName }: { it: DecisionItem; agentName?: (a: AgentKey) => string }) {
  const r = it.record;
  const { text, failed } = decisionLine(it, agentName ? agentName(r.agent) : null);
  return (
    <div className="msg ai decision" role="note" title={`応答した時刻: ${new Date(r.at).toLocaleString()}`}>
      <div className="activity"><span className="muted">{new Date(r.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>　{text}
        {failed ? <span className="err" title={failed}>　（この記録は保存できていません。再起動後には残りません）</span> : null}</div>
    </div>
  );
}

function optClass(o: DecisionOption, first: boolean): string {
  if (o.effect === "deny" || o.effect === "cancel") return "btn-danger";
  return first ? "btn-main" : "btn-line";
}

/** MCPの許可確認（elicitation）。承諾は「入力項目なし」の型だけ（ホストが選択肢に出す）。出せないものは灰色の破線で理由と案内を示す。 */
function ElicitationCard({ r, onRespond, agentName }: { r: PendingRequest; onRespond: CenterProps["onRespond"]; agentName?: (a: AgentKey) => string }) {
  const d = r.detail;
  const server = d.server.kind === "value" ? d.server.value : null;
  const canAccept = r.options.some((o) => o.id === "accept");
  const who = agentName ? agentName(r.agent) : "エージェント";
  return (
    <div className="req" role="group" aria-label="MCPの許可確認">
      <div className="h"><Icon name="alert" />{server ? `MCPサーバー「${server}」からの許可の確認です` : "MCPサーバーからの許可の確認です"}</div>
      <div className="t">要求元: <span title={`ID: ${r.agent.id}`}>{who}</span>（この確認が終わるまで、一覧の子の表示は「承認待ち」、ヘッダーは「対応待ち」になります）</div>
      <div className="t small muted" lang="en" title="MCPサーバーが示した原文です">原文: {d.summary}</div>
      {d.url.kind === "value" ? <code>{d.url.value}</code> : null}
      <div className="opts">
        {r.options.map((o, i) => (
          <button key={o.id} className={optClass(o, i === 0)} title={o.description ?? undefined} onClick={() => onRespond(r, { kind: "decision", optionId: o.id })}>{o.label}</button>
        ))}
      </div>
      {!canAccept ? (
        <div className="req-na" role="note">
          <b>許可する: ここでは選べません</b>
          <div>入力項目のある確認、またはURLを開く確認のため、AgentDockでは許可できません。許可するときは、Codexのアプリ／CLIで許可してください。</div>
        </div>
      ) : null}
      <div className="scope">辞退は「今回は許可しない」（明示的に拒否）、取りやめは「許可も拒否もせず取りやめる」です。どちらもこの確認にだけ作用します。</div>
    </div>
  );
}

export function RequestCard({ r, onRespond, agentName }: { r: PendingRequest; onRespond: CenterProps["onRespond"]; agentName?: (a: AgentKey) => string }) {
  if (r.kind.kind === "toolElicitation") return <ElicitationCard r={r} onRespond={onRespond} agentName={agentName} />;
  const isQuestion = r.kind.kind === "userInput";
  const d = r.detail;
  return (
    <div className="req" role="group" aria-label={isQuestion ? "質問" : "承認要求"}>
      <div className="h"><Icon name={isQuestion ? "chat" : "alert"} />{d.summary}</div>
      <div className="t">要求元: <span title={`ID: ${r.agent.id}`}>{agentName ? agentName(r.agent) : "エージェント"}</span>　作業フォルダ: <span className="mono">{showKnown(d.cwd)}</span></div>
      {d.command.kind === "value" ? <code>{d.command.value}</code> : null}
      {d.files.kind === "value" ? <code>{d.files.value.join("\n")}</code> : null}
      {isQuestion ? d.questions.map((q) => (
        <div key={q.id}>
          <div className="t">{q.text}</div>
          <div className="opts" title="選択肢と効力範囲は Codex が示したものを表示します。">
            {q.options.map((o, i) => (
              <button key={o.id} className={optClass(o, i === 0)} onClick={() => onRespond(r, { kind: "answers", answers: [{ questionId: q.id, optionId: o.id, text: null }] })}>{o.label}</button>
            ))}
          </div>
        </div>
      )) : null}
      {!isQuestion ? (
        <div className="opts">
          {r.options.map((o, i) => (
            <button key={o.id} className={optClass(o, i === 0)} title={SCOPE_TEXT[o.scope]} onClick={() => onRespond(r, { kind: "decision", optionId: o.id })}>{o.label}</button>
          ))}
        </div>
      ) : null}
      {!isQuestion ? <div className="scope" title="選択肢と効力範囲は Codex が示したものを表示します。">効力範囲: {r.options.map((o) => `${o.label}=${SCOPE_TEXT[o.scope]}`).join("／")}</div> : null}
    </div>
  );
}

function EntryRow({ n, e, qact, steerWhy, holdAttempt, holdHint }: { n: number; e: QueueEntry; qact: QueueActions; steerWhy: string | null; holdAttempt: string | null; holdHint: string | null }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(e.text);
  const [steering, setSteering] = useState(false);
  const s = e.state;
  // 追加指示の受理不明は、ホストがキューを保留している間だけ（照合で確定すれば消える）
  const steerUnk = qact.steerUnknown[e.id] && qact.steerUnknown[e.id] === holdAttempt ? qact.steerUnknown[e.id] : undefined;
  const why = steerUnk ? "追加指示として送りましたが、受理されたか確認できません。この依頼は残しています（再送しません）。履歴と照合して確定してください"
    : s.kind === "acceptanceUnknown" ? "受理不明: 送信したか確認できません。履歴と照合して確定するまで、再送しません"
    : s.kind === "sending" ? (e.applied?.unconfirmedDescendants ? `送信中（${unconfirmedNote(e.applied.unconfirmedDescendants)}）` : "送信中")
    : s.kind === "notAccepted" ? `受理されていません（${s.message}）。自動では再送しません`
    : null;
  const save = async () => { if (await qact.edit(e.id, text)) setEditing(false); };
  return (
    <div className="qitem">
      <span className="qnum">{n}</span>
      <div>
        {editing ? <textarea aria-label="依頼を編集" value={text} onChange={(ev) => setText(ev.target.value)} style={{ width: "100%" }} /> : e.text.trim() ? <div>{e.text}</div> : <div className="muted">（本文なし・添付のみ）</div>}
        {e.attachments.length ? <div className="small muted" title="このチャット用のコピーを送ります">添付 {e.attachments.length}件</div> : null}
        {why ? <div className={`why ${s.kind === "waiting" && !steerUnk ? "" : "err"}`}>{why}</div> : null}
      </div>
      <div style={{ display: "flex", gap: 2 }}>
        {s.kind === "waiting" && !editing ? (() => {
          // 追加指示（実行中のturnへ今すぐ）。条件を満たさないとき・受理不明の確認中は押せない（理由はツールチップ）
          const block = steerUnk ? "追加指示の受理を確認中です。先に「履歴と照合」で確定してください" : steering ? "送信中です" : steerWhy ? (holdHint ? `${steerWhy}（${holdHint}）` : steerWhy) : null;
          return <button className="small" aria-label="追加指示として今すぐ送る（実行中のturnへ割り込んで送ります）" aria-disabled={block ? true : undefined}
            title={block ?? "追加指示として今すぐ送る（実行中のturnへ割り込んで送ります。受理されたらこの依頼は取り消します）"}
            onClick={() => { if (block) return; setSteering(true); void qact.steerEntry(e).finally(() => setSteering(false)); }}><Icon name="send" />追加指示</button>;
        })() : null}
        {steerUnk ? <button className="small" title="履歴と照合して、追加指示の受理を確認します（読み取りのみ。再送しません）" onClick={() => qact.steerReconcile(e.id)}>履歴と照合</button> : null}
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
        <button className={h.kind === "stateUnknown" ? "btn-main" : "small"} title="保留中の子孫の履歴を読み直します（読み取りのみ。再開はしません）。終端を確認できれば、自動で送信に進みます" onClick={qact.recheck}>状態を再確認</button>
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

function Queue({ q, nameOf, qact, steerWhy }: { q: ChatQueue | undefined; nameOf: (a: AgentKey) => string; qact: QueueActions; steerWhy: string | null }) {
  if (!q) return null;
  const items = q.entries.filter((e) => e.state.kind !== "sent" && e.state.kind !== "cancelled");
  if (!items.length) return null;
  const unknown = items.some((e) => e.state.kind === "acceptanceUnknown");
  const stopped = q.run.kind !== "active";
  const unkKids = !stopped && q.hold?.kind === "stateUnknown" ? q.hold.targets.filter((t) => t.agent.id !== q.chat.id).length : 0;
  const holdHint = unkKids ? `子${unkKids}件の状態が不明です。下の「状態を再確認」で確認できます` : null;
  return (
    <div className="queue">
      {stopped ? (
        <div className="cbanner warn row" role="alert" style={{ margin: "0 0 6px" }}>
          <div className="grow" title="失敗した依頼そのものは再実行しません。「確認済み」にしてもキューは再開しません。">
            <b>{q.run.kind === "pausedAfterRestart" ? "再起動後のため、自動送信を止めています" : "自動送信を止めています"}</b>
            {q.run.kind === "stopped" ? stopCauseText(q.run.cause, nameOf) : "アプリを再起動したため、送信待ちの依頼は自動では送りません。内容を確認して再開してください。"}
          </div>
          <button className="btn-main" disabled={unknown} title={unknown ? "受理不明の依頼を照合で確定するまで再開できません" : undefined} onClick={qact.resume}>キューを再開</button>
        </div>
      ) : null}
      <details open>
        <summary title={!stopped && !q.hold ? "親と子孫の作業が終わると、登録順に1件ずつ送ります" : undefined}><Icon name="list" /><b>完了後に送る依頼 {items.length}件</b>
          <span className="muted" title={!stopped && q.hold && (q.hold.kind === "stateUnknown" || q.hold.kind === "notLive") && q.hold.targets.length ? targetList(q.hold.targets, nameOf) : undefined}>{stopped ? "停止中" : q.hold ? holdText(q.hold, nameOf, q.chat.id) : ""}</span>
          <span className="muted small qnote" title="送信待ちの依頼は、送信する時点のモデル・権限・作業フォルダで送ります。">送信時の設定で送信</span></summary>
        <HoldExit q={q} head={items[0]} nameOf={nameOf} qact={qact} />
        {items.map((e, i) => <EntryRow key={e.id + e.state.kind} n={i + 1} e={e} qact={qact} steerWhy={steerWhy} holdAttempt={q.hold?.kind === "acceptanceUnknown" ? q.hold.attempt : null} holdHint={holdHint} />)}
      </details>
    </div>
  );
}

/** Skillのチップの出所（取得済みの一覧にあるときだけ。無ければ出さない）。 */
function skillOrigin(f: AttachmentEntry, api: SkillPickApi | null): string | null {
  if (f.source.kind !== "skill") return null;
  const path = f.source.path;
  const s = api?.skills?.find((x) => x.path === path);
  return s ? originLabel(s) : null;
}

function Composer({ p, running, lock, hu }: { p: CenterProps; running: boolean; lock: string | null; hu: HandoffUi }) {
  const { settings, models, attachments } = p;
  const dflt = models.find((m) => m.isDefault) ?? models[0];
  const [model, setModel] = useState(settings?.selected?.model ?? dflt?.id ?? "");
  const [effort, setEffort] = useState(settings?.selected?.effort ?? dflt?.defaultEffort ?? "");
  const [speed, setSpeed] = useState(settings?.selected?.speedTier ?? "");
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
  // 追加指示は送るたびの1回限り（状態を持たない。次の送信は常に既定の「完了後に送信」）
  const empty = !p.draft.trim() && attachments.length === 0;
  const steerWhy = lock ? "いまは送信できません" : attachBlock ? "添付に問題があります" : empty ? "送る内容がありません" : steerBlockReason(running, changed, p.local);
  const send = async (asSteer = false) => {
    if (empty || lock || attachBlock) return;
    if (asSteer && steerWhy) return;
    const ids = attachments.map((a) => a.id);
    // 引き継ぎの案内中で、資料を確認でき止める理由がないときだけ「引き継いで送信」（送信キーも同じ）。それ以外は通常の送信
    const ok = hu.armed && !asSteer && !running && p.onHandoffSend ? await p.onHandoffSend(p.draft, ids) : await p.onSend(p.draft, running && asSteer, ids);
    if (ok) p.setDraft("");
  };
  const pickModel = (id: string) => {
    const e = models.find((m) => m.id === id)?.defaultEffort ?? "";
    // 速度の選択肢はモデルごとに違うので、モデルを変えたら指定なしに戻す。
    setModel(id); setEffort(e); setSpeed(""); p.onModel(id, e, "");
  };
  const pickEffort = (e: string) => { setEffort(e); p.onModel(model, e, speed); };
  const pickSpeed = (s: string) => { setSpeed(s); p.onModel(model, effort, s); };
  // 入力欄は2行（44px）から160pxまで内容に合わせて伸びる（表示だけ）
  useLayoutEffect(() => {
    const el = p.inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [p.draft, p.inputRef]);
  // ── `$`・`/skill ` で開くSkill候補（IME変換中は反応しない。候補が無い・未取得・モックは出さない）──
  const [caret, setCaret] = useState(0);
  const [composing, setComposing] = useState(false);
  const [suppress, setSuppress] = useState<number | null>(null);
  const [sugActive, setSugActive] = useState(0);
  const sugId = useId();
  const trig = useMemo(() => (p.skillPick && !lock ? skillTrigger(p.draft, caret) : null), [p.draft, caret, p.skillPick, lock]);
  useEffect(() => { if (!trig) setSuppress(null); }, [trig]);
  useEffect(() => { if (trig && !composing) p.skillPick?.load(); }, [!!trig, composing]); // eslint-disable-line react-hooks/exhaustive-deps
  const sugItems = useMemo(
    () => (trig && p.skillPick?.skills ? filterSkills(p.skillPick.skills.filter((x) => !isDisabled(x)), trig.query, p.skillPick.recent).items : []),
    [trig?.query, !!trig, p.skillPick?.skills, p.skillPick?.recent], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const sugOpen = !!trig && !composing && suppress !== trig.start && sugItems.length > 0;
  useEffect(() => { setSugActive(0); }, [trig?.query]);
  const sugCur = Math.min(sugActive, Math.max(sugItems.length - 1, 0));
  /** 候補を指定する。入力した `$語` の部分は消えてチップになる（送信はしない）。 */
  const pickSuggest = (s: SkillInfo) => {
    if (!trig || !p.skillPick) return;
    const r = removeTrigger(p.draft, trig);
    p.setDraft(r.text);
    setCaret(r.cursor);
    window.requestAnimationFrame(() => { const el = p.inputRef.current; if (el) { el.focus(); el.setSelectionRange(r.cursor, r.cursor); } });
    p.skillPick.pick(s);
  };
  const enterHint = p.enterMode === "ctrl" ? "Ctrl＋Enter で送信、Enter で改行" : "Enter で送信、Shift＋Enter で改行";
  // 受理の根拠。全文は title と「⋯」に常に置き、1行目には問題があるとき（未受理・受理未確認・不一致）だけ短く出す。
  const acceptLine = `${changed ? `選択: ${model}／${effort}${speed ? `／速度 ${speed}` : ""}（未受理。次のターンから適用）　` : ""}受理済み: ${settings ? (accepted ? `${accepted.model}／${accepted.effort ?? "既定"}${accepted.speedTier ? `／速度 ${accepted.speedTier}` : ""}` : showKnown(settings.accepted)) : "未確認"}${!accepted ? "（受け取るまで確認不可）" : ""}`;
  const acceptTip = !accepted ? "会話の再開・開始の応答か Codex の設定通知でモデルを受け取るまで、受理は確認できません。受け取っていない間は、選択値が使われたとは言えません。" : undefined;
  const wmLine = wmSelected || wmAccepted
    ? `計画／実行: 選択 ${wmSelected ? WORK_MODE_TEXT[wmSelected] : "未選択"}／受理済み ${wmAccepted ? WORK_MODE_TEXT[wmAccepted] : "未確認"}${wmSelected && wmAccepted !== wmSelected ? "（未受理。次のターンから適用）" : ""}`
    : null;
  const wmShort = (m: WorkMode) => (m === "plan" ? "計画" : "実行");
  const perm = p.local?.permission ?? "workspaceWriteOnRequest";
  const permOff = perm !== "workspaceWriteOnRequest";
  const speedKept = accepted?.speedTier && !speed ? accepted.speedTier : null;
  const speedKeptLine = speedKept ? `速度は「指定なし」ですが、Codex 側は速度 ${speedKept} のままです。標準へ戻すには「標準（default）」を選んで送信し、受理済みの表示で確認してください。` : null;
  const speedName = speed ? (speed === "default" ? "標準" : tiers.find((t) => t.id === speed)?.name ?? speed) : null;
  return (
    <div className="composer">
      <div className={`shell ${lock ? "locked" : ""}`}>
        {attachments.length ? (
          <div className="chips">
            {attachments.map((f) => {
              const st = attachmentStatus(f);
              const target: FileRef = { kind: "attachment", chat: f.chat, id: f.id };
              return (
                <span key={f.id} className={`chip ${st.bad ? "bad" : ""}`} title={f.source.kind === "skill" ? f.source.path : undefined}>
                  {f.kind === "image" && f.state.kind === "ready" ? <Thumb target={target} name={f.displayName} load={p.files.preview} /> : <Icon name="clip" />}
                  <span>{f.displayName}</span>{skillOrigin(f, p.skillPick) ? <span className="tag">{skillOrigin(f, p.skillPick)}</span> : null}<span className="st">{st.text}</span>
                  {f.state.kind === "ready" && f.kind !== "skill" ? <button className="small" aria-label={`${f.displayName}を関連アプリで開く`} onClick={() => p.files.open(target)}>開く</button> : null}
                  <button aria-label={`${f.displayName}を取り外す`} disabled={f.state.kind === "copying"} onClick={() => p.files.remove(f.id)}><Icon name="x" /></button>
                </span>
              );
            })}
            <Help text="送信前に確認・取り外せます。このチャット用にコピーした分を送ります（元のファイルは変更しません）。画像は表示できますが、モデルが内容を読めるかは未確認です。" />
          </div>
        ) : null}
        {lock ? <div className="lockmsg">{lock}</div> : null}
        <HandoffNotice ui={hu} onPreview={() => p.onAct("handoffPreview")} />
        {attachBlock ? <div className="lockmsg">{attachBlock}</div> : null}
        {wmSelected && !wmUsable ? (
          <div className="lockmsg">計画／実行（{WORK_MODE_TEXT[wmSelected]}）を選んだままですが、この Codex 接続では使えないため、送信は止まります。<button className="small" onClick={() => p.onWorkMode(null)}>選択を外す</button></div>
        ) : null}
        {sugOpen && p.skillPick?.skills ? <SkillSuggest items={sugItems} all={p.skillPick.skills} terms={queryTerms(trig!.query)} active={sugCur} listId={sugId} onHover={setSugActive} onPick={pickSuggest} /> : null}
        <textarea aria-label="メッセージ" disabled={!!lock} value={p.draft} onChange={(e) => { p.setDraft(e.target.value); setCaret(e.target.selectionStart); }} ref={p.inputRef}
          onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
          onCompositionStart={() => setComposing(true)} onCompositionEnd={(e) => { setComposing(false); setCaret(e.currentTarget.selectionStart); }}
          aria-controls={sugOpen ? sugId : undefined} aria-expanded={sugOpen || undefined} aria-activedescendant={sugOpen ? `${sugId}-${sugCur}` : undefined}
          placeholder={`${running ? "完了後に送る次の依頼" : "メッセージを入力"}（${enterHint}${running ? "／Ctrl＋Shift＋Enter で追加指示" : ""}）`}
          onPaste={(e) => {
            // クリップボードの画像は添付にする（生バイトでホストへ）。文章だけの貼り付けは通常どおり。
            const img = Array.from(e.clipboardData.files).find((f) => f.type.startsWith("image/"));
            if (!img) return;
            e.preventDefault();
            void img.arrayBuffer().then((buf) => p.files.pasteImage(clipboardImageName(new Date()), buf));
          }}
          onKeyDown={(e) => {
            if (sugOpen && !e.nativeEvent.isComposing) {
              const n = sugItems.length;
              if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); setSugActive((sugCur + (e.key === "ArrowDown" ? 1 : -1) + n) % n); return; }
              if ((e.key === "Enter" && !e.ctrlKey && !e.shiftKey && !e.altKey) || (e.key === "Tab" && !e.shiftKey)) { e.preventDefault(); pickSuggest(sugItems[sugCur]); return; }
              if (e.key === "Escape") { e.preventDefault(); setSuppress(trig!.start); return; }
            }
            if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
            // 追加指示で送る（実行中のみ。条件を満たさないときは何も送らない。理由は▾メニューの項目に出る）
            if (e.ctrlKey && e.shiftKey) { e.preventDefault(); if (running) void send(true); return; }
            if ((p.enterMode === "ctrl" && e.ctrlKey) || (p.enterMode === "enter" && !e.shiftKey)) { e.preventDefault(); void send(); }
          }} />
        <div className="ctrl">
          <div className="ctrl-l">
          <button aria-label="ファイル・画像・参考情報を追加" disabled={!!lock} onClick={() => p.onAct("attach")}><Icon name="clip" /></button>
          <span className="sel"><select aria-label="モデル" title="モデル（次の送信から適用）" value={model} onChange={(e) => pickModel(e.target.value)}>{models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></span>
          <span className="sel eff-inline"><select aria-label="推論の強さ" title="推論の強さ（次の送信から適用）" value={effort} onChange={(e) => pickEffort(e.target.value)}>{efforts.map((x) => <option key={x.id} value={x.id}>{x.id}</option>)}</select></span>
          {/* まれな設定（権限・速度・計画／実行）と受理の全文は「⋯」へ。既定から外れているときだけ、下の小さな印で示す */}
          <Popover trigger={<Icon name="more" />} label="送信の設定" title="次の送信に使う権限・推論の強さ・速度・計画／実行（と、受理の全文）" triggerClassName="btn subtle icon">
            <div className="moreset">
              <label>
                <span>権限</span>
                <select aria-label="権限" className={perm === "fullAccess" ? "danger" : undefined}
                  title={`次の送信から適用。送信待ちの依頼にも送信時に適用されます（現在: ${PERMISSION_FULL[perm]}）`}
                  value={perm} onChange={(e) => p.onPermission(e.target.value as PermissionPreset)}>
                  {(Object.keys(PERMISSION_LABEL) as PermissionPreset[]).filter((k) => k === perm || !k.endsWith("Untrusted")).map((k) => <option key={k} value={k} title={PERMISSION_FULL[k]}>{k === "fullAccess" ? "■ " : ""}{PERMISSION_LABEL[k]}</option>)}
                </select>
              </label>
              <label className="eff-pop">
                <span>推論の強さ</span>
                <select aria-label="推論の強さ" value={effort} onChange={(e) => pickEffort(e.target.value)}>{efforts.map((x) => <option key={x.id} value={x.id}>{x.id}</option>)}</select>
              </label>
              <label>
                <span>速度 {stCap?.verification === "unverified" && speed ? <UnverifiedTag cap={stCap} /> : null}</span>
                <select aria-label="速度" value={speed} disabled={tiers.length === 0}
                  title={tiers.length === 0 ? "このモデルが示す速度の選択肢はありません（Codex から取得した一覧）" : "次の送信から適用。実際に適用されたかは、受理済みの表示で確認します"}
                  onChange={(e) => pickSpeed(e.target.value)}>
                  <option value="">速度: 指定なし（変更しない）</option>
                  <option value="default" title="標準の速度へ戻す指定を送ります（schemaが標準速度の値として示す default）。結果は受理済みの表示で確認してください">速度: 標準（default）</option>
                  {tiers.filter((t) => t.id !== "default").map((t) => <option key={t.id} value={t.id} title={t.description ?? undefined}>{t.name}</option>)}
                </select>
              </label>
              {speed === "default" ? <div className="small muted">標準へ戻す指定を送ります。戻ったかは、送信後の受理済みの表示で確認してください（未確認）。</div> : null}
              <label>
                <span>計画／実行</span>
                <select aria-label="計画／実行" value={wmSelected ?? ""} disabled={!wmUsable && wmSelected === null}
                  title={wmUsable ? "次の送信から適用。実際に適用されたかは、受理済みの表示で確認します（experimental）" : (wmCap?.note ?? "この Codex 接続では使えません")}
                  onChange={(e) => p.onWorkMode(e.target.value === "" ? null : (e.target.value as WorkMode))}>
                  <option value="">計画／実行: 指定なし</option>
                  {wmOptions.map((m) => <option key={m.mode} value={m.mode} disabled={!wmUsable}>{m.label}</option>)}
                </select>
              </label>
              <div className="small accfull" title={acceptTip}>{acceptLine}</div>
              {wmLine ? <div className="small accfull" title="計画／実行は experimental の設定で、設定の更新通知を受け取るまで受理は確認できません。">{wmLine} <UnverifiedTag cap={wmCap} /></div> : null}
              {speedKeptLine ? <div className="small why">{speedKeptLine}</div> : null}
            </div>
          </Popover>
          <UsageChip local={p.local} settings={p.snap.settings.continuation} />
          {permOff ? <span className={`small acc ${perm === "fullAccess" ? "danger" : ""}`} title={`権限は既定から外れています: ${PERMISSION_FULL[perm]}（次の送信から適用。変更は「…」）`}>権限: {PERMISSION_LABEL[perm]}</span> : null}
          {speedName ? <span className="small muted" title="速度の指定（次の送信から適用）">速度 {speedName}</span> : null}
          {wmSelected ? <span className="small muted" title={`計画／実行の指定: ${WORK_MODE_TEXT[wmSelected]}（次の送信から適用）`}>{wmShort(wmSelected)}</span> : null}
          {/* 受理の根拠は、問題があるときだけ1行目に出す（全文は title と「⋯」） */}
          {!accepted ? <span className="small acc" title={`${acceptLine}\n${acceptTip}`}>受理未確認</span> : changed ? <span className="small acc" title={acceptLine}>未受理</span> : null}
          {wmSelected && wmAccepted !== wmSelected ? <span className="small acc" title={wmLine ?? undefined}>{wmShort(wmSelected)}: 未受理</span> : null}
          {!wmSelected && wmAccepted === "plan" ? <span className="small acc" title={wmLine ?? undefined}>計画／実行 {wmShort(wmAccepted)} のまま</span> : null}
          {speedKept ? <span className="small why" title={speedKeptLine ?? undefined}>速度 {speedKept} のまま</span> : null}
          </div>
          {/* 右端は折り返さない（送信・中断・送信方法）。設定部品は左の群で折り返す */}
          <div className="ctrl-r">
          {running ? <button className="btn-line" aria-label="中断" title="実行中の作業に中断を要求します（停止の確認は別に行います）" onClick={() => p.onAct("interrupt")}><Icon name="stop" /><span className="lbl">中断</span></button> : null}
          <span className="sendgrp">
            <button className={`btn-main send${hu.armed ? " handoff" : ""}`} aria-label={hu.armed ? "引き継いで送信" : running ? "完了後に送る依頼として登録" : "送信"} title={hu.armed ? `新しい thread へ引き継いで送信します（目的メモ＋直近${hu.rounds}往復）。${enterHint}` : running ? `完了後に送信（親と子孫の作業が終わってから送ります）。${enterHint}` : enterHint} disabled={!!lock || !!attachBlock || empty} onClick={() => void send()}><Icon name="send" />{hu.armed ? <span className="lbl">引き継いで送信</span> : null}</button>
            {/* 追加指示は送る前に決めず、送る時点で選ぶ（1回限り）。実行中のみ */}
            {running ? (
              <Popover role="menu" align="right" label="送信方法" title="この送信の送り方（完了後に送信／追加指示で送る）" triggerClassName="btn-main sendmore" trigger={<span aria-hidden="true">▾</span>}>
                {(close) => (
                  <>
                    <button role="menuitem" disabled={!!lock || !!attachBlock || empty} title="次の依頼として。親と子孫の作業が終わってから送ります（既定）" onClick={() => { close(); void send(false); }}><span>完了後に送信</span><span className="tag">既定</span></button>
                    <button role="menuitem" aria-disabled={steerWhy ? true : undefined}
                      title={steerWhy ?? "現在のturnへ割り込んで送ります。実行中のturnへ対象を照合して送ります。モデル・権限・フォルダの変更には使えません"}
                      onClick={() => { if (steerWhy) return; close(); void send(true); }}><span>追加指示で送る</span><span className="kbd">{shortcutById("sendQueue")?.keys}</span></button>
                    {steerWhy ? <div className="small muted" style={{ padding: "2px 8px 4px" }}>追加指示にできない理由: {steerWhy}</div> : null}
                  </>
                )}
              </Popover>
            ) : null}
          </span>
          </div>
        </div>
      </div>
      <PurposeLine local={p.local} onEdit={() => p.onAct("purpose")} />
    </div>
  );
}

/** 送信を止める理由（無ければ null）。中央の入力欄とドックの指示欄で同じ判定を使う。 */
export function sendLockReason(snap: HostSnapshot, chat: Chat, local: CenterProps["local"]): string | null {
  const delPending = local?.deletePending ?? null;
  return local?.org && !local.org.released ? ORG_SEND_LOCK // 組織実行のチャットは閲覧のみ（ホストも同じ判定で拒否する）
    : delPending ? `削除を保留中のため、送信できません。${PENDING_TEXT(delPending.reason)}`
    : chatStops(snap, chat).some(stopOpen) ? "停止を確認できるまで、このチャットへの新しい送信は止めています。"
    : chat.origin === "external" && rootView(snap, chat)?.freshness !== "live" ? "再開するまで送信できません（上の案内から再開）" : null;
}

export function CenterPane(p: CenterProps) {
  const { snap, chat } = p;
  const running = isRunning(snap, chat);
  const reqs = chatRequests(snap, chat);
  const hasOpenStop = chatStops(snap, chat).some(stopOpen);
  // 作業フォルダの帯はチャットごと。開いているチャットのIDを持ち、別のチャットへ切り替えたら自然に閉じる（描画中の書き換えはしない）。
  const [cwdOpenFor, setCwdOpenFor] = useState<string | null>(null);
  const cwdOpen = cwdOpenFor === chat.key.id;
  // 上部メニュー「作業フォルダの確認と変更…」（App が "agentdock:cwd-toggle" を送る）。ヘッダーのボタンと同じ開閉。
  useEffect(() => {
    const h = () => setCwdOpenFor((id) => (id === chat.key.id ? null : chat.key.id));
    window.addEventListener("agentdock:cwd-toggle", h);
    return () => window.removeEventListener("agentdock:cwd-toggle", h);
  }, [chat.key.id]);
  // 会話内検索（Ctrl+F。Appが "agentdock:find" を送る）。チャットを切り替えても開いたまま同じ語で検索し直す。
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [findTick, setFindTick] = useState(0);
  const findOpenRef = useRef(false);
  const beforeFind = useRef<HTMLElement | null>(null);
  findOpenRef.current = findOpen;
  useEffect(() => {
    const h = () => {
      if (!findOpenRef.current) beforeFind.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      setFindOpen(true);
      setFindTick((t) => t + 1);
    };
    window.addEventListener("agentdock:find", h);
    return () => window.removeEventListener("agentdock:find", h);
  }, []);
  const closeFind = () => {
    setFindOpen(false);
    const back = beforeFind.current;
    beforeFind.current = null;
    // 開く前のフォーカスへ戻す。無ければ入力欄へ
    if (back && back !== document.body && back.isConnected) back.focus(); else p.inputRef.current?.focus();
  };
  const find: FindState = { open: findOpen, query: findQuery, setQuery: setFindQuery, tick: findTick, partial: p.turns.some((t) => t.end !== null && !t.complete), close: closeFind };
  const cwdLock = cwdLockReason(chat, running, hasOpenStop);
  const nameOf = (a: AgentKey): string => {
    const v = chatAgents(snap, chat).find((x) => keyStr(x.agent.key) === keyStr(a));
    return v ? showKnown(v.agent.displayName) : a.id;
  };
  const lock = sendLockReason(snap, chat, p.local) ?? firstSendLockReason(p.pending);
  // 引き継ぎの案内（worktree に結び付いたチャットは対象外）。台帳の結び付き（作成元チャット）か、作業フォルダが台帳の worktree 内か
  const inWorktree = snap.worktrees.some((w) => w.createdForChat?.id === chat.key.id) || worktreeOf(knownValue(chat.cwd) ?? null, snap.worktrees) !== null;
  const hu = useHandoff({ chat, local: p.local, settings: snap.settings.continuation, running, worktree: inWorktree, load: p.loadHandoffPreview });
  return (
    <>
      <Header snap={snap} chat={chat} onAct={p.onAct} commands={p.commands} local={p.local} onCwdToggle={() => setCwdOpenFor((id) => (id === chat.key.id ? null : chat.key.id))} wasLive={p.wasLive} externalLabel={p.externalLabel} cwdInfo={p.cwdInfo} leftShown={p.leftShown} rightShown={p.rightShown} />
      {cwdOpen ? <WorkFoldersPanel key={`wf-${chat.key.id}`} api={p.wf} lockReason={cwdLock} permissionText={PERMISSION_FULL[p.local?.permission ?? "workspaceWriteOnRequest"]} fallback={p.local?.nextFolders?.folders ?? [p.local?.nextCwd ?? knownValue(chat.cwd) ?? ""].filter(Boolean)} onClose={() => setCwdOpenFor(null)} /> : null}
      <Banners p={p} />
      <Messages key={chat.key.id} orgBound={!!p.local?.org} headKey={chat.key} threads={p.local?.conversation?.threads ?? NO_THREADS} chatId={chat.key.id} agentName={nameOf} decisions={p.decisions} turns={p.turns} reqs={reqs} gaps={chatGapRequests(snap, chat.key)} running={running} onRespond={p.onRespond} onAct={p.onAct} find={find}
        locals={snap.attachments.filter((a) => keyStr(a.chat) === keyStr(chat.key))} files={p.files} sends={p.sends}
        pending={p.pending} onPendingRestore={p.onPendingRestore} onPendingDismiss={p.onPendingDismiss}
        resend={{ ctx: resendCtxOf(snap, chat, snap.sources.some((x) => x.connection.kind === "connected"), p.turns), rootAgentId: rootView(snap, chat)?.agent.key.id }}
        extra={<FilesBlock chatId={chat.key.id} sent={snap.attachments.filter((a) => keyStr(a.chat) === keyStr(chat.key) && a.usedBy.length > 0)} artifacts={snap.artifacts.filter((a) => keyStr(a.chat) === keyStr(chat.key))} files={p.files} />} />
      <Queue q={snap.queues.find((q) => keyStr(q.chat) === keyStr(chat.key))} nameOf={nameOf} qact={p.qact} steerWhy={steerBlockReason(running, modelChanged(p.settings), p.local)} />
      <GoalBar goal={p.goal} live={rootView(snap, chat)?.freshness === "live"} cap={snap.opCapabilities.find((o) => o.op === "goal")} busy={p.goalBusy}
        onEdit={() => p.onAct("goal")} onClear={p.onGoalClear} onReload={p.onGoalReload} />
      <SendStatusBar input={{
        pending: p.pending.filter((x) => !appearsInTurns(x, p.turns)), running,
        // 実行中のturn（終端がない）に、依頼以外の出力が出ているか
        turnHasOutput: p.turns.some((t) => t.end === null && t.entries.some((e) => e.kind.kind !== "userMessage")),
        queue: queueCounts(snap.queues.find((q) => keyStr(q.chat) === keyStr(chat.key))?.entries),
      }} />
      <IntakeSection chat={chat.key} local={p.local} live />
      <Composer key={`${chat.key.id}:${p.models.length}`} p={p} running={running} lock={lock} hu={hu} />
    </>
  );
}

export function EmptyCenter({ onAct }: { onAct: (a: string) => void }) {
  return (
    <div className="msgs" style={{ display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div style={{ maxWidth: 420 }}>
        <h2 style={{ fontSize: "calc(var(--fs) + 4px)", margin: "0 0 8px" }}>最初のチャットを始めましょう</h2>
        <p className="muted" style={{ margin: "0 0 12px" }}>作業フォルダを選ばずに、相談や文章作成から始められます。コードを扱うときはフォルダを指定します。</p>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button className="btn-main" onClick={() => onAct("newChatQuick")}><Icon name="chat" />一般チャットをすぐ始める</button>
          <button className="btn-line" onClick={() => onAct("newChat")}><Icon name="folder" />フォルダを指定して始める</button>
        </div>
        <p className="small muted" style={{ marginTop: 14 }}>CLI や VS Code で作った会話は、左の一覧から開けます（外部で実行中なら閲覧のみ）。</p>
      </div>
    </div>
  );
}

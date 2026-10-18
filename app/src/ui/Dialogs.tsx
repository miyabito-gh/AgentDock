import { isNewChatSubmitKey } from "./quickNew";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { OrgRunningNote, StartOrgArgs } from "../ipc/types";
import type { Chat, ChatKey, ChatLocalView, ChatPendingOp, ChatQueue, ChatModelSettings, ForceKillPreview, Goal, GoalUpdate, Known, ModelInfo, NotificationSettings, OrgDeleteOutcome, OrgDeletePreview, OpCapability, QuitDecision, QuitPhase, RevertResult, SaveStatus, SideSessionMeta, SourceInfo, StopRecord, StopSummary, TurnRecord, WorktreeRecord, CloudTaskRecord } from "../ipc/types";
import type { AppearanceSettings, BodyTextSize, ChatListView, StrongReason, ThemePref, UiTextSize, Support } from "../ipc/types";
import { chatName } from "./derive";
import { SCOPE_TEXT } from "./Chrome";
import { Icon } from "./Icon";
import { DeleteBody, ExportBody, StorageBody, type BaselineProps, type DeleteProps, type ExportProps, type UsageProps } from "./ManageDialogs";
import { FootCtx, useFootSlots, About, ConfirmBlock, FootActions } from "./DialogParts";
import * as host from "../ipc/client";
import { copyText } from "./markdown/parts";
import { hostOf, isIdnHost, textMismatchesUrl } from "./markdown/plugins";
import { ParityBody } from "./ParityDialog";
import { AboutBody, HowtoBody, ShortcutsBody } from "./HelpDialogs";
import { ChangesPanel } from "./ChangesDialog";
import { GoalBody, StatusBody } from "./PrefsDialogs";
import { UsagePanel } from "./UsagePanel";
import { CompactBody, ForkBody, ReviewBody } from "./ThreadOpsDialogs";
import { ResendBody, type ResendHandlers, type ResendState } from "./ResendDialog";
import { ExtensionsBody } from "./ExtensionsDialogs";
import { InstructionBody, ReferenceBody, SkillsBody, type ComposeProps } from "./ComposeDialogs";
import { WorkspaceChoice, WorktreesBody } from "./WorktreeDialogs";
import { CloudBody } from "./CloudDialogs";
import { ContinuationSettingsBody, HandoffPreviewBody, PurposeBody, type ContinuityDialogProps } from "./ContinuityDialogs";
import { OrgDeleteDialog, OrgSettingsBody, OrgStartBody, type LoadGraphs, type OrgSettingsProps } from "./OrgDialogs";
import { FolderListEditor } from "./WorkFoldersUi";
import { FolderSetBar, type FolderTools } from "./WorkFolderSetsUi";
import { inspectFolders, splitForNewChat } from "./workFolderSets";
import { addBlockReason, applyCheck, confirmedStrongPaths, MAX_FOLDERS, projectDefaults, REJECT_TEXT, saveBlockReason, strongText, type FolderItem } from "./workFolders";

export type DialogState =
  | { type: "settings"; tab: string }
  | { type: "newChat"; dev?: boolean; cwd?: string; cwdChoices?: string[]; /** 「同じ作業フォルダで新規」: 先頭＝プライマリ。開いたとき検査して並べる。 */ initialFolders?: string[]; folderNote?: string; /** 作成に失敗したときの「入力し直す」: 同じ値を入れて開くだけ（送らない）。 */ prefill?: { model: string; first: string; isolate: boolean; worktree: string | null } }
  | { type: "worktrees" }
  | { type: "cloud"; chatId: string | null }
  | { type: "orgStart" }
  | { type: "orgDelete"; workflowId: string }
  | { type: "quit" }
  | { type: "force" }
  | { type: "delete"; chatId: string }
  | { type: "export"; chatId: string }
  | { type: "unv"; why: string }
  | { type: "parity" }
  | { type: "shortcuts" }
  | { type: "howto" }
  | { type: "about" }
  | { type: "changes"; chatId: string }
  | { type: "revert"; chatId: string }
  | { type: "review"; chatId: string }
  | { type: "fork"; chatId: string; throughTurn: string | null }
  | { type: "compact"; chatId: string }
  | { type: "resend"; chatId: string; purpose: "editResend" | "regenerate"; turnId: string; itemId: string | null }
  | { type: "goal"; chatId: string }
  | { type: "purpose"; chatId: string }
  | { type: "handoffPreview"; chatId: string }
  | { type: "status"; chatId: string | null }
  | { type: "usage" }
  | { type: "attach" }
  | { type: "reference"; chatId: string }
  | { type: "skills"; chatId: string }
  | { type: "instructions"; chatId: string }
  | { type: "resumeExternal"; chatId: string }
  | { type: "rename"; chatId: string }
  | { type: "openLink"; url: string; text: string };

/** 外枠。フッターは左＝副操作、右＝閉じる（キャンセル）＋主操作（いちばん右）。
 *  `close` を渡すと、閉じるボタン付きのフッターを作り、本文側が FootActions で副操作・主操作を差し込める。
 *  `foot` は主操作・危険操作まで呼び出し側が組むとき（閉じる／キャンセルも含めて渡す）。`footLeft` は左の副操作。 */
function Shell({ title, children, foot, footLeft, close, wide, full, onClose, locked }: { title: string; children: ReactNode; foot?: ReactNode; footLeft?: ReactNode; close?: string; wide?: boolean; full?: boolean; onClose: () => void; locked?: boolean }) {
  const fs = useFootSlots();
  // locked: 実行中の操作があり、結果を受け取るまで閉じさせない（閉じて開き直しての二重実行を防ぐ）。
  // Esc＝閉じる／キャンセルと同じ（IME変換中・locked中は閉じない）。Popover等は捕捉段階で止めるので、最前面の1つだけが閉じる。
  const esc = useRef({ onClose, locked });
  esc.current = { onClose, locked };
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing || e.defaultPrevented || esc.current.locked) return;
      e.preventDefault();
      esc.current.onClose();
    };
    document.addEventListener("keydown", h);
    return () => document.removeEventListener("keydown", h);
  }, []);
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget && !locked) onClose(); }}>
      <div className={`dialog ${wide ? "wide" : ""} ${full ? "full-narrow" : ""}`} role="dialog" aria-modal="true" aria-label={title}>
        <header><h3>{title}</h3><button className="btn subtle icon" aria-label="閉じる" disabled={locked} onClick={onClose}><Icon name="x" /></button></header>
        <FootCtx.Provider value={fs.slots}>{children}</FootCtx.Provider>
        {close ? (
          <footer>
            <div className="foot-l" ref={fs.setLeft} />
            <div className="foot-r"><button className="btn" disabled={locked} onClick={onClose}>{close}</button><span className="foot-main" ref={fs.setMain} /></div>
          </footer>
        ) : foot ? <footer><div className="foot-l">{footLeft}</div><div className="foot-r">{foot}</div></footer> : null}
      </div>
    </div>
  );
}

const TABS: Array<[string, string]> = [["general", "全般"], ["display", "表示"], ["notify", "通知"], ["input", "入力"], ["model", "モデル"], ["codex", "Codex"], ["mcp", "MCP・Plugins"], ["skills", "Skills"], ["org", "組織実行"], ["continuation", "継続チャット"], ["storage", "保存と容量"]];

export interface CodexExe { path: string; setPath: (p: string) => void; placeholder: string; connect: (p: string) => void; openDiag: () => void; browse: () => void; live: boolean; sandboxWorkaround: { value: boolean; set: (on: boolean) => void } }

/** 削除・エクスポート・使用量（P7）。 */
export interface ManageProps { del: DeleteProps; exp: ExportProps; usage: UsageProps; baselines: BaselineProps; selectedId: string | null; ackedWarnings: { list: string[]; reset: () => void } }

/** Goal・Codex の状態の表示に使う、選択中チャットの値（ホストが持つ値のコピー）。 */
export interface PrefsProps {
  goal: Known<Goal> | undefined;
  /** 目標を要求する。戻り値は表示する文（結果は断定しない）。 */
  saveGoal: (chatId: string, u: GoalUpdate) => Promise<string>;
  settings: ChatModelSettings | undefined;
  local: ChatLocalView | undefined;
}

/** 参考指定・Skill・指示ファイルに使う、選択中のチャットと入力欄への操作（ホスト側の値のコピーと操作の入口）。 */
export interface ComposeDialogProps extends ComposeProps {
  /** 選択中のチャットのside相談（新しい順）と、開く・記録を見る操作。 */
  sides: SideSessionMeta[];
  onOpenSide: () => void;
  onShowSide: (id: string) => void;
  /** 指示ファイル・作成した雛形を関連アプリで開く（ユーザー操作）。 */
  openPath: (path: string) => void;
}

export interface NotifyProps { value: NotificationSettings; set: (n: NotificationSettings) => void }

/** 組織実行（P-ORG-7b）。設定タブ（停滞時間・役割定義）と、開始ダイアログの操作。 */
export interface OrgDialogProps { settings: OrgSettingsProps; start: (a: StartOrgArgs) => Promise<string | null>; browse: () => Promise<string | null>; loadGraphs: LoadGraphs;
  previewDelete: (id: string) => Promise<OrgDeletePreview>; deleteWorkflow: (id: string, deleteChats: boolean, expect: ChatKey[]) => Promise<OrgDeleteOutcome>; onDeleted: (id: string) => void }

export interface AutostartProps { value: boolean; set: (on: boolean) => void }
export interface AppearanceProps { value: AppearanceSettings; view: ChatListView; set: (a: AppearanceSettings) => void; setView: (v: ChatListView) => void }

/** 完全終了の進行（ホストの `quitUpdated`）と、画面から出す選択。 */
export interface QuitProps {
  phase: QuitPhase; stops: StopRecord[]; failedSaves: SaveStatus[]; live: boolean;
  decide: (d: QuitDecision) => void; onForce: () => void; onRetrySave: () => void;
}

/** 強制終了の確認（`preview_force_kill` の結果）。preview が null の間は取得中（error があれば取得失敗）。 */
export interface ForceProps { preview: ForceKillPreview | null; error: string | null; running: boolean; run: () => void }

const THEME_CHOICES: Array<[ThemePref, string]> = [["System", "OSに従う"], ["Light", "ライト"], ["Dark", "ダーク"]];
const UI_SIZE_CHOICES: Array<[UiTextSize, string]> = [["Small", "小"], ["Normal", "標準"], ["Large", "大"]];
const BODY_SIZE_CHOICES: Array<[BodyTextSize, string]> = [["Small", "小"], ["Normal", "標準"], ["Large", "大"], ["XLarge", "特大"]];
const LIST_VIEW_CHOICES: Array<[ChatListView, string]> = [["Recent", "最近使った順"], ["ByFolder", "作業フォルダごと"]];

function SettingsBody({ tab, enterMode, setEnterMode, top, source, models, exe, notify, autostart, appearance, manage, chats, compose, opCaps, org, cont }: {
  org: OrgSettingsProps; cont: ContinuityDialogProps;
  manage: ManageProps; chats: Chat[]; compose: ComposeDialogProps; opCaps: OpCapability[];
  autostart: AutostartProps;
  appearance: AppearanceProps;
  notify: NotifyProps;
  tab: string; enterMode: "ctrl" | "enter"; setEnterMode: (m: "ctrl" | "enter") => void; top: { main: boolean; mini: boolean; setMain: (b: boolean) => void; setMini: (b: boolean) => void };
  source: SourceInfo | undefined; models: ModelInfo[]; exe: CodexExe;
}) {
  switch (tab) {
    case "general": return (
      <>
        <div className="field"><span>ログイン時に起動</span><label><input type="checkbox" checked={autostart.value} onChange={(e) => autostart.set(e.target.checked)} />Windows にサインインしたら起動する</label><span className="note">オンにすると、通常画面は開かずトレイに入った状態で起動します。起動しただけで過去の依頼を再実行しません。</span></div>
        <div className="field"><span>最前面</span><div><label><input type="checkbox" checked={top.main} onChange={(e) => top.setMain(e.target.checked)} />通常画面</label>　<label><input type="checkbox" checked={top.mini} onChange={(e) => top.setMini(e.target.checked)} />監視窓</label></div><span className="note">最前面にしても入力フォーカスは奪いません。</span></div>
        <div className="field"><span>閉じるボタン</span><span>トレイに格納して作業を続ける（終了はメニューの「AgentDock を終了」）</span></div>
      </>);
    case "display": return (
      <>
        <div className="field"><span>テーマ</span><div role="radiogroup" aria-label="テーマ">
          {THEME_CHOICES.map(([v, t]) => <label key={v}><input type="radio" name="ap-theme" checked={appearance.value.theme === v} onChange={() => appearance.set({ ...appearance.value, theme: v })} />{t}</label>)}</div></div>
        <div className="field"><span>画面の文字</span><div role="radiogroup" aria-label="画面の文字の大きさ">
          {UI_SIZE_CHOICES.map(([v, t]) => <label key={v}><input type="radio" name="ap-ui" checked={appearance.value.uiText === v} onChange={() => appearance.set({ ...appearance.value, uiText: v })} />{t}</label>)}</div>
          <span className="note">一覧・ヘッダー・ドック・ダイアログなど、会話本文以外の文字の大きさです。</span></div>
        <div className="field"><span>会話本文の文字</span><div role="radiogroup" aria-label="会話本文の文字の大きさ">
          {BODY_SIZE_CHOICES.map(([v, t]) => <label key={v}><input type="radio" name="ap-body" checked={appearance.value.bodyText === v} onChange={() => appearance.set({ ...appearance.value, bodyText: v })} />{t}</label>)}</div></div>
        <div className="field"><span>一覧の並び</span><div role="radiogroup" aria-label="チャット一覧の並び">
          {LIST_VIEW_CHOICES.map(([v, t]) => <label key={v}><input type="radio" name="ap-view" checked={appearance.view === v} onChange={() => appearance.setView(v)} />{t}</label>)}</div></div>
        <div className="note">変更はすぐ反映され、保存されます。保存できなかったときは、次に起動したときに保存されている値に戻ります。</div>
      </>);
    case "notify": return (
      <>
        <div className="field"><span>通知</span><label><input type="checkbox" checked={notify.value.enabled} onChange={(e) => notify.set({ ...notify.value, enabled: e.target.checked })} />通知を使う</label></div>
        <div className="field"><span>種類</span><div>
          <label><input type="checkbox" checked={notify.value.approvalAndQuestion} onChange={(e) => notify.set({ ...notify.value, approvalAndQuestion: e.target.checked })} />承認・質問待ち</label>
          <label><input type="checkbox" checked={notify.value.completed} onChange={(e) => notify.set({ ...notify.value, completed: e.target.checked })} />完了</label>
          <label><input type="checkbox" checked={notify.value.failed} onChange={(e) => notify.set({ ...notify.value, failed: e.target.checked })} />失敗</label>
          <label><input type="checkbox" checked={notify.value.orgAttention} onChange={(e) => notify.set({ ...notify.value, orgAttention: e.target.checked })} />組織実行: 計画の承認待ち・止まったタスク</label>
          <label><input type="checkbox" checked={notify.value.orgFinished} onChange={(e) => notify.set({ ...notify.value, orgFinished: e.target.checked })} />組織実行: 完了・失敗</label></div>
          <span className="note">子・孫の完了と失敗は、チャットごとに2秒間まとめて通知します。承認と質問は待たずに通知します。中断は「失敗」の設定に従います。組織実行の「計画の承認待ち・止まったタスク」は即時、「完了・失敗」はワークフロー単位で2秒間まとめて通知します。</span></div>
        <div className="field"><span>音</span><label><input type="checkbox" checked={notify.value.sound} onChange={(e) => notify.set({ ...notify.value, sound: e.target.checked })} />Windows の標準の音を鳴らす</label></div>
        <div className="field"><span>チャット名</span><label><input type="checkbox" checked={!notify.value.showChatName} onChange={(e) => notify.set({ ...notify.value, showChatName: !e.target.checked })} />通知にチャット名を出さず、状態だけ表示する</label></div>
        <div className="field"><span>抑制</span><span className="small">通常画面を操作している間だけ、選択中のチャットの通知を止めます。別のチャット、画面が背後にあるとき、監視窓だけのときは通知します。通知を開くと該当のチャットを表示するだけで、回答や再実行はしません。</span></div>
      </>);
    case "org": return <OrgSettingsBody {...org} />;
    case "continuation": return <ContinuationSettingsBody value={cont.value} set={cont.setValue} />;
    case "input": return (
      <>
        <div className="field"><span>送信キー</span><div>
          <label><input type="radio" name="ek" checked={enterMode === "ctrl"} onChange={() => setEnterMode("ctrl")} />Ctrl＋Enter で送信／Enter で改行</label><br />
          <label><input type="radio" name="ek" checked={enterMode === "enter"} onChange={() => setEnterMode("enter")} />Enter で送信／Shift＋Enter で改行（新規チャットの最初の依頼では Ctrl＋Enter でも送信）</label></div></div>
        <div className="field"><span>実行中の入力</span><span>初期値は「完了後に送信」</span></div>
      </>);
    case "model": return (
      <div className="field"><span>新しいチャットの既定</span>
        <div style={{ display: "flex", gap: 6 }}><select>{models.map((m) => <option key={m.id}>{m.displayName}</option>)}</select></div>
        <span className="note">既定値を変えても、既存のチャットの設定は変わりません。選択肢は Codex から取得します。</span></div>);
    case "codex": {
      const v = source?.version;
      return (
        <>
          <div className="field"><span>codex.exe の場所</span><div style={{ display: "flex", gap: 6 }}><input type="text" className="mono" style={{ flex: 1 }} value={exe.path} placeholder={exe.placeholder} onChange={(e) => exe.setPath(e.target.value)} aria-label="codex.exe のパス" /><button className="btn-line" disabled={!exe.live} onClick={exe.browse}>参照…</button><button className="btn-line" disabled={!exe.live} onClick={() => exe.connect(exe.path)}>この場所で接続</button></div><span className="note">空欄なら既定の場所を使います。接続済みの間は変更できません。~/.codex の設定と認証は共有し、このアプリからは変更しません。</span></div>
          <div className="field"><span>検出結果</span><span>{v?.kind === "match" ? `codex-cli ${v.version}（対象版）` : v?.kind === "mismatch" ? `${v.actual}（対象版 ${v.expected} と不一致）` : v?.kind === "unknown" ? `確認できません（${v.message}）` : "未確認"}</span></div>
          <div className="field"><span>接続</span><span>App Server（stdio）</span></div>
          <div className="field"><span>sandbox の競合</span><label><input type="checkbox" checked={exe.sandboxWorkaround.value} onChange={(e) => exe.sandboxWorkaround.set(e.target.checked)} />sandbox の競合を回避する（プラグインを無効にして Codex を起動）</label><span className="note">組織実行の開始前チェックが失敗したときだけ使います。オンの間は computer-use などのプラグインが全チャットで使えません。AgentDock の再起動後に有効になります。{source && source.sandboxWorkaround !== exe.sandboxWorkaround.value ? "（現在の接続は、この設定と異なる状態で起動しています）" : ""}</span></div>
          <div className="field"><span>診断ログ</span><div><button className="btn-line" onClick={exe.openDiag}>診断ログの場所を開く</button></div><span className="note">%APPDATA%\com.agentdock.app\diag\diag.log。未対応の通知・項目の形（キー構造のみ。本文・認証情報は記録しません）と、子孫の走査結果などを記録します。</span></div>
        </>);
    }
    case "mcp": return <ExtensionsBody live={compose.live} toolCap={opCaps.find((x) => x.op === "toolServers")} extCap={opCaps.find((x) => x.op === "extensions")} />;
    case "skills": {
      // 選んでいるチャットの作業フォルダのSkillと、読み込まれた指示ファイル（確認だけ。有効・無効の切替はしない）。
      const c = compose.chat;
      return (
        <>
          <p className="small muted">{c ? `対象のチャット: ${chatName(c)}` : "チャットを選ぶと、その作業フォルダのSkillと指示ファイルを表示します。"}</p>
          <SkillsBody chat={c} live={compose.live} cap={opCaps.find((x) => x.op === "skills")} recent={compose.recentSkills} onList={compose.noteSkills} />
          <hr />
          <InstructionBody chat={c} live={compose.live} cap={opCaps.find((x) => x.op === "instructionFiles")} insertText={compose.insertText} openFile={compose.openPath} />
        </>);
    }
    default: return <StorageBody u={manage.usage} chats={chats} selected={manage.selectedId} acked={manage.ackedWarnings} bl={manage.baselines} />;
  }
}

const STOP_TEXT: Record<StopSummary, string> = {
  notRequested: "中断要求を送れていません（停止を確認できません）",
  interruptRequested: "中断要求済み・turn終端を未確認",
  turnEndConfirmed: "停止を確認しました（turn終端）",
  managedExecEndConfirmed: "停止を確認しました（管理実行の終了）",
  osGoneConfirmed: "停止を確認しました（OSプロセスの消滅）",
  unconfirmed: "停止未確認（中断要求から10秒以上）",
  ownershipUnknown: "所有を確認できないため、強制終了の対象にしません",
};
const CONFIRMED_STOP: StopSummary[] = ["turnEndConfirmed", "managedExecEndConfirmed", "osGoneConfirmed"];

const nameOf = (chats: Chat[], k: ChatKey): string => {
  const c = chats.find((x) => x.key.id === k.id);
  return c ? chatName(c) : "（名前を確認できないチャット）";
};

function targetText(t: StopRecord["targets"][number]): string {
  const r = t.target;
  return r.kind === "turn" ? "エージェント" : r.kind === "managedExec" ? `管理実行 ${r.execId}` : `OSプロセス ${r.pid}`;
}

/** 完全終了の確認・進行。閉じる操作（トレイ格納）ではここを出さない。停止は証拠で確認し、時間経過だけでは確認にしない。 */
/** リンクの確認（DESIGN_P5 §3.3）。http・https だけがここへ来る。行き先の全文を見せ、押したときだけ既定のブラウザで開く。 */
function OpenLinkBody({ url, text, onClose }: { url: string; text: string; onClose: () => void }) {
  const [msg, setMsg] = useState<string | null>(null);
  const hn = hostOf(url);
  const at = hn ? url.indexOf(hn) : -1;
  const open = async () => {
    try { await host.openAuthorizationUrl(url); onClose(); } catch (e) { setMsg(`ブラウザで開けませんでした: ${host.asIpcError(e).message}`); }
  };
  const copy = async () => setMsg((await copyText(url)) ? "URLをコピーしました。" : "コピーできませんでした。");
  return (
    <div className="content">
      <p>外部のページをブラウザで開きます。</p>
      {textMismatchesUrl(text, url) ? <div className="banner info" role="alert"><div>表示の文字と実際の行き先が違います。</div></div> : null}
      {isIdnHost(hn) ? <div className="banner info" role="alert"><div>国際化ドメイン名です。見た目が似た別のサイトの可能性があります。</div></div> : null}
      <p className="small">行き先のホスト: <b className="mono" style={{ userSelect: "text" }}>{hn}</b></p>
      <p className="mono" style={{ overflowWrap: "anywhere", userSelect: "text" }}>
        {at >= 0 ? <>{url.slice(0, at)}<b>{hn}</b>{url.slice(at + hn.length)}</> : url}
      </p>
      {msg ? <p className="small" role="status">{msg}</p> : null}
      <FootActions left={<button className="btn" onClick={() => void copy()}>URLをコピー</button>} main={<button className="btn primary" autoFocus onClick={() => void open()}>ブラウザで開く</button>} />
    </div>
  );
}

/** 完全終了・強制終了の確認に併記する、組織実行中のワークフロー（併記だけ。終了の手順や対象は変えない）。 */
function OrgRunningSection({ notes }: { notes: OrgRunningNote[] }) {
  if (notes.length === 0) return null;
  return (
    <div className="org-running" role="group" aria-label="組織実行中">
      <p><b>組織実行中</b></p>
      <ul className="quit-list">
        {notes.map((n) => <li key={n.workflowId}>{n.title ?? "（依頼の名前なし）"}<span className="sub">終わっていないタスク {n.activeTasks} 件（全 {n.totalTasks} 件）</span></li>)}
      </ul>
      <p className="small muted">終了しても、再起動後に自動では再開しません。状態を照合したあと、「再開」を押したときだけ再開します。</p>
    </div>
  );
}

function QuitDialog({ q, chats, onClose, onAct }: { q: QuitProps; chats: Chat[]; onClose: () => void; onAct: (a: string) => void }) {
  const p = q.phase;
  const cancel = <button className="btn" onClick={() => q.decide("cancel")}>終了を取り消す</button>;
  if (!q.live) return (
    <Shell title="AgentDock を終了" onClose={onClose} foot={<><button className="btn" onClick={onClose}>終了を取り消す</button><button className="btn danger" onClick={() => onAct("quitStop")}>作業を中断して終了</button></>}>
      <div className="content"><p>作業中のチャットがある場合、各対象の停止と保存を確認してから終了します。この操作はモックでは動きません。</p></div>
    </Shell>);
  switch (p.kind) {
    case "idle": return (
      <Shell title="AgentDock を終了" onClose={onClose}><div className="content"><p>作業の状況を確認しています…</p></div></Shell>);
    case "confirming": return (
      <Shell title="AgentDock を終了" onClose={onClose} foot={<>{cancel}<button className="btn danger" onClick={() => q.decide("stopAndQuit")}>作業を中断して終了</button></>}>
        <div className="content">
          <p>作業中のチャットがあります。</p>
          <ul className="quit-list">{p.busy.map((k) => <li key={k.id}>{nameOf(chats, k)}</li>)}</ul>
          <OrgRunningSection notes={p.orgRunning} />
          <p className="small">停止を確認できない対象があれば、その内容を表示して終了を止めます。</p>
          <About><p>「作業を中断して終了」では、各チャットへ中断要求を送り、停止と保存を確認してから終了します。</p></About>
        </div>
      </Shell>);
    case "stopping": return (
      <Shell title="停止と保存を確認しています" onClose={onClose} foot={cancel}>
        <div className="content">
          <p>中断を要求しました。停止と保存を確認してから終了します。</p>
          <p className="small">約10秒で停止を確認できない対象があれば、ここに表示します。取り消しても、すでに送った中断要求は取り消せません。</p>
        </div>
      </Shell>);
    case "stopUnconfirmed": {
      const recs = q.stops.filter((r) => p.records.includes(r.id));
      return (
        <Shell title="停止を確認できていない対象があります" wide onClose={onClose}
          footLeft={<><button className="btn" onClick={() => q.decide("wait")}>待つ</button><button className="btn" onClick={() => q.decide("retryInterrupt")}>中断を再試行</button></>}
          foot={<>{cancel}<button className="btn danger-line" onClick={q.onForce}>強制終了…</button></>}>
          <div className="content">
            <p>中断要求から10秒たっても、停止を確認できていない対象があります。時間の経過だけで停止や失敗とは判断していません。</p>
            <ul className="quit-list">
              {recs.map((r) => (
                <li key={r.id}><b>{nameOf(chats, r.chat)}</b>
                  {r.targets.filter((t) => !CONFIRMED_STOP.includes(t.summary) || t.ownership === "unknown").map((t, i) => (
                    <span className="sub" key={i} title={t.target.kind === "turn" ? `ID: ${t.target.turn.agent.id}` : undefined}>{targetText(t)}: {STOP_TEXT[t.summary]}</span>
                  ))}
                </li>
              ))}
            </ul>
            <p className="small">「待つ」では終了せずに確認を続けます。確認できた時点で保存して終了します。強制終了は、同じ Codex 上の全チャットが止まります（確認画面で対象を示します）。</p>
          </div>
        </Shell>);
    }
    case "flushing": return (
      <Shell title="保存を確認しています" onClose={onClose}><div className="content"><p>保存を確認してから終了します。</p></div></Shell>);
    case "saveFailed": return (
      <Shell title="保存できていない内容があります" wide onClose={onClose} foot={<>{cancel}<button className="btn primary" onClick={q.onRetrySave}>保存を再試行</button></>}>
        <div className="content">
          <p>保存に成功したことを確認できないため、終了していません。</p>
          <ul className="quit-list">
            {q.failedSaves.map((s, i) => (
              <li key={i}>{SCOPE_TEXT[s.scope.kind]}{s.state.kind === "saveFailed" ? <span className="sub">{s.state.message}</span> : null}</li>
            ))}
          </ul>
        </div>
      </Shell>);
  }
}

/** 強制終了の確認。App Server（Job Object）単位なので、同じ監視元の全チャットを示す。実行は明示確認の後だけ。 */
function ForceDialog({ f, chats, onClose }: { f: ForceProps; chats: Chat[]; onClose: () => void }) {
  const pv = f.preview;
  return (
    <Shell title="強制終了" onClose={onClose}>
      <div className="content">
        {f.error ? <p style={{ color: "var(--fail)" }}>{f.error}</p> : !pv ? <p>影響を受ける対象を確認しています…</p> : (
          <>
            <p>この Codex（App Server）を、子のプロセスごと終了します。同じ Codex 上の次のチャットがすべて止まります。</p>
            {pv.affected.length
              ? <ul className="check-list">{pv.affected.map((k) => <li key={k.id}>{nameOf(chats, k)}</li>)}</ul>
              : <p className="small muted">作業中・停止未確認のチャットは確認できません（Codex 自体は終了します）。</p>}
            <p className="small muted">稼働中のプロセス数: {pv.activeProcesses.kind === "value" ? pv.activeProcesses.value : "未取得"}</p>
            <OrgRunningSection notes={pv.orgRunning} />
          </>
        )}
        <ConfirmBlock label="強制終了の確認" actions={<>
          <button className="btn" onClick={onClose}>やめる</button>
          <button className="btn danger" disabled={!pv || f.running} onClick={f.run}>{f.running ? "要求しています…" : "強制終了する"}</button></>}>
          <p>保存されていない内容が失われたり、処理の一部が残ったりする可能性があります。このアプリが起動していない実行は対象にしません。強制終了の後も、停止を確認できるまでは「停止未確認」として扱います。</p>
        </ConfirmBlock>
      </div>
    </Shell>);
}

/** 名前の変更。Codex に反映できたと確認できたときだけ閉じる（失敗は理由を残す）。 */
function RenameDialog({ chat, onClose, run }: { chat: Chat | undefined; onClose: () => void; run: (name: string) => Promise<string | null> }) {
  const [name, setName] = useState(chat ? chatName(chat) : "");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const go = async () => {
    setBusy(true); setErr(null);
    const e = await run(name.trim());
    setBusy(false);
    if (e) setErr(e);
  };
  return (
    <Shell title="名前を変更" onClose={onClose} foot={<><button className="btn" onClick={onClose}>キャンセル</button><button className="btn primary" disabled={busy || !name.trim()} onClick={() => void go()}>{busy ? "変更しています…" : "変更"}</button></>}>
      <div className="content">
        <input type="text" style={{ width: "100%" }} value={name} onChange={(e) => setName(e.target.value)} aria-label="チャットの名前" autoFocus />
        {err ? <div className="why err" style={{ marginTop: 4 }}>{err}</div> : null}
        <p className="small muted">Codex 側の名前を変更します。変更を確認できるまで、表示は変わりません。</p>
      </div>
    </Shell>
  );
}

/** isolate は「分離」（worktreeを作成してから開始）。worktree は既存のAgentDock作成worktreeの記録ID（チャットに結び付ける）。 */
export interface NewChatInput { cwd: string | null; model: string; firstMessage: string | null; isolate: boolean; worktree: string | null; /** 追加の作業フォルダ（プライマリ以外）。 */ extraFolders: string[]; /** 強い確認を済ませたパス（ホストが再検査する）。 */ confirmedStrong: string[] }

export function Dialogs({ cont, org, folderTools, workspaceRoots, d, onClose, chats, source, models, enterMode, setEnterMode, top, setTab, onAct, exe, onCreateChat, notify, autostart, appearance, quit, force, manage, onRename, opCaps, live, changesTick, turnRecords, onRevertDone, prefs, threadOps, worktree, cloudTasks, compose, resend }: {
  org: OrgDialogProps;
  cont: ContinuityDialogProps;
  compose: ComposeDialogProps;
  /** 作業フォルダのセットの保存・「参照…」。 */
  folderTools: FolderTools;
  /** 複数の作業フォルダの指定の能力（新しいチャットの「フォルダを追加」の可否）。 */
  workspaceRoots: Support;
  /** 編集して再送・再生成（M50）。state は種類「resend」のときだけ App が組み立てる。 */
  resend: { state: ResendState | null; handlers: ResendHandlers };
  opCaps: OpCapability[];
  /** worktreeの台帳と、選択中のチャットの作業フォルダ（リポジトリの一覧用）。 */
  worktree: { records: WorktreeRecord[]; cwd: string | null };
  /** クラウド委任の記録（`cloud-tasks.json`）。 */
  cloudTasks: CloudTaskRecord[];
  /** レビュー・分岐・圧縮に使う、結果が未確認の操作・送信待ち・別の会話を開く操作。 */
  threadOps: { pendingOps: ChatPendingOp[]; queues: ChatQueue[]; onOpenChat: (chat: ChatKey) => void };
  prefs: PrefsProps;
  /** 実接続か（差分・戻すはモックでは動かない）。 */
  live: boolean;
  /** 変更の報告が更新されるたびに増える。 */
  changesTick: number;
  /** 変更を確認の依頼名（発言の先頭）に使う、開いているチャットの履歴。 */
  turnRecords: TurnRecord[];
  onRevertDone: (r: RevertResult | null) => void;
  onRename: (chatId: string, name: string) => Promise<string | null>;
  autostart: AutostartProps; appearance: AppearanceProps; quit: QuitProps; force: ForceProps; manage: ManageProps;
  notify: NotifyProps;
  d: DialogState; onClose: () => void; chats: Chat[]; source: SourceInfo | undefined; models: ModelInfo[];
  exe: CodexExe; onCreateChat: (i: NewChatInput) => Promise<void>;
  enterMode: "ctrl" | "enter"; setEnterMode: (m: "ctrl" | "enter") => void;
  top: { main: boolean; mini: boolean; setMain: (b: boolean) => void; setMini: (b: boolean) => void };
  setTab: (t: string) => void; onAct: (a: string) => void;
}) {
  const [kind, setKind] = useState(d.type === "newChat" && (d.dev || d.cwd) ? "dev" : "general");
  const [cwd, setCwd] = useState(d.type === "newChat" ? d.cwd ?? "" : "");
  const pre = d.type === "newChat" ? d.prefill : undefined;
  const [isolate, setIsolate] = useState(pre?.isolate ?? false);
  const [wt, setWt] = useState<string | null>(pre?.worktree ?? null);
  const [model, setModel] = useState(pre && models.some((m) => m.id === pre.model) ? pre.model : models.find((m) => m.isDefault)?.id ?? models[0]?.id ?? "");
  const [first, setFirst] = useState(pre?.first ?? "");
  // 追加の作業フォルダ（新しいチャット）。プロジェクトから作るときは、プロジェクトの全フォルダを既定で並べる（外せる。上限8件）。
  const [extras, setExtras] = useState<FolderItem[]>([]);
  const [ncNote, setNcNote] = useState<string | null>(null);
  const [ncErr, setNcErr] = useState<string | null>(null);
  const [primaryStrong, setPrimaryStrong] = useState<{ path: string; reasons: StrongReason[]; confirmed: boolean } | null>(null);
  const [creating, setCreating] = useState(false);
  const projectChoices = d.type === "newChat" ? d.cwdChoices : undefined;
  useEffect(() => {
    if (!projectChoices || projectChoices.length < 2 || !live) return;
    const def = projectDefaults(projectChoices);
    const block = addBlockReason(workspaceRoots);
    if (block) { setNcNote(`プロジェクトの他のフォルダは並べていません: ${block}`); return; }
    let alive = true;
    void (async () => {
      let list: FolderItem[] = [];
      const skipped: string[] = [];
      for (const p of def.extras) {
        try {
          const r = applyCheck(list, await host.checkWorkFolder(p, [def.primary, ...list.map((i) => i.path)]), 1);
          if (r.error) skipped.push(`${p}（${r.error}）`); else list = r.items;
        } catch { skipped.push(`${p}（検査できませんでした）`); }
      }
      if (!alive) return;
      setExtras(list);
      const notes = [def.omitted ? `上限${MAX_FOLDERS}件のため、残り${def.omitted}件は並べていません。` : "", skipped.length ? `並べられなかったフォルダ: ${skipped.join("、")}` : ""].filter(Boolean);
      setNcNote(notes.length ? notes.join(" ") : null);
    })();
    return () => { alive = false; };
    // 開いたときに1回だけ（プロジェクトの候補は開いている間変わらない）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  /** 並び（先頭＝プライマリ）を検査して入れる。使えないものは理由つきで残し、強い確認は未確認のまま。保存・作成・送信はしない。 */
  const applyFolders = async (folders: string[], note: string | null) => {
    setNcErr(null); setPrimaryStrong(null);
    const { primary, extras: rest } = splitForNewChat(await inspectFolders(folders, host.checkWorkFolder));
    if (!primary) return;
    setCwd(primary.path); setIsolate(false); setWt(null);
    setExtras(rest);
    if (primary.strong.length) setPrimaryStrong({ path: primary.path, reasons: primary.strong, confirmed: false });
    if (primary.rejected) setNcErr(`プライマリ: ${primary.rejected}: ${primary.path}`);
    const block = rest.length > 0 ? addBlockReason(workspaceRoots) : null;
    const notes = [note, block].filter(Boolean).join(" ");
    setNcNote(notes || null);
  };
  useEffect(() => {
    if (d.type !== "newChat" || !d.initialFolders) return;
    if (live) void applyFolders(d.initialFolders, d.folderNote ?? null); else setNcNote(d.folderNote ?? null);
    // 開いたときに1回だけ
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  /** プライマリの「参照…」。選んだパスはすぐ検査する（拒否は理由を出し、強い確認はチェックを出す）。 */
  const browsePrimary = async () => {
    const p = await folderTools.browse();
    if (!p) return;
    setCwd(p); setIsolate(false); setWt(null); setPrimaryStrong(null); setNcErr(null);
    if (!live) return;
    try {
      const c = await host.checkWorkFolder(p, extras.map((e) => e.path));
      if (c.kind === "rejected") { setNcErr(`プライマリ: ${REJECT_TEXT[c.reason]}`); return; }
      setCwd(c.normalized);
      if (c.kind === "needsStrongConfirm") setPrimaryStrong({ path: c.normalized, reasons: c.reasons, confirmed: false });
    } catch (e) { setNcErr(e instanceof Error ? e.message : "検査できませんでした"); }
  };
  const createNewChat = async () => {
    if (creating) return; // 二重押し
    const dev = kind === "dev";
    const primary = cwd.trim();
    let strong = confirmedStrongPaths(extras);
    setNcErr(null);
    setCreating(true);
    if (dev) {
      const why = saveBlockReason([{ path: primary, strong: [], confirmed: false }, ...extras], workspaceRoots);
      if (why) { setNcErr(why); setCreating(false); return; }
      try {
        // プライマリも（追加フォルダなしでも）、並べた状態で検査する（強い確認・重複）。ホストも同じ検査をやり直す。
        const c = await host.checkWorkFolder(primary, extras.map((e) => e.path));
        if (c.kind === "rejected") { setNcErr(`プライマリ: ${REJECT_TEXT[c.reason]}`); setCreating(false); return; }
        if (c.kind === "needsStrongConfirm") {
          if (!primaryStrong || primaryStrong.path !== c.normalized || !primaryStrong.confirmed) {
            setPrimaryStrong({ path: c.normalized, reasons: c.reasons, confirmed: false });
            setNcErr("プライマリのフォルダの確認が必要です。内容を確認してチェックを入れてください。");
            setCreating(false);
            return;
          }
          strong = [...strong, c.normalized];
        }
      } catch (e) { setNcErr(e instanceof Error ? e.message : "検査できませんでした"); setCreating(false); return; }
    }
    // 以後は呼び出し側が仮行で示す。作成が終わるまでボタンは押せないまま（完了で閉じる）。
    try { await onCreateChat({ cwd: dev ? primary : null, model, firstMessage: first.trim() || null, isolate: dev && isolate, worktree: dev && !isolate ? wt : null, extraFolders: dev ? extras.map((e) => e.path) : [], confirmedStrong: dev ? strong : [] }); } finally { setCreating(false); }
  };
  const [resendRunning, setResendRunning] = useState(false);
  void chats;
  switch (d.type) {
    case "settings": return (
      <Shell title="設定" wide onClose={onClose} close="閉じる">
        <div className="tabs vt" role="tablist" aria-orientation="vertical">{TABS.map(([k, t]) => <button key={k} role="tab" aria-selected={d.tab === k} onClick={() => setTab(k)}>{t}</button>)}</div>
        <div className="content"><SettingsBody tab={d.tab} enterMode={enterMode} setEnterMode={setEnterMode} top={top} source={source} models={models} exe={exe} notify={notify} autostart={autostart} appearance={appearance} manage={manage} chats={chats} compose={compose} opCaps={opCaps} org={org.settings} cont={cont} /></div>
      </Shell>);
    case "orgStart": return (
      <Shell title="組織実行を開始" onClose={onClose} close="キャンセル">
        <OrgStartBody start={org.start} browse={org.browse} live={org.settings.live} loadGraphs={org.loadGraphs} />
      </Shell>);
    case "orgDelete": return (
      <Shell title="ワークフローを削除" onClose={onClose} close="閉じる">
        <OrgDeleteDialog workflowId={d.workflowId} live={org.settings.live} preview={org.previewDelete} remove={org.deleteWorkflow} onDone={() => org.onDeleted(d.workflowId)} />
      </Shell>);
    case "newChat": return (
      <Shell title="新しいチャット" onClose={onClose} foot={<><button className="btn" onClick={onClose}>キャンセル</button><button className="btn primary" disabled={(kind === "dev" && !cwd.trim()) || creating} onClick={() => void createNewChat()}>{creating ? <><span className="spinner" aria-hidden="true" /> 作成中…</> : kind === "dev" && isolate ? "worktreeを作って開始" : "作成"}</button></>}>
        <div className="content">
          <div className="field"><span>AI</span><div><label><input type="radio" defaultChecked />Codex</label><div className="small muted">他の AI は今後追加できるようにする予定です。</div></div></div>
          <div className="field"><span>作業フォルダ</span><div><label><input type="radio" name="k" checked={kind === "general"} onChange={() => setKind("general")} />指定しない（一般チャット）</label><br /><label><input type="radio" name="k" checked={kind === "dev"} onChange={() => setKind("dev")} />フォルダを指定する</label>
            {kind === "dev" && live ? (
              <div style={{ marginTop: 4 }}>
                <FolderSetBar tools={folderTools} current={[cwd.trim(), ...extras.map((e) => e.path)]}
                  saveBlock={!cwd.trim() ? "プライマリのフォルダを指定してください" : extras.some((e) => e.rejected) ? "使えないフォルダがあります（外すか置き換えてから保存してください）" : null}
                  onPick={(set) => void applyFolders(set.folders, null)} />
              </div>) : null}
            {kind === "dev" ? (
              <div style={{ display: "flex", gap: 4, marginTop: 4 }}>
                <input type="text" className="mono grow" value={cwd} onChange={(e) => { setCwd(e.target.value); setIsolate(false); setWt(null); setPrimaryStrong(null); setNcErr(null); }} placeholder="作業フォルダのフルパス" aria-label="作業フォルダのパス" />
                <button type="button" className="btn sm" title="フォルダを選んで検査します" onClick={() => void browsePrimary()}>参照…</button>
              </div>) : null}
            {kind === "dev" ? (
              <div className="small" style={{ marginTop: 6 }}>
                <div className="muted">追加の作業フォルダ（上のフォルダがプライマリ。Codex が検索・読取り・編集できる範囲が広がります）</div>
                {projectChoices && projectChoices.length > 1 ? <div className="muted">プロジェクトのフォルダを並べました（先頭をプライマリと推定しています）。不要なものは外せます。</div> : null}
                {live ? <FolderListEditor items={extras} setItems={(v) => { setExtras(v); setNcErr(null); }} support={workspaceRoots} check={host.checkWorkFolder} minItems={0} primaryFixed browse={folderTools.browse} /> : <div className="muted">モックでは動きません。</div>}
                {primaryStrong ? <label style={{ display: "block", marginTop: 4 }}><input type="checkbox" checked={primaryStrong.confirmed} onChange={(e) => setPrimaryStrong({ ...primaryStrong, confirmed: e.target.checked })} /> プライマリのフォルダ全体を許可することを理解しました（{strongText(primaryStrong.reasons)}）</label> : null}
                {ncNote ? <div className="muted" style={{ marginTop: 2 }}>{ncNote}</div> : null}
                {ncErr ? <div className="why err" role="alert" style={{ marginTop: 4 }}>{ncErr}</div> : null}
              </div>) : null}</div></div>
          {kind === "dev" ? <WorkspaceChoice live={live} cwd={cwd} isolate={isolate} setIsolate={setIsolate} onUse={(p, r) => { setCwd(p); setWt(r); setIsolate(false); }} /> : null}
          <div className="field"><span>モデル</span><div style={{ display: "flex", gap: 6 }}><select value={model} onChange={(e) => setModel(e.target.value)}>{models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></div><span className="note">このチャットの最初の選択です。Codex から取得した一覧です。</span></div>
          <div className="field"><span>最初の依頼</span><textarea value={first} onChange={(e) => setFirst(e.target.value)} rows={3} style={{ width: "100%" }} placeholder="空欄なら、チャットだけ作成します" aria-label="最初の依頼"
            onKeyDown={(e) => {
              if (!isNewChatSubmitKey({ key: e.key, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: e.metaKey, isComposing: e.nativeEvent.isComposing }, enterMode)) return;
              e.preventDefault();
              if (!((kind === "dev" && !cwd.trim()) || creating)) void createNewChat();
            }} />
            <span className="note">{enterMode === "ctrl" ? "Ctrl＋Enterで作成、Enterで改行" : "EnterまたはCtrl＋Enterで作成、Shift＋Enterで改行"}</span></div>
        </div>
      </Shell>);
    case "rename": return <RenameDialog chat={chats.find((x) => x.key.id === d.chatId)} onClose={onClose} run={(n) => onRename(d.chatId, n)} />;
    case "quit": return <QuitDialog q={quit} chats={chats} onClose={onClose} onAct={onAct} />;
    case "force": return <ForceDialog f={force} chats={chats} onClose={onClose} />;
    case "delete": {
      const m = manage.del;
      const c = chats.find((x) => x.key.id === d.chatId);
      const finished = m.outcome !== null;
      const pv = m.preview;
      const label = m.running ? "削除しています…" : pv?.requiresStop ? "中断して削除" : "削除";
      return (
        <Shell title="チャットを削除" onClose={onClose}
          foot={<><button className="btn" onClick={onClose}>{finished ? "閉じる" : "キャンセル"}</button>{finished && m.outcome?.kind !== "partial" ? null : <button className="btn danger" disabled={!pv || m.running || !!pv.blockedReason} title={pv?.blockedReason ?? undefined} onClick={m.run}>{label}</button>}</>}>
          <DeleteBody chat={c} d={m} />
        </Shell>);
    }
    case "export": {
      const m = manage.exp;
      return (
        <Shell title="Markdown にエクスポート" onClose={onClose}
          foot={<><button className="btn" onClick={onClose}>キャンセル</button><button className="btn primary" disabled={m.running} onClick={m.run}>{m.running ? "書き出しています…" : "保存先を選ぶ…"}</button></>}>
          <ExportBody e={m} />
        </Shell>);
    }
    case "openLink": return (
      <Shell title="外部のページを開く" onClose={onClose} close="キャンセル">
        <OpenLinkBody url={d.url} text={d.text} onClose={onClose} />
      </Shell>);
    case "unv": return (
      <Shell title="この操作はまだ使えません" onClose={onClose} close="閉じる">
        <div className="content"><p>{d.why}。</p><p className="small muted">Codex 側の経路と動作を確認できるまで、成功したように見せることはしません。</p></div>
      </Shell>);
    case "changes": case "revert": return (
      <Shell title="変更を確認" wide full onClose={onClose} close="閉じる">
        <ChangesPanel chat={chats.find((x) => x.key.id === d.chatId)} live={live} tick={changesTick} caps={[opCaps.find((c) => c.op === "changeList"), opCaps.find((c) => c.op === "revertChanges")]} turnRecords={turnRecords} onDone={onRevertDone} baselinesEnabled={manage.baselines.enabled} repoPrefs={manage.baselines.repos} onSetRepo={manage.baselines.setRepo} />
      </Shell>);
    case "review": case "compact": case "fork": {
      const c = chats.find((x) => x.key.id === d.chatId);
      const pending = threadOps.pendingOps.filter((p) => p.chat.id === d.chatId).map((p) => p.pending);
      const waiting = threadOps.queues.find((q) => q.chat.id === d.chatId)?.entries.filter((e) => e.state.kind === "waiting").length ?? 0;
      const common = { chat: c, live, pending, waiting, onOpenChat: (k: ChatKey) => { onClose(); threadOps.onOpenChat(k); } };
      if (d.type === "review") return (
        <Shell title="コードレビュー" wide full onClose={onClose} close="閉じる">
          <ReviewBody {...common} cap={opCaps.find((x) => x.op === "codeReview")} />
        </Shell>);
      if (d.type === "fork") return (
        <Shell title="会話を分岐" onClose={onClose} close="閉じる">
          <ForkBody chat={c} live={live} cap={opCaps.find((x) => x.op === "fork")} throughTurn={d.throughTurn} onOpenChat={common.onOpenChat} />
        </Shell>);
      return (
        <Shell title="文脈を圧縮" wide onClose={onClose} close="閉じる">
          <CompactBody {...common} cap={opCaps.find((x) => x.op === "compact")} />
        </Shell>);
    }
    case "resend": {
      const st = resend.state;
      const blocked = !st || st.plan.kind === "blocked";
      return (
        <Shell title={d.purpose === "regenerate" ? "もう一度（再生成）" : "編集して分岐"} onClose={onClose} locked={resendRunning} close={blocked ? "閉じる" : "キャンセル"}>
          {st ? <ResendBody key={`${d.chatId}:${d.turnId}:${d.itemId ?? ""}`} chat={chats.find((x) => x.key.id === d.chatId)} live={live} cap={opCaps.find((x) => x.op === "fork")} s={st} h={resend.handlers} onClose={onClose} onRunning={setResendRunning} />
            : <div className="content"><p>対象の会話を読み込めていません。</p></div>}
        </Shell>);
    }
    case "goal": return (
      <Shell title="Goal" onClose={onClose} close="閉じる">
        <GoalBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} current={prefs.goal} live={live} cap={opCaps.find((c) => c.op === "goal")} save={(u) => prefs.saveGoal(d.chatId, u)} />
      </Shell>);
    case "purpose": {
      const ch = chats.find((x) => x.key.id === d.chatId);
      const lc = cont.locals.find((l) => l.chat.id === d.chatId);
      return (
        <Shell title="目的メモ" onClose={onClose} close="キャンセル">
          <PurposeBody key={d.chatId} chatName={ch ? chatName(ch) : "（不明）"} current={lc?.conversation?.purpose ?? null} live={live} orgOwned={!!lc?.org && !lc.org.released}
            save={(text, rev) => cont.savePurpose(d.chatId, text, rev)} onDone={onClose} />
        </Shell>);
    }
    case "handoffPreview": {
      const ch = chats.find((x) => x.key.id === d.chatId);
      return (
        <Shell title="引き継ぎ資料" wide onClose={onClose} close="閉じる">
          <HandoffPreviewBody key={d.chatId} chatName={ch ? chatName(ch) : "（不明）"} live={live} load={() => cont.loadPreview(d.chatId)} />
        </Shell>);
    }
    case "status": return (
      <Shell title="Codex の状態" wide onClose={onClose} close="閉じる">
        <StatusBody live={live} chat={chats.find((x) => x.key.id === d.chatId)} settings={prefs.settings} local={prefs.local} caps={opCaps} />
      </Shell>);
    case "usage": return (
      <Shell title="利用状況" onClose={onClose} close="閉じる">
        <div className="content"><UsagePanel live={live} /></div>
      </Shell>);
    case "worktrees": return (
      <Shell title="worktree の管理" wide onClose={onClose} close="閉じる">
        <WorktreesBody live={live} records={worktree.records} cwd={worktree.cwd} chats={chats} />
      </Shell>);
    case "cloud": return (
      <Shell title="クラウドに委任" wide onClose={onClose} close="閉じる">
        <CloudBody live={live} cap={opCaps.find((c) => c.op === "cloudDelegation")} chat={d.chatId ? chats.find((x) => x.key.id === d.chatId) : undefined} records={cloudTasks} />
      </Shell>);
    case "parity": return (
      <Shell title="同等性の確認状況" wide onClose={onClose} close="閉じる">
        <ParityBody caps={opCaps} />
      </Shell>);
    case "shortcuts": return (
      <Shell title="キーボードショートカット" onClose={onClose} close="閉じる"><ShortcutsBody /></Shell>);
    case "howto": return (
      <Shell title="状態の見方" wide onClose={onClose} close="閉じる"><HowtoBody /></Shell>);
    case "about": return (
      <Shell title="バージョン情報" onClose={onClose} close="閉じる"><AboutBody source={source} live={live} /></Shell>);
    case "reference": return (
      <Shell title="過去の会話を参考に" wide onClose={onClose} close="閉じる">
        <ReferenceBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} chats={chats} live={live} cap={opCaps.find((c) => c.op === "referenceChat")} insertText={compose.insertText} onDone={onClose} />
      </Shell>);
    case "skills": return (
      <Shell title="Skill を指定" wide onClose={onClose} close="閉じる">
        <SkillsBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} live={live} cap={opCaps.find((c) => c.op === "skills")} pickSkill={compose.pickSkill} onDone={onClose} recent={compose.recentSkills} onList={compose.noteSkills} />
      </Shell>);
    case "instructions": return (
      <Shell title="指示ファイル（AGENTS.md）" wide onClose={onClose} close="閉じる">
        <InstructionBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} live={live} cap={opCaps.find((c) => c.op === "instructionFiles")} insertText={compose.insertText} openFile={compose.openPath} />
      </Shell>);
    case "resumeExternal": return (
      <Shell title="外部の会話を再開" onClose={onClose} foot={<><button className="btn" onClick={onClose}>やめる</button><button className="btn primary" onClick={() => onAct("doResumeExternal")}>実行中ではないことを確認した。再開する</button></>}>
        <div className="content"><p>外部（VS Code／CLI）側で実行中でないことを確認しましたか？</p><p>実行中の場合は再開しないでください。同じ会話を二つの場所で同時に動かすと、履歴や作業内容が食い違うおそれがあります。</p></div>
      </Shell>);
    case "attach": return (
      <Shell title="追加" onClose={onClose}>
        <div className="content" style={{ display: "grid", gap: 6 }}>
          <button className="btn-line" onClick={() => onAct("attachPick")}><Icon name="clip" />ファイルを選ぶ（このチャット用にコピーします）</button>
          <button className="btn-line" onClick={() => onAct("attachPasteSelection")}><Icon name="copy" />選択範囲を貼り付け（クリップボードの文章をコード枠で入力欄へ）</button>
          <button className="btn-line" disabled={opCaps.find((c) => c.op === "referenceChat")?.support !== "supported"} onClick={() => onAct("reference")}><Icon name="chat" />過去の会話を参考に…（本文の抜粋を入力欄へ。要約しません）</button>
          <button className="btn-line" disabled={opCaps.find((c) => c.op === "skills")?.support !== "supported"} onClick={() => onAct("skills")}><Icon name="check" />Skill を指定…（送信時に明示的に呼び出します）</button>
          <button className="btn-line" disabled={opCaps.find((c) => c.op === "sideChat")?.support !== "supported"} onClick={() => onAct("sideOpen")}><Icon name="branch" />side相談を開く（読み取り専用の一時的な相談。主会話には影響しません）</button>
          {compose.sides.length ? (
            <div className="small" style={{ borderTop: "1px solid var(--line2)", paddingTop: 6 }}>このチャットのside相談:
              {compose.sides.map((s) => (
                <div key={s.id} className="frow"><span className="grow">{s.openedAt !== null ? new Date(s.openedAt).toLocaleString("ja-JP") : "（開いた時刻は不明）"}　{s.state.kind === "open" ? "開いています" : "終了（再開不可）"}</span><button className="small" onClick={() => compose.onShowSide(s.id)}>{s.state.kind === "open" ? "パネルを開く" : "記録を見る"}</button></div>
              ))}
            </div>
          ) : null}
          <p className="small muted">ドラッグ＆ドロップや、画像の貼り付け（Ctrl＋V）でも添付できます。元のファイルは変更しません。フォルダは作業フォルダの指定として扱い、中身をコピーしません。</p>
        </div>
      </Shell>);
  }
}

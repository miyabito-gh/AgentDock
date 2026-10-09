import { useState, type ReactNode } from "react";
import type { Chat, ChatKey, ChatLocalView, ChatPendingOp, ChatQueue, ChatModelSettings, ForceKillPreview, Goal, GoalUpdate, Known, ModelInfo, NotificationSettings, OpCapability, QuitDecision, QuitPhase, RevertResult, SaveStatus, SideSessionMeta, SourceInfo, StopRecord, StopSummary, WorktreeRecord, CloudTaskRecord } from "../ipc/types";
import { chatName } from "./derive";
import { SCOPE_TEXT } from "./Chrome";
import { Icon } from "./Icon";
import { DeleteBody, ExportBody, StorageBody, type BaselineProps, type DeleteProps, type ExportProps, type UsageProps } from "./ManageDialogs";
import { FootCtx, useFootSlots, About, ConfirmBlock } from "./DialogParts";
import { ParityBody } from "./ParityDialog";
import { ChangesBody, RevertBody } from "./ChangesDialog";
import { GoalBody, StatusBody } from "./PrefsDialogs";
import { CompactBody, ForkBody, ReviewBody } from "./ThreadOpsDialogs";
import { ExtensionsBody } from "./ExtensionsDialogs";
import { InstructionBody, ReferenceBody, SkillsBody, type ComposeProps } from "./ComposeDialogs";
import { WorkspaceChoice, WorktreesBody } from "./WorktreeDialogs";
import { CloudBody } from "./CloudDialogs";

export type DialogState =
  | { type: "settings"; tab: string }
  | { type: "newChat"; dev?: boolean }
  | { type: "worktrees" }
  | { type: "cloud"; chatId: string | null }
  | { type: "quit" }
  | { type: "force" }
  | { type: "delete"; chatId: string }
  | { type: "export"; chatId: string }
  | { type: "unv"; why: string }
  | { type: "parity" }
  | { type: "changes"; chatId: string }
  | { type: "revert"; chatId: string }
  | { type: "review"; chatId: string }
  | { type: "fork"; chatId: string; throughTurn: string | null }
  | { type: "compact"; chatId: string }
  | { type: "goal"; chatId: string }
  | { type: "status"; chatId: string | null }
  | { type: "attach" }
  | { type: "reference"; chatId: string }
  | { type: "skills"; chatId: string }
  | { type: "instructions"; chatId: string }
  | { type: "resumeExternal"; chatId: string }
  | { type: "rename"; chatId: string };

/** 外枠。フッターは左＝副操作、右＝閉じる（キャンセル）＋主操作（いちばん右）。
 *  `close` を渡すと、閉じるボタン付きのフッターを作り、本文側が FootActions で副操作・主操作を差し込める。
 *  `foot` は主操作・危険操作まで呼び出し側が組むとき（閉じる／キャンセルも含めて渡す）。`footLeft` は左の副操作。 */
function Shell({ title, children, foot, footLeft, close, wide, full, onClose }: { title: string; children: ReactNode; foot?: ReactNode; footLeft?: ReactNode; close?: string; wide?: boolean; full?: boolean; onClose: () => void }) {
  const fs = useFootSlots();
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className={`dialog ${wide ? "wide" : ""} ${full ? "full-narrow" : ""}`} role="dialog" aria-modal="true" aria-label={title}>
        <header><h3>{title}</h3><button className="btn subtle icon" aria-label="閉じる" onClick={onClose}><Icon name="x" /></button></header>
        <FootCtx.Provider value={fs.slots}>{children}</FootCtx.Provider>
        {close ? (
          <footer>
            <div className="foot-l" ref={fs.setLeft} />
            <div className="foot-r"><button className="btn" onClick={onClose}>{close}</button><span className="foot-main" ref={fs.setMain} /></div>
          </footer>
        ) : foot ? <footer><div className="foot-l">{footLeft}</div><div className="foot-r">{foot}</div></footer> : null}
      </div>
    </div>
  );
}

const TABS: Array<[string, string]> = [["general", "全般"], ["notify", "通知"], ["input", "入力"], ["model", "モデル"], ["codex", "Codex"], ["mcp", "MCP・Plugins"], ["skills", "Skills"], ["storage", "保存と容量"]];

export interface CodexExe { path: string; setPath: (p: string) => void; placeholder: string; connect: (p: string) => void; openDiag: () => void; browse: () => void; live: boolean }

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

export interface AutostartProps { value: boolean; set: (on: boolean) => void }

/** 完全終了の進行（ホストの `quitUpdated`）と、画面から出す選択。 */
export interface QuitProps {
  phase: QuitPhase; stops: StopRecord[]; failedSaves: SaveStatus[]; live: boolean;
  decide: (d: QuitDecision) => void; onForce: () => void; onRetrySave: () => void;
}

/** 強制終了の確認（`preview_force_kill` の結果）。preview が null の間は取得中（error があれば取得失敗）。 */
export interface ForceProps { preview: ForceKillPreview | null; error: string | null; running: boolean; run: () => void }

function SettingsBody({ tab, enterMode, setEnterMode, top, source, models, exe, notify, autostart, manage, chats, compose, opCaps }: {
  manage: ManageProps; chats: Chat[]; compose: ComposeDialogProps; opCaps: OpCapability[];
  autostart: AutostartProps;
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
    case "notify": return (
      <>
        <div className="field"><span>通知</span><label><input type="checkbox" checked={notify.value.enabled} onChange={(e) => notify.set({ ...notify.value, enabled: e.target.checked })} />通知を使う</label></div>
        <div className="field"><span>種類</span><div>
          <label><input type="checkbox" checked={notify.value.approvalAndQuestion} onChange={(e) => notify.set({ ...notify.value, approvalAndQuestion: e.target.checked })} />承認・質問待ち</label>
          <label><input type="checkbox" checked={notify.value.completed} onChange={(e) => notify.set({ ...notify.value, completed: e.target.checked })} />完了</label>
          <label><input type="checkbox" checked={notify.value.failed} onChange={(e) => notify.set({ ...notify.value, failed: e.target.checked })} />失敗</label></div>
          <span className="note">子・孫の完了と失敗は、チャットごとに2秒間まとめて通知します。承認と質問は待たずに通知します。中断は「失敗」の設定に従います。</span></div>
        <div className="field"><span>音</span><label><input type="checkbox" checked={notify.value.sound} onChange={(e) => notify.set({ ...notify.value, sound: e.target.checked })} />Windows の標準の音を鳴らす</label></div>
        <div className="field"><span>チャット名</span><label><input type="checkbox" checked={!notify.value.showChatName} onChange={(e) => notify.set({ ...notify.value, showChatName: !e.target.checked })} />通知にチャット名を出さず、状態だけ表示する</label></div>
        <div className="field"><span>抑制</span><span className="small">通常画面を操作している間だけ、選択中のチャットの通知を止めます。別のチャット、画面が背後にあるとき、監視窓だけのときは通知します。通知を開くと該当のチャットを表示するだけで、回答や再実行はしません。</span></div>
      </>);
    case "input": return (
      <>
        <div className="field"><span>送信キー</span><div>
          <label><input type="radio" name="ek" checked={enterMode === "ctrl"} onChange={() => setEnterMode("ctrl")} />Ctrl＋Enter で送信／Enter で改行</label><br />
          <label><input type="radio" name="ek" checked={enterMode === "enter"} onChange={() => setEnterMode("enter")} />Enter で送信／Shift＋Enter で改行</label></div></div>
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
          <SkillsBody chat={c} live={compose.live} cap={opCaps.find((x) => x.op === "skills")} />
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
  return r.kind === "turn" ? `エージェント ${r.turn.agent.id}` : r.kind === "managedExec" ? `管理実行 ${r.execId}` : `OSプロセス ${r.pid}`;
}

/** 完全終了の確認・進行。閉じる操作（トレイ格納）ではここを出さない。停止は証拠で確認し、時間経過だけでは確認にしない。 */
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
                    <span className="sub" key={i}>{targetText(t)}: {STOP_TEXT[t.summary]}</span>
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
export interface NewChatInput { cwd: string | null; model: string; firstMessage: string | null; isolate: boolean; worktree: string | null }

export function Dialogs({ d, onClose, chats, source, models, enterMode, setEnterMode, top, setTab, onAct, exe, onCreateChat, notify, autostart, quit, force, manage, onRename, opCaps, live, changesTick, onRevertDone, prefs, threadOps, worktree, cloudTasks, compose }: {
  compose: ComposeDialogProps;
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
  onRevertDone: (r: RevertResult | null) => void;
  onRename: (chatId: string, name: string) => Promise<string | null>;
  autostart: AutostartProps; quit: QuitProps; force: ForceProps; manage: ManageProps;
  notify: NotifyProps;
  d: DialogState; onClose: () => void; chats: Chat[]; source: SourceInfo | undefined; models: ModelInfo[];
  exe: CodexExe; onCreateChat: (i: NewChatInput) => void;
  enterMode: "ctrl" | "enter"; setEnterMode: (m: "ctrl" | "enter") => void;
  top: { main: boolean; mini: boolean; setMain: (b: boolean) => void; setMini: (b: boolean) => void };
  setTab: (t: string) => void; onAct: (a: string) => void;
}) {
  const [kind, setKind] = useState(d.type === "newChat" && d.dev ? "dev" : "general");
  const [cwd, setCwd] = useState("");
  const [isolate, setIsolate] = useState(false);
  const [wt, setWt] = useState<string | null>(null);
  const [model, setModel] = useState(models.find((m) => m.isDefault)?.id ?? models[0]?.id ?? "");
  const [first, setFirst] = useState("");
  void chats;
  switch (d.type) {
    case "settings": return (
      <Shell title="設定" wide onClose={onClose} close="閉じる">
        <div className="tabs" role="tablist">{TABS.map(([k, t]) => <button key={k} role="tab" aria-selected={d.tab === k} onClick={() => setTab(k)}>{t}</button>)}</div>
        <div className="content"><SettingsBody tab={d.tab} enterMode={enterMode} setEnterMode={setEnterMode} top={top} source={source} models={models} exe={exe} notify={notify} autostart={autostart} manage={manage} chats={chats} compose={compose} opCaps={opCaps} /></div>
      </Shell>);
    case "newChat": return (
      <Shell title="新しいチャット" onClose={onClose} foot={<><button className="btn" onClick={onClose}>キャンセル</button><button className="btn primary" disabled={kind === "dev" && !cwd.trim()} onClick={() => onCreateChat({ cwd: kind === "dev" ? cwd.trim() : null, model, firstMessage: first.trim() || null, isolate: kind === "dev" && isolate, worktree: kind === "dev" && !isolate ? wt : null })}>{kind === "dev" && isolate ? "worktreeを作って開始" : "作成"}</button></>}>
        <div className="content">
          <div className="field"><span>AI</span><div><label><input type="radio" defaultChecked />Codex</label><div className="small muted">他の AI は今後追加できるようにする予定です。</div></div></div>
          <div className="field"><span>作業フォルダ</span><div><label><input type="radio" name="k" checked={kind === "general"} onChange={() => setKind("general")} />指定しない（一般チャット）</label><br /><label><input type="radio" name="k" checked={kind === "dev"} onChange={() => setKind("dev")} />フォルダを指定する</label>
            {kind === "dev" ? <input type="text" className="mono" style={{ width: "100%", marginTop: 4 }} value={cwd} onChange={(e) => { setCwd(e.target.value); setIsolate(false); setWt(null); }} placeholder="作業フォルダのフルパス" aria-label="作業フォルダのパス" /> : null}</div></div>
          {kind === "dev" ? <WorkspaceChoice live={live} cwd={cwd} isolate={isolate} setIsolate={setIsolate} onUse={(p, r) => { setCwd(p); setWt(r); setIsolate(false); }} /> : null}
          <div className="field"><span>モデル</span><div style={{ display: "flex", gap: 6 }}><select value={model} onChange={(e) => setModel(e.target.value)}>{models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></div><span className="note">このチャットの最初の選択です。Codex から取得した一覧です。</span></div>
          <div className="field"><span>最初の依頼</span><textarea value={first} onChange={(e) => setFirst(e.target.value)} rows={3} style={{ width: "100%" }} placeholder="空欄なら、チャットだけ作成します" aria-label="最初の依頼" /></div>
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
          foot={<><button className="btn" onClick={onClose}>{finished ? "閉じる" : "キャンセル"}</button>{finished && m.outcome?.kind !== "partial" ? null : <button className="btn danger" disabled={!pv || m.running} onClick={m.run}>{label}</button>}</>}>
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
    case "unv": return (
      <Shell title="この操作はまだ使えません" onClose={onClose} close="閉じる">
        <div className="content"><p>{d.why}。</p><p className="small muted">Codex 側の経路と動作を確認できるまで、成功したように見せることはしません。</p></div>
      </Shell>);
    case "changes": return (
      <Shell title="変更ファイルと差分" wide full onClose={onClose} close="閉じる">
        <ChangesBody chat={chats.find((x) => x.key.id === d.chatId)} live={live} tick={changesTick} cap={opCaps.find((c) => c.op === "changeList")} onRevert={() => onAct("revert")} />
      </Shell>);
    case "revert": return (
      <Shell title="変更を戻す" wide full onClose={onClose} close="閉じる">
        <RevertBody chat={chats.find((x) => x.key.id === d.chatId)} live={live} cap={opCaps.find((c) => c.op === "revertChanges")} onDone={onRevertDone} />
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
    case "goal": return (
      <Shell title="Goal" onClose={onClose} close="閉じる">
        <GoalBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} current={prefs.goal} live={live} cap={opCaps.find((c) => c.op === "goal")} save={(u) => prefs.saveGoal(d.chatId, u)} />
      </Shell>);
    case "status": return (
      <Shell title="Codex の状態" wide onClose={onClose} close="閉じる">
        <StatusBody live={live} chat={chats.find((x) => x.key.id === d.chatId)} settings={prefs.settings} local={prefs.local} caps={opCaps} />
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
    case "reference": return (
      <Shell title="過去の会話を参考に" wide onClose={onClose} close="閉じる">
        <ReferenceBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} chats={chats} live={live} cap={opCaps.find((c) => c.op === "referenceChat")} insertText={compose.insertText} onDone={onClose} />
      </Shell>);
    case "skills": return (
      <Shell title="Skill を指定" wide onClose={onClose} close="閉じる">
        <SkillsBody key={d.chatId} chat={chats.find((x) => x.key.id === d.chatId)} live={live} cap={opCaps.find((c) => c.op === "skills")} pickSkill={compose.pickSkill} onDone={onClose} />
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

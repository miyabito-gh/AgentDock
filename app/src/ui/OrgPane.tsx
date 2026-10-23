// 組織実行のワークフロー画面・計画承認カード・左一覧の行・ドックのバッジ（P-ORG-7b。DESIGN_ORG §2）。
// 選ぶ・開くだけでは何も送らない。再開・一時停止・承認・取消し・手動切替・上限変更は、明示的なボタンだけがホストの命令を呼ぶ（危険なものは確認ブロックを経る）。
// 状態は文字＋アイコン名で示す（色だけにしない）。取得できない値は「—」「未取得」で、0や完了では代用しない。
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { AgentKey, Chat, ChatKey, ChatLocalView, HostSnapshot, OrgChatCandidate, OrgConfirmView, OrgGraphNodeView, OrgGraphView, OrgManualEnd, OrgManualResultView, OrgSnapshotView, OrgStatusView, OrgTaskView, OrgWorkflowView, PendingRequest } from "../ipc/types";
import * as host from "../ipc/client";
import { Icon, orgIcon } from "./Icon";
import { Popover } from "./Popover";
import { ConfirmBlock } from "./DialogParts";
import { tipWithKey } from "./shortcuts";
import { roleLabel } from "./orgChat";
import { CONFIRM_TONE, EFFECT_TEXT, NODE_KIND_TEXT, PHASE_TONE, waitCauseOf, artifactText, attemptText, retryText, edgeText, groupTasks, orgPendingRequests, requestWaitText, roleCounts, reworkLimitText, roundText, taskRequestWait, taskRowId, verdictText, waitBand, waitText, waitingDetail, workflowSummary, type OrgTone } from "./org";
import { RequestGapLine } from "./RequestGap";
import { keyStr, showKnown } from "./format";
import { InterjectComposer, NEXT_TURN_NOTE } from "./InterjectComposer";
import { OrgTimeline } from "./OrgTimeline";
import { activityLines, elapsedText, lineText, stabilize, type ActivityLine, type ChatActivity } from "./orgActivity";
import { useThrottled } from "./useThrottled";
import { takeNoteDraft } from "./orgNoteDraft";
import { rootView } from "./derive";
import { ArtifactsSection } from "./OrgArtifacts";

/** 状態の表示（アイコン＋文字。形・色は `.st.<tone>`）。 */
function Chip({ tone, icon, label, title }: { tone: OrgTone; icon: string; label: string; title?: string }) {
  return <span className={`st ${tone}`} title={title}><Icon name={orgIcon(icon)} />{label}</span>;
}
const StatusChip = ({ s }: { s: OrgStatusView }) => <Chip tone={PHASE_TONE[s.phase]} icon={s.icon} label={s.label} />;
const ConfirmChip = ({ c }: { c: OrgConfirmView }) => <Chip tone={CONFIRM_TONE[c.state]} icon={c.icon} label={c.label} />;

/** 左一覧のワークフロー行。選ぶと中央にワークフロー画面を開くだけ。 */
export function OrgListRow({ wf, selected, onOpen, onStart, counts }: { wf: OrgWorkflowView | null; selected: boolean; onOpen: () => void; onStart: () => void; /** 承認・質問の回答待ちと、回答を届けられなかった要求の件数（スナップショットから導いた値） */ counts?: { wait: number; fail: number } }) {
  if (!wf) return null;
  const sum = workflowSummary(wf);
  const name = wf.title ?? wf.request.slice(0, 30);
  return (
    <div className="left-org">
      <div className={`row-chat org-row${selected ? " sel" : ""}`}>
        <button type="button" className="row-main" aria-current={selected} onClick={onOpen} title={`組織実行「${name}」\n状態: ${sum.label}`}>
          <span className="tag ai" title="組織実行（複数の役割のチャットで進める実行）">組織</span>
          <span className="nm">{name}</span>
          <span className="marks">
            {sum.planWaiting ? <span className="mark wait" title="計画の承認待ち。ワークフロー画面で承認します">承認待ち</span> : null}
            {counts && counts.wait > 0 ? <span className="mark wait" title={`承認・質問の回答待ち ${counts.wait}件。ワークフロー画面から回答できます`}>回答待ち {counts.wait}</span> : null}
            {counts && counts.fail > 0 ? <span className="mark fail" title={`回答を届けられなかった要求 ${counts.fail}件。ワークフロー画面で理由と次の手を確認してください`}>回答不可 {counts.fail}</span> : null}
            {sum.attention > 0 ? <span className="mark fail" title={`止まっている・要確認のタスク ${sum.attention}件`}>要対応 {sum.attention}</span> : null}
            {wf.artifactHint && wf.artifactHint.ready > 0 ? <span className="mark wait" title="あなたの確認と取り込みを待っている成果（AIのレビュー中とは別）。ワークフロー画面の「成果物」で確認します">レビュー待ち {wf.artifactHint.ready}</span> : null}
          </span>
        </button>
        <Popover role="menu" align="right" triggerClassName="row-more" label={`組織実行「${name}」の操作`} trigger={<span aria-hidden="true">…</span>}>
          {(close) => (
            <>
              <button type="button" role="menuitem" onClick={() => { close(); onOpen(); }}>ワークフローを開く</button>
              <button type="button" role="menuitem" disabled={!wf.finished} title={wf.finished ? undefined : "実行中のワークフローがあります"} onClick={() => { close(); onStart(); }}>新しいワークフローを開始…</button>
            </>
          )}
        </Popover>
      </div>
      <div className="org-row-st"><Chip tone={sum.tone} icon={sum.icon} label={sum.label} /></div>
    </div>
  );
}

/** 左一覧の過去のワークフロー（保存済み。読み取り専用）。完了／失敗／取消しの印つき。多いときは折りたたむ。選ぶと中央に開くだけ。 */
const HISTORY_FOLDED = 3;
export function OrgHistoryRows({ history, selectedId, onOpen }: { history: OrgWorkflowView[]; selectedId: string | null; onOpen: (id: string) => void }) {
  const [all, setAll] = useState(false);
  const shown = all || history.length <= HISTORY_FOLDED + 1 ? history : history.slice(0, HISTORY_FOLDED);
  return (
    <div className="left-org" aria-label="過去のワークフロー">
      {shown.map((wf) => {
        const sum = workflowSummary(wf);
        const name = wf.title ?? wf.request.slice(0, 30);
        const sel = selectedId === wf.workflowId;
        return (
          <div key={wf.workflowId} className={`row-chat org-row${sel ? " sel" : ""}`}>
            <button type="button" className="row-main" aria-current={sel} onClick={() => onOpen(wf.workflowId)} title={`過去の組織実行「${name}」（読み取り専用）
状態: ${sum.label}`}>
              <span className="tag ai" title="過去の組織実行（保存済み。読み取り専用）">組織</span>
              <span className="nm">{name}</span>
              <span className="marks"><Chip tone={sum.tone} icon={sum.icon} label={sum.label} />{wf.artifactHint && wf.artifactHint.ready > 0 ? <span className="mark wait" title="あなたの確認と取り込みを待っている成果">レビュー待ち {wf.artifactHint.ready}</span> : null}</span>
            </button>
          </div>
        );
      })}
      {shown.length < history.length ? <button type="button" className="small" onClick={() => setAll(true)}>過去のワークフローをすべて表示（あと {history.length - shown.length} 件）</button> : null}
    </div>
  );
}

/** ドックのカード用の役割バッジとタスクID（組織実行のチャットだけ）。 */
export function OrgBadge({ local }: { local: ChatLocalView | undefined }) {
  const o = local?.org;
  if (!o) return null;
  return (
    <span className="tag ai org-badge" title={`組織実行のチャット（ワークフロー ${o.workflowId}）${o.released ? "。手動に切り替え済み" : ""}`}>
      {o.roleId}・{o.taskId}{o.released ? "・手動" : ""}
    </span>
  );
}

/** 組織実行のチャットへ手入力できない理由（送信欄の無効理由。ホストも同じ判定で拒否する）。 */
export const ORG_SEND_LOCK = "組織実行のチャットです。ここから通常の送信はできません（turn を始めるのは組織実行だけです）。中央の入力欄は「割り込み指示」で、実行中の turn に届けるか、次の送信に添えます。通常のチャットとして送るには、ワークフロー画面で「手動に切り替え」てください。";

type Confirm =
  | { kind: "resume"; /** 上限で止まったタスクのうち、再開の後に新しい試行として送るもの（チェックしたものだけ） */ checked: Record<string, boolean> }
  | { kind: "cancelWorkflow" }
  | { kind: "replan"; note: string }
  | { kind: "gate"; task: string; node: string; choice: string; label: string; needsNote: boolean; note: string }
  | { kind: "extraRound"; task: string; node: string; note: string }
  | { kind: "acceptRound"; task: string; node: string }
  | { kind: "cancelTask"; task: string }
  | { kind: "acceptManual"; task: string }
  | { kind: "retry"; task: string }
  | { kind: "retryNew"; task: string }
  | { kind: "failTask"; task: string }
  | { kind: "interject"; task: string }
  | { kind: "skip"; task: string }
  | { kind: "sendBack"; task: string; note: string }
  | { kind: "acceptAsIs"; task: string }
  | { kind: "stopTask"; task: string }
  | { kind: "adopt"; task: string; res: OrgManualResultView | null; error: string | null }
  | { kind: "stopRunning" }
  | { kind: "release"; task: string; chat: ChatKey }
  | { kind: "raise"; to: string }
  | { kind: "choose"; task: string; cands: OrgChatCandidate[] | null; error: string | null };

export interface OrgPaneProps {
  snap: HostSnapshot;
  /** null＝未取得（空のワークフローとは別）。 */
  org: OrgSnapshotView | null;
  error: string | null;
  live: boolean;
  leftShown: boolean; rightShown: boolean;
  onAct: (a: string) => void;
  onOpenChat: (key: ChatKey) => void;
  onReload: () => void;
  /** 命令の実行（App が実接続か確認し、失敗を通知し、成功したら取り直す）。f が文字を返したらそれを通知する。 */
  run: (label: string, f: () => Promise<string | void>) => Promise<boolean>;
  /** 削除の確認ダイアログを開く（開くだけ。消すのはダイアログの「削除する」）。 */
  onDelete: (workflowId: string) => void;
  /** 進行タイムラインの合図（orgTimelineUpdated）の回数。変わったら差分を取り直す。 */
  timelineTick: number;
  /** 承認・質問のカード（回答は App の既存の経路。ここからも直接答えられる）。 */
  renderRequest: (r: PendingRequest, agentName?: (a: AgentKey) => string) => ReactNode;
}

const END_TEXT: Record<OrgManualEnd, string> = { completed: "完了", running: "実行中", failed: "失敗", interrupted: "中断", unknown: "終端を確認できません" };

const chatName = (chats: Chat[], k: ChatKey | null | undefined): string => {
  const c = k ? chats.find((x) => x.key.id === k.id) : undefined;
  return c ? (c.name.kind === "value" ? c.name.value : "（名前未確認）") : k ? k.id : "—";
};

export function OrgPane(p: OrgPaneProps) {
  const { snap, org, run } = p;
  const wf = org?.workflow ?? null;
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busy, setBusy] = useState(false);
  // 別のワークフローに変わったら確認は閉じる（古い画面からの操作を防ぐ）。
  const wid = wf?.workflowId ?? null;
  useEffect(() => { setConfirm(null); }, [wid]);
  // Esc＝確認をやめる（IME変換中・実行中は閉じない）。
  useEffect(() => {
    if (!confirm) return;
    const h = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing || e.defaultPrevented || busy) return;
      e.preventDefault();
      setConfirm(null);
    };
    document.addEventListener("keydown", h);
    return () => document.removeEventListener("keydown", h);
  }, [confirm, busy]);

  // 表の上の「いまの動き」（役割ごとの現在の活動を1タスク1行）。更新は1秒に1回まで（承認・質問の増減だけ即時）、活動文は最低3秒は替えず、
  // 経過時間は30秒ごとに描き直す。鮮度は状態の文字とは別の印で示す。
  const [tab, setTab] = useState<"tasks" | "timeline">("tasks");
  const [tlTask, setTlTask] = useState<string | null>(null);
  const [nowTick, setNowTick] = useState(() => Date.now());
  useEffect(() => { const t = window.setInterval(() => setNowTick(Date.now()), 30_000); return () => window.clearInterval(t); }, []);
  const rawAct = useMemo(() => {
    if (!wf) return { lines: [] as ActivityLine[], done: 0 };
    const activityOf = (chat: ChatKey): ChatActivity | null => {
      const c = snap.chats.find((x) => x.key.id === chat.id);
      const v = c ? rootView(snap, c) : undefined;
      if (!v) return null;
      const a = v.currentActivity;
      return { live: v.freshness === "live", kind: a ? a.kind.kind : null, summary: a && a.summary.kind === "value" ? a.summary.value : null };
    };
    const waitOf = (t: OrgTaskView): string | null => (waitCauseOf(snap, t) === null ? null : requestWaitText(taskRequestWait(snap, t)) ?? t.attentionNote ?? t.status.label);
    return activityLines(wf, { now: Date.now(), waitOf, activityOf, roleLabel });
  }, [wf, snap]);
  const reqSig = snap.requests.filter((r) => r.state.kind === "pending").map((r) => `${r.key.source}/${r.key.requestId}`).join("|");
  const throttledAct = useThrottled(rawAct, 1000, reqSig);
  const prevAct = useRef<ActivityLine[]>([]);
  const actLines = useMemo(() => { const s = stabilize(prevAct.current, throttledAct.lines, Date.now()); prevAct.current = s; return s; }, [throttledAct]);
  /** 命令を実行する。成功したら確認ブロックを閉じ、失敗したら理由を通知して開いたままにする。 */
  const go = async (label: string, f: () => Promise<string | void>) => {
    setBusy(true);
    const ok = await run(label, f);
    setBusy(false);
    if (ok) setConfirm(null);
    return ok;
  };

  const head = (title: string, extra?: ReactNode) => (
    <div className="chead">
      <button className="btn subtle icon panebtn" aria-label="チャット一覧" aria-pressed={p.leftShown} title={tipWithKey(`チャット一覧を${p.leftShown ? "閉じる" : "開く"}`, "toggleLeft")} onClick={() => p.onAct("toggleLeft")}><Icon name="list" /></button>
      <span className="ttl" title={title}>{title}</span>
      {extra}
      <span className="grow" />
      <button className="btn subtle icon panebtn" aria-label="エージェントのドック" aria-pressed={p.rightShown} title={tipWithKey(`エージェントのドックを${p.rightShown ? "閉じる" : "開く"}`, "toggleRight")} onClick={() => p.onAct("toggleRight")}><Icon name="dock" /></button>
    </div>
  );

  if (!org || !wf) {
    return (
      <>
        {head("組織実行")}
        <div className="org-body">
          {p.error ? (
            <div className="banner err" role="alert"><Icon name="alert" /><div>組織実行の状態を取得できませんでした（{p.error}）。取得できていないだけで、実行が無いとは限りません。<div className="pop-acts"><button className="btn sm" onClick={p.onReload}>取り直す</button></div></div></div>
          ) : !org ? (
            <div className="empty-note">組織実行の状態を取得しています…</div>
          ) : (
            <div className="empty-note">
              <p>組織実行はありません。</p>
              <button className="btn primary" onClick={() => p.onAct("orgStart")}>組織実行を開始…</button>
            </div>
          )}
        </div>
      </>
    );
  }

  const sum = workflowSummary(wf);
  // 過去のワークフロー（保存データの読取り）。操作ボタンは出さず、タスク表と「チャットを見る」だけ。
  const ro = wf.readonly;
  const doneN = wf.tasks.filter((t) => t.status.phase === "completed").length;
  const approvals = orgPendingRequests(snap, wf.workflowId);
  const canResume = !!wf.halt?.resumable;
  const lim = wf.limits;
  const use = wf.usage;
  const chatExists = (k: ChatKey | null) => !!k && snap.chats.some((c) => c.key.id === k.id);

  const resumeFn = async (retryTasks: string[]): Promise<string> => {
    const r = await host.orgResume(wf.workflowId, retryTasks);
    const again = retryTasks.length > 0 ? `チェックした ${retryTasks.length} 件のタスクを新しい試行として送ります。` : "";
    return r.failed.length > 0
      ? `再開しました。ただし ${r.failed.length} 件のチャットは再開できず、そのタスクは送らずに止まっています（${r.failed[0].message}）。${again}`
      : `再開しました（チャット ${r.resumed.length} 件）。${again}`;
  };
  /** 利用上限で止まっていて、新しい試行で再送できるタスク（再開の確認にチェック付きで並べる）。 */
  const limitTasks = wf.tasks.filter((t) => t.status.reason === "usageLimit" && t.retry?.kind === "newAttempt");
  const openResume = () => setConfirm({ kind: "resume", checked: Object.fromEntries(limitTasks.map((t) => [t.id, true])) });
  const agentName = (a: AgentKey): string => { const v = snap.agents.find((x) => keyStr(x.agent.key) === keyStr(a)); return v ? showKnown(v.agent.displayName) : a.id; };
  const jumpTo = (id: string | null) => { const el = id ? document.getElementById(taskRowId(id)) : null; el?.scrollIntoView({ block: "center" }); el?.focus({ preventScroll: true }); };

  const noteField = (label: string, value: string, onChange: (v: string) => void, hint: string) => (
    <div className="field"><span>{label}</span>
      <textarea rows={3} style={{ width: "100%" }} value={value} maxLength={2000} aria-label={label} onChange={(e) => onChange(e.target.value)} />
      <span className="note">{hint}</span></div>
  );

  const confirmBlock = () => {
    if (!confirm) return null;
    const cancelBtn = <button className="btn" disabled={busy} onClick={() => setConfirm(null)}>やめる</button>;
    const wrap = (label: string, body: ReactNode, main: ReactNode) => <ConfirmBlock label={label} wide actions={<>{cancelBtn}{main}</>}>{body}</ConfirmBlock>;
    switch (confirm.kind) {
      case "resume":
        return wrap("再開の確認",
          <>
            <p>ワークフローを再開します。必要なチャットを Codex で再開（resume）し、止めていたタスクの送信を始めます。実行中だったタスクのチャットも再開して、その状態を受け取ります（再開した後に、履歴の読取りで終端を確認し直します）。再起動や通信断で状態を確認できていないタスクは、確認できるまで送りません。</p>
            {limitTasks.length > 0 ? (
              <>
                <p>利用上限で止まっているタスクです。チェックしたタスクを新しい試行として送ります（自動では送りません。この操作が送信の指示です）。チェックを外したタスクは止まったままです。</p>
                <ul className="org-resume-list">
                  {limitTasks.map((t) => (
                    <li key={t.id}><label><input type="checkbox" checked={!!confirm.checked[t.id]} onChange={(e) => setConfirm({ kind: "resume", checked: { ...confirm.checked, [t.id]: e.target.checked } })} /> <b className="mono">{t.id}</b> {t.title}<span className="small muted">　再送 {t.retry!.used}/{t.retry!.max}</span></label></li>
                  ))}
                </ul>
              </>
            ) : null}
          </>,
          <button className="btn primary" disabled={busy} onClick={() => void go("再開", () => resumeFn(limitTasks.filter((t) => confirm.checked[t.id]).map((t) => t.id)))}>{busy ? "再開しています…" : "再開する"}</button>);
      case "cancelWorkflow":
        return wrap("ワークフローの取消しの確認", <p>終わっていないタスクをすべて取り消します。実行中のタスクの停止は、Codex 側で確認できるまで「停止未確認」として扱います（取り消しただけで停止したとは表示しません）。取り消したものを元に戻すことはできません。</p>,
          <button className="btn danger" disabled={busy} onClick={() => void go("取り消し", async () => { const r = await host.orgCancel({ workflowId: wf.workflowId, scope: { kind: "workflow" } }); return `${r.length} 件のタスクを取り消しました。`; })}>{busy ? "取り消しています…" : "すべて取り消す"}</button>);
      case "cancelTask":
        return wrap(`タスク ${confirm.task} の取消しの確認`, <p>タスク {confirm.task} を取り消します。実行中なら、停止は Codex 側で確認できるまで「停止未確認」として扱います。依存するタスクは進めなくなります。</p>,
          <button className="btn danger" disabled={busy} onClick={() => void go("取り消し", async () => { await host.orgCancel({ workflowId: wf.workflowId, scope: { kind: "task", task: confirm.task } }); return `タスク ${confirm.task} を取り消しました。`; })}>{busy ? "取り消しています…" : "このタスクを取り消す"}</button>);
      case "replan":
        return wrap("再計画の確認",
          <>
            <p>今の計画を差し戻し、Planner に再計画させます。まだ実行していないタスクは作り直されます。再計画の結果は、確認が必要なら改めて承認カードに出ます。</p>
            {noteField("再計画への要望（任意）", confirm.note, (note) => setConfirm({ kind: "replan", note }), "例: タスクをもっと小さく分けてください。理由は Planner に利用者の要望として渡ります（権限や出力の形式は変わりません）。")}
          </>,
          <button className="btn primary" disabled={busy} onClick={() => void go("再計画の要求", async () => { await host.orgReplan(wf.workflowId, confirm.note.trim() || null); return "再計画を要求しました。"; })}>{busy ? "要求しています…" : "差し戻して再計画する"}</button>);
      case "gate": {
        const c = confirm;
        const ready = !c.needsNote || c.note.trim() !== "";
        return wrap(`判断の確認（${c.node}）`,
          <>
            <p>工程「{c.node}」の判断として「<b>{c.label}</b>」を選びます。選んだ道筋の次の工程が送られ始めます（差し戻しに進む選択は、周回の回数に数えられます）。送信が止まっているときは、判断を記録するだけで、「再開」するまで次の工程は送られません。</p>
            {noteField(c.needsNote ? "理由（必須）" : "理由（任意）", c.note, (note) => setConfirm({ ...c, note }), "戻り先の次の turn に、利用者の要望として渡ります（権限や出力の形式は変わりません）。")}
            {!ready ? <p className="small muted">この選択には理由が必要です。</p> : null}
          </>,
          <button className="btn primary" disabled={busy || !ready} onClick={() => void go("判断", async () => { await host.orgGateDecide(wf.workflowId, c.task, c.choice, c.note.trim() || null); return `「${c.label}」で進めます。`; })}>{busy ? "処理しています…" : "この判断で進める"}</button>);
      }
      case "extraRound": {
        const c = confirm;
        return wrap(`もう1周の確認（${c.node}）`,
          <>
            <p>工程「{c.node}」の周回が上限に達しています。<b>この辺の許容回数を1回だけ増やし</b>、もう1周します（自動では増えません。増やすたびにこの確認が要ります）。周回が増えるほど、使用量（トークン）も増えます。送信が止まっているときは、「再開」するまで送られません。</p>
            {noteField("次の周回への理由（任意）", c.note, (note) => setConfirm({ ...c, note }), "戻り先の次の turn に、利用者の要望として渡ります（権限や出力の形式は変わりません）。")}
          </>,
          <button className="btn primary" disabled={busy} onClick={() => void go("もう1周", async () => { await host.orgExtraRound(wf.workflowId, c.task, c.note.trim() || null); return "もう1周します。"; })}>{busy ? "処理しています…" : "理由を書いてもう1周する"}</button>);
      }
      case "acceptRound": {
        const c = confirm;
        return wrap(`この結果で受け入れる確認（${c.node}）`,
          <p>工程「{c.node}」の周回が上限に達しています。<b>今ある結果のまま</b>この周回を終え、次へ進めます（何も送りません。周回の回数も増えません）。検証で直すべきとされた点が残っている可能性があります。受け入れずに続けるなら「もう1周」、やめるなら取り消してください。</p>,
          <button className="btn primary" disabled={busy} onClick={() => void go("受け入れ", async () => { await host.orgAcceptRound(wf.workflowId, c.task); return "この結果で受け入れました。"; })}>{busy ? "処理しています…" : "この結果で受け入れる"}</button>);
      }
      case "acceptManual":
        return wrap(`タスク ${confirm.task} を手動で完了にする確認`, <p>タスク {confirm.task} を、あなたの判断で完了として扱います。Codex の結果を確認できたという意味ではありません。依存するタスクが進み始めます。</p>,
          <button className="btn primary" disabled={busy} onClick={() => void go("手動完了", async () => { await host.orgAcceptManual(wf.workflowId, confirm.task); return `タスク ${confirm.task} を完了にしました。`; })}>{busy ? "処理しています…" : "完了にする"}</button>);
      case "retry":
        return wrap(`タスク ${confirm.task} のやり直しの確認`, <p>タスク {confirm.task} を、もう一度送れる状態に戻します。このタスクは、まだ Codex に送られていないことを確認できています（受理されたか不明なものは、ここには出ません）。新しい試行として、通常の送信と同じ手順で送ります。送信が止まっているときは、先に「再開」が必要です。</p>,
          <button className="btn primary" disabled={busy} onClick={() => void go("やり直し", async () => { await host.orgRetryTask(wf.workflowId, confirm.task); return `タスク ${confirm.task} をやり直す状態に戻しました。`; })}>{busy ? "処理しています…" : "やり直す"}</button>);
      case "retryNew": {
        const t = wf.tasks.find((x) => x.id === confirm.task);
        const r = t?.retry;
        return wrap(`タスク ${confirm.task} の新しい試行の確認`,
          <>
            {r?.reasonLabel === "変更なし"
              ? <p>タスク {confirm.task} の前の turn は終わりましたが、<b>Worker が変更を書きませんでした</b>。<b>同じチャットへ「依頼と spec の範囲を実装し、変更を残してください」という新しい turn を1回だけ送ります</b>（自動では送りません。この操作が送信の指示です）。先に下の「ワークフローへの追加指示」を足しておくと、この送信に添えられます。</p>
              : <p>タスク {confirm.task} の前の turn は{r?.reasonLabel ? `「${r.reasonLabel}」で` : ""}結果を出さずに終わりました。<b>同じチャットへ、続きを行う新しい turn を1回だけ送ります</b>（自動では送りません。この操作が送信の指示です）。作業フォルダの途中の変更はそのまま引き継がれます。</p>}
            <p className="small">再送の回数: {r ? `これまで ${r.used} 回、上限 ${r.max} 回` : "—"}。この送信は turn 数にも数えられます。送信が止まっているときは、先に「再開」が必要です。</p>
          </>,
          <button className="btn primary" disabled={busy} onClick={() => void go("新しい試行での再送", async () => { await host.orgRetryTask(wf.workflowId, confirm.task); return `タスク ${confirm.task} を新しい試行として送る状態に戻しました。`; })}>{busy ? "処理しています…" : "新しい試行で再送する"}</button>);
      }
      case "failTask":
        return wrap(`タスク ${confirm.task} を失敗として確定する確認`, <p>タスク {confirm.task} を、あなたの判断で失敗として確定します。<b>元に戻せません</b>。依存するタスクは進めなくなります。Codex 側の状態を変えることはしません（再送・中断もしません）。</p>,
          <button className="btn danger" disabled={busy} onClick={() => void go("失敗として確定", async () => { await host.orgFailTask(wf.workflowId, confirm.task); return `タスク ${confirm.task} を失敗として確定しました。`; })}>{busy ? "処理しています…" : "失敗として確定する"}</button>);
      case "interject": {
        const t = wf.tasks.find((x) => x.id === confirm.task);
        if (!t) return null;
        return wrap(`${confirm.task} への指示`,
          <InterjectComposer workflowId={wf.workflowId} task={t} interjects={wf.interjects} run={p.run} compact />,
          null);
      }
      case "skip": {
        const t = wf.tasks.find((x) => x.id === confirm.task);
        const also = t?.skip?.also ?? [];
        const unrev = t?.skip?.completesUnreviewed ?? null;
        return wrap(`タスク ${confirm.task} のスキップの確認`,
          <>
            <p><b>{confirm.task}</b> をスキップします。{also.length > 0 ? <>あわせて <b>{also.join("、")}</b>（検証）も飛ばします。</> : null}{unrev ? <><b>{unrev}</b> は検証なしで完了にします。</> : null}成果は後続へ渡りません。<b>元に戻せません</b>。</p>
            <p className="small">後続の作業タスクは、スキップしたタスクを「満たした」とみなして進みます（依頼文に「スキップ: 成果なし」と書きます）。</p>
          </>,
          <button className="btn danger" disabled={busy} onClick={() => void go("スキップ", async () => { await host.orgSkipTask(wf.workflowId, confirm.task); return `タスク ${confirm.task} をスキップしました。`; })}>{busy ? "処理しています…" : "スキップする"}</button>);
      }
      case "sendBack": {
        const c = confirm;
        const t = wf.tasks.find((x) => x.id === c.task);
        const needNote = t?.sendBackNeedsNote ?? true;
        const ready = !needNote || c.note.trim() !== "";
        return wrap(`タスク ${c.task} の差し戻し`,
          <>
            <p><b>{c.task}</b> を、次の指摘で直させます。自動の差し戻し回数には数えません（turn を1つ使います）。</p>
            {noteField(needNote ? "指摘（必須）" : "指摘（任意）", c.note, (note) => setConfirm({ ...c, note }), "次の turn に、利用者の指摘として渡ります（権限や出力の形式は変わりません）。")}
          </>,
          <button className="btn primary" disabled={busy || !ready} onClick={() => void go("差し戻し", async () => { await host.orgSendBack(wf.workflowId, c.task, c.note.trim() || null); return `タスク ${c.task} を差し戻しました。`; })}>{busy ? "処理しています…" : "差し戻す"}</button>);
      }
      case "acceptAsIs": {
        const checkFailed = wf.tasks.find((x) => x.id === confirm.task)?.status.reason === "checkFailed";
        return wrap(`タスク ${confirm.task} をこのまま受け入れる確認`,
          <p>{checkFailed ? <>検証のコマンドが失敗したまま</> : <>検証で不合格のまま</>} <b>{confirm.task}</b> を完了にします（表示は「検証不合格のまま受け入れ」）。待っている検証役はスキップします。取込みは利用者が行います（自動マージしません）。</p>,
          <button className="btn danger" disabled={busy} onClick={() => void go("このまま受け入れる", async () => { await host.orgAcceptAsIs(wf.workflowId, confirm.task); return `タスク ${confirm.task} を受け入れました（検証不合格のまま）。`; })}>{busy ? "処理しています…" : "このまま受け入れる"}</button>);
      }
      case "stopTask": {
        const t = wf.tasks.find((x) => x.id === confirm.task);
        return wrap(`タスク ${confirm.task} だけ止める確認`,
          <>
            <p><b>{confirm.task}</b> のチャットへ<b>中断を要求</b>します。他のタスクとワークフローは続きます。</p>
            <p className="small">中断の要求は停止の確認ではありません。停止は Codex 側で確認できるまで「停止未確認」と扱い、10秒で確認できなければ案内します。止まれば「新しい試行で再送」できます（自動では送りません）。</p>
          </>,
          <button className="btn danger" disabled={busy || !t?.chat} onClick={() => void go("このタスクだけ止める", async () => { await host.interruptChat(t!.chat!); return `タスク ${confirm.task} のチャットに中断を要求しました。停止は確認できるまで「停止未確認」です。`; })}>{busy ? "止めています…" : "このタスクだけ止める"}</button>);
      }
      case "adopt": {
        const c = confirm;
        const res = c.res;
        const copyFormat = () => { if (res?.formatHint) void navigator.clipboard?.writeText(res.formatHint).then(() => p.run("形式の説明のコピー", async () => "形式の説明をコピーしました（送信はしていません）。")); };
        return wrap(`タスク ${c.task} の結果を取り込む確認`,
          <>
            <p>手動に切り替えたチャットの<b>最新の turn</b> の最終回答を、このタスクの結果として取り込みます。<b>チャットへは何も送りません</b>（読み取りだけ）。</p>
            {c.error ? <p className="why err" role="alert">{c.error}</p> : null}
            {res === null && !c.error ? <p className="small muted">最新の turn を読み込んでいます…</p> : null}
            {res ? (
              <>
                <p className="small">最新の turn の終端: <b>{END_TEXT[res.end]}</b>　履歴: {res.complete ? "全件読めました" : "全件は読めていません"}</p>
                {res.finalHead !== null ? <pre className="mono small org-pre org-pre-wrap">{res.finalHead}{res.finalHead.length >= 400 ? "\n…（先頭400字）" : ""}</pre> : <p className="small muted">最終回答を読めていません。</p>}
                {res.problem ? <p className="why err" role="alert">取り込めません: {res.problem}</p> : null}
              </>
            ) : null}
            <p>取り込むと、通常の完了判定（形式の検証・子孫の確認・Worker はコミット）に進めます。形式に合わなければ、手動のまま戻します。</p>
            <p className="small muted">形式を整えた回答をチャットで作らせたいときは、形式の説明をコピーして、あなたがチャットに貼り付けて送ってください（AgentDock は送りません）。</p>
          </>,
          <>
            <button className="btn" disabled={busy || !res?.formatHint} title={res?.formatHint ? "このタスクの結果の形式の説明をクリップボードへコピーします（送信はしません）" : "形式の説明を取得できていません"} onClick={copyFormat}>形式の説明をコピー</button>
            <button className="btn primary" disabled={busy || !res || !!res.problem || res.turnId === null} onClick={() => void go("結果の取り込み", async () => { await host.orgAdoptManual(wf.workflowId, c.task, res!.turnId!); return `タスク ${c.task} の結果を取り込みました。完了の判定に進みます。`; })}>{busy ? "取り込んでいます…" : "この回答を取り込む"}</button>
          </>);
      }
      case "stopRunning": {
        const running = wf.tasks.filter((t) => ["running", "reviewing"].includes(t.status.phase) && t.chat);
        return wrap("実行中も止める確認",
          <>
            <p>新しい送信を止め、実行中のタスク（{running.length}件{running.length ? `：${running.map((t) => t.id).join("、")}` : ""}）のチャットへ<b>中断を要求</b>します。</p>
            <p className="small">中断の要求は停止の確認ではありません。停止は Codex 側で確認できるまで「停止未確認」と扱い、10秒で確認できなければ案内します。タスクは取消しにも失敗にもなりません。止まったタスクは「新しい試行で再送」できます（送信は自動では再開しません）。</p>
          </>,
          <button className="btn danger" disabled={busy} onClick={() => void go("実行中も止める", async () => {
            await host.orgPause(wf.workflowId);
            let failed = 0;
            for (const t of running) { try { await host.interruptChat(t.chat!); } catch { failed += 1; } }
            return failed > 0 ? `新しい送信を止めました。${failed} 件のチャットには中断を要求できませんでした。` : `新しい送信を止め、実行中の ${running.length} 件に中断を要求しました。停止は確認できるまで「停止未確認」です。`;
          })}>{busy ? "止めています…" : "実行中も止める"}</button>);
      }
      case "release":
        return wrap(`タスク ${confirm.task} を手動に切り替える確認`, <p>このタスクのチャット「{chatName(snap.chats, confirm.chat)}」を組織実行から切り離し、通常のチャットとして手入力で送れるようにします。以後、組織実行はこのチャットへ送りません。元に戻す操作はありません。</p>,
          <button className="btn danger" disabled={busy} onClick={() => void go("手動への切替", async () => { await host.orgReleaseChat(confirm.chat); return "手動に切り替えました。"; })}>{busy ? "切り替えています…" : "手動に切り替える"}</button>);
      case "raise": {
        const to = Number(confirm.to);
        const valid = Number.isInteger(to) && to > lim.maxTurns;
        return wrap("turn上限の引き上げの確認",
          <>
            <p>このワークフロー全体の turn 数の上限を上げます。現在の上限は {lim.maxTurns}、使用済みは {use.turnsUsed}（残り {use.turnsRemaining}）です。turn が増えるほど、使用量（トークン）も増えます。</p>
            <label>新しい上限　<input type="number" min={lim.maxTurns + 1} step={1} value={confirm.to} aria-label="新しいturn上限" onChange={(e) => setConfirm({ kind: "raise", to: e.target.value })} style={{ width: 90 }} /></label>
            {!valid ? <p className="small muted">現在の上限（{lim.maxTurns}）より大きい整数を入れてください。</p> : null}
          </>,
          <button className="btn primary" disabled={busy || !valid} onClick={() => void go("上限の変更", async () => { await host.orgRaiseTurnLimit(wf.workflowId, to); return `turn上限を ${to} にしました。`; })}>{busy ? "変更しています…" : "上限を上げる"}</button>);
      }
      case "choose": {
        const c = confirm;
        return wrap(`タスク ${c.task} のチャットを選ぶ`,
          <>
            <p>このタスクのチャットを作成した結果を確認できていません。AgentDock が自動で結び付けることはしません。結び付けるチャットを、あなたが選んでください。</p>
            {c.error ? <p className="why err" role="alert">{c.error}</p> : null}
            {c.cands === null ? <p className="small muted">候補を読み込んでいます…</p> : c.cands.length === 0 ? <p className="small muted">結び付けられる候補はありません。</p> : (
              <ul className="org-cands">
                {c.cands.map((k) => (
                  <li key={k.chat.id}>
                    <span>{k.name ?? "（名前未確認）"}</span>
                    {k.selectable
                      ? <button className="btn sm" disabled={busy} onClick={() => void go("チャットの結び付け", async () => { await host.orgChooseChat(wf.workflowId, c.task, k.chat); return "チャットを結び付けました。"; })}>このチャットを結び付ける</button>
                      : <span className="small muted">選べません（{k.whyNot ?? "理由を取得できません"}）</span>}
                  </li>
                ))}
              </ul>
            )}
          </>, null);
      }
    }
  };

  const openChoose = async (task: string) => {
    setConfirm({ kind: "choose", task, cands: null, error: null });
    if (!p.live) { setConfirm({ kind: "choose", task, cands: [], error: "モックでは動きません。" }); return; }
    try { setConfirm({ kind: "choose", task, cands: await host.orgRecoverCandidates(wf.workflowId, task), error: null }); }
    catch (e) { setConfirm({ kind: "choose", task, cands: [], error: `候補を取得できませんでした（${host.asIpcError(e).message}）` }); }
  };

  const openAdopt = async (task: string) => {
    setConfirm({ kind: "adopt", task, res: null, error: null });
    if (!p.live) { setConfirm({ kind: "adopt", task, res: null, error: "モックでは動きません。" }); return; }
    try { setConfirm({ kind: "adopt", task, res: await host.orgManualResult(wf.workflowId, task), error: null }); }
    catch (e) { setConfirm({ kind: "adopt", task, res: null, error: `最新の turn を読めませんでした（${host.asIpcError(e).message}）` }); }
  };

  const taskActions = (t: OrgTaskView) => (
    <div className="org-acts">
      {t.chat ? <button className="btn sm" disabled={!chatExists(t.chat)} title={chatExists(t.chat) ? "このタスクのチャットを開きます（見るだけ。何も送りません）" : "チャットがまだ一覧にありません"} onClick={() => p.onOpenChat(t.chat!)}>チャットを見る</button> : null}
      {ro ? null : <>
      {t.needsChatChoice ? <button className="btn sm" onClick={() => void openChoose(t.id)}>チャットを選ぶ…</button> : null}
      {t.chat ? <button className="btn sm" onClick={() => setConfirm({ kind: "release", task: t.id, chat: t.chat! })} title="組織実行から切り離し、手入力できる通常のチャットにします">手動に切り替え…</button> : null}
      {t.retry?.kind === "unsent" ? <button className="btn sm" onClick={() => setConfirm({ kind: "retry", task: t.id })} title="まだ送られていないことを確認できているタスクを、もう一度送る状態に戻します">やり直す…</button> : null}
      {t.retry?.kind === "newAttempt" ? <button className="btn sm" onClick={() => setConfirm({ kind: "retryNew", task: t.id })} title={`前の turn（${t.retry.reasonLabel}）の続きを、同じチャットへ新しい試行として1回送ります`}>新しい試行で再送…（再送 {t.retry.used}/{t.retry.max}）</button> : null}
      {!t.retry && t.retryRefusal ? <button className="btn sm" disabled title={t.retryRefusal}>新しい試行で再送…</button> : null}
      {t.canFail ? <button className="btn sm danger-line" onClick={() => setConfirm({ kind: "failTask", task: t.id })} title="あなたの判断で失敗として確定します（元に戻せません）">失敗として確定…</button> : null}
      {t.canAdoptManual ? <button className="btn sm" onClick={() => void openAdopt(t.id)} title="手動に切り替えたチャットの最新の回答を、このタスクの結果として取り込みます（チャットへは何も送りません）">結果を取り込む…</button> : null}
      {t.canAcceptManual ? <button className="btn sm" onClick={() => setConfirm({ kind: "acceptManual", task: t.id })}>手動で完了にする…</button> : null}
      {t.canCancel ? <button className="btn sm danger-line" onClick={() => setConfirm({ kind: "cancelTask", task: t.id })}>取り消す…</button> : null}
      <Popover role="menu" align="right" triggerClassName="btn sm subtle" label={`タスク ${t.id} の操作`} title="このタスクへの指示・スキップ・差し戻しなど" trigger={<span aria-hidden="true">…</span>}>
        {(close) => (
          <>
            <div className="small muted" style={{ padding: "2px 8px" }}>このタスク（{t.id}）</div>
            <button type="button" role="menuitem" aria-disabled={t.interject.route === "refused" ? true : undefined} title={t.interject.label} onClick={() => { if (t.interject.route === "refused") return; close(); setConfirm({ kind: "interject", task: t.id }); }}>指示を挟む…</button>
            {t.skip || t.skipRefusal ? <button type="button" role="menuitem" aria-disabled={t.skip ? undefined : true} title={t.skip ? "このタスク（と、その検証役）を飛ばします" : t.skipRefusal ?? undefined} onClick={() => { if (!t.skip) return; close(); setConfirm({ kind: "skip", task: t.id }); }}>スキップ…</button> : null}
            {t.canSendBack || t.sendBackRefusal ? <button type="button" role="menuitem" aria-disabled={t.canSendBack ? undefined : true} title={t.canSendBack ? "次の指摘で直させます（自動の差し戻し回数には数えません）" : t.sendBackRefusal ?? undefined} onClick={() => { if (!t.canSendBack) return; close(); setConfirm({ kind: "sendBack", task: t.id, note: "" }); }}>差し戻す…</button> : null}
            {t.canAcceptAsIs ? <button type="button" role="menuitem" title="検証で不合格のまま完了にします" onClick={() => { close(); setConfirm({ kind: "acceptAsIs", task: t.id }); }}>このまま受け入れる…</button> : null}
            {t.canHold || t.held ? <button type="button" role="menuitem" title={t.held ? "保留を解除すると、順番が来れば送られます" : "解除するまで、このタスクを送りません"} onClick={() => { close(); void p.run(t.held ? "保留の解除" : "保留", async () => { await host.orgHoldTask(wf.workflowId, t.id, !t.held); return t.held ? "保留を解除しました。" : "保留にしました（解除するまで送りません）。"; }); }}>{t.held ? "保留を解除" : "保留"}</button> : null}
            <button type="button" role="menuitem" aria-disabled={t.status.phase === "running" && t.chat ? undefined : true} title={t.status.phase === "running" && t.chat ? "このタスクのチャットだけに中断を要求します" : "実行中のタスクだけ止められます"} onClick={() => { if (!(t.status.phase === "running" && t.chat)) return; close(); setConfirm({ kind: "stopTask", task: t.id }); }}>このタスクだけ止める…</button>
            <button type="button" role="menuitem" onClick={() => { close(); setTlTask(t.id); setTab("timeline"); }}>このタスクの経過</button>
          </>
        )}
      </Popover>
      </>}
    </div>
  );

  const groups = groupTasks(wf.tasks);
  const shownNotices = wf.notices.slice(-8).reverse();

  return (
    <>
      {head(wf.title ?? "組織実行",
        <Chip tone={sum.tone} icon={sum.icon} label={sum.label} />)}
      <div className="org-body">
        <div className="org-bar">
          {ro ? <span className="small muted">過去のワークフロー（読み取り専用）</span> : null}
          {canResume && !ro ? <button className="btn primary" disabled={busy} title="必要なチャットを再開し、止めていたタスクの送信を始めます（確認があります）" onClick={openResume}>再開…</button> : null}
          {!ro && !wf.finished ? (
            <Popover role="menu" align="left" triggerClassName="btn" label="止める" title="止め方を選びます（範囲が違います）" trigger={<span>止める…</span>}>
              {(close) => {
                const manageOff = !wf.finished || wf.tasks.some((t) => t.confirm.state !== "confirmed");
                return (
                  <>
                    <button type="button" role="menuitem" disabled={busy || !wf.dispatching} title={wf.dispatching ? "新しいタスクの送信だけを止めます。実行中のタスクは続きます（取消し・失敗にはなりません）" : "すでに新しい送信は止まっています"} onClick={() => { close(); void go("一時停止", async () => { await host.orgPause(wf.workflowId); return "新しい送信を止めました。実行中のタスクは止めていません。"; }); }}>新しい送信だけ止める（実行中は続ける）</button>
                    <button type="button" role="menuitem" onClick={() => { close(); setConfirm({ kind: "stopRunning" }); }} title="新しい送信を止め、実行中のタスクに中断を要求します（取消し・失敗にはしません）">実行中も止める…</button>
                    <button type="button" role="menuitem" disabled={manageOff || !!wf.deletion} title={manageOff ? "アーカイブできるのは、すべて終わって状態が確定したワークフローだけです（実行中のものを一覧から隠すことはできません）" : "一覧から隠します（作業には触れません）"} onClick={() => { close(); void p.run("アーカイブ", async () => { await host.orgArchiveWorkflow(wf.workflowId); return "ワークフローをアーカイブしました（アーカイブ一覧から戻せます）。"; }); }}>アーカイブ</button>
                  </>
                );
              }}
            </Popover>
          ) : null}
          <span className="grow" />
          <Popover role="menu" align="right" triggerClassName="btn subtle icon" label="ワークフローの操作" title="ワークフローの操作" trigger={<Icon name="more" />}>
            {(close) => {
              const manageOff = !wf.finished || wf.tasks.some((t) => t.confirm.state !== "confirmed");
              const offTip = "実行中・停止未確認・通信断のタスクがあるため、まだ削除できません";
              return (
                <>
                  {ro ? null : <>
                    <button type="button" role="menuitem" disabled={wf.finished} title={wf.finished ? "すべて終わっています" : undefined} onClick={() => { close(); setConfirm({ kind: "raise", to: String(lim.maxTurns + 10) }); }}>turn上限を上げる…</button>
                    <button type="button" role="menuitem" onClick={() => { close(); p.onReload(); }}>状態を取り直す</button>
                    <button type="button" role="menuitem" disabled={!wf.finished} title={wf.finished ? undefined : "実行中のワークフローがあります"} onClick={() => { close(); p.onAct("orgStart"); }}>新しいワークフローを開始…</button>
                    <hr />
                    <button type="button" role="menuitem" className="danger" disabled={wf.finished} onClick={() => { close(); setConfirm({ kind: "cancelWorkflow" }); }}>ワークフローを取り消す…</button>
                    <hr />
                  </>}
                  <button type="button" role="menuitem" title="作業フォルダの今のブランチを、確認のうえリモートへ送ります（取り込みの後に使います）" onClick={() => { close(); p.onAct(`gitPush:${wf.repo}`); }}>プッシュ…</button>
                  {wf.archivedAt === null
                    ? <button type="button" role="menuitem" disabled={manageOff || !!wf.deletion} title={manageOff ? offTip : undefined} onClick={() => { close(); void p.run("アーカイブ", async () => { await host.orgArchiveWorkflow(wf.workflowId); return "ワークフローをアーカイブしました（アーカイブ一覧から戻せます）。"; }); }}>アーカイブ</button>
                    : <button type="button" role="menuitem" onClick={() => { close(); void p.run("アーカイブの解除", async () => { await host.orgUnarchiveWorkflow(wf.workflowId); return "アーカイブを解除しました。"; }); }}>アーカイブを解除</button>}
                  <button type="button" role="menuitem" className="danger" disabled={manageOff && !wf.deletion} title={manageOff && !wf.deletion ? offTip : undefined} onClick={() => { close(); p.onDelete(wf.workflowId); }}>{wf.deletion ? "削除を続ける…" : "削除…"}</button>
                </>
              );
            }}
          </Popover>
        </div>

        {confirmBlock()}

        {ro ? (
          <div className="banner info" role="status"><Icon name="help" /><div>保存されている記録を読み取り専用で表示しています。再開・承認・取消しなどの操作はできません（実行中のワークフローがあれば、左の一覧の上の行から開けます）。</div></div>
        ) : null}
        {wf.halt && wf.halt.kind !== "awaitingPlanApproval" && !ro ? (
          <div className={`banner ${wf.halt.kind === "sandboxUnhealthy" || wf.halt.kind === "definitionUnavailable" ? "err" : "warn"}`} role="status">
            <Icon name={orgIcon(wf.halt.icon)} />
            <div>
              <b>{wf.halt.label}</b>
              {wf.halt.kind === "restart" || wf.halt.kind === "needsResume"
                ? <div>再起動前に実行中だったワークフローを照合しました。再開するには「再開」を押してください。再開するまで、新しい送信はしません。</div>
                : wf.halt.kind === "shutdown" ? <div>終了の手順中のため、新しい送信を止めています。</div>
                : wf.halt.kind === "user" ? <div>一時停止中です。「再開」を押すまで、新しい送信はしません。</div>
                : wf.halt.kind === "usageLimit" ? <div>利用上限に達して turn が止まりました。上限が解けてから「再開」し、再送するタスクをチェックしてください。チェックしたタスクだけを新しい試行として送ります（自動では送りません）。</div> : null}
              {wf.halt.resumable ? null : <div className="small muted">この停止は「再開」では解除できません。</div>}
            </div>
          </div>
        ) : null}
        {wf.sandboxNotice ? <div className="banner err" role="alert"><Icon name="alert" /><div>{wf.sandboxNotice}</div></div> : null}
        {wf.trust ? (
          <div className={`banner ${wf.trust.severity === "attention" ? "warn" : "info"}`} role="status">
            <Icon name={wf.trust.severity === "attention" ? "alert" : "help"} />
            <div>
              <b>{wf.trust.severity === "attention" ? "注意: " : "お知らせ: "}</b>{wf.trust.text}
              {wf.trust.projects.length > 0 ? <ul className="check-list">{wf.trust.projects.map((x) => <li key={x} className="mono">{x}</li>)}</ul> : null}
              <div className="small muted">AgentDock は ~/.codex/config.toml を書き換えていません。自動で戻すことも、送信を止めることもしません。</div>
            </div>
          </div>
        ) : null}
        {wf.finished ? (
          <div className="banner info" role="status"><Icon name="branch" /><div>成果物を確認して取り込んでください。AgentDock は自動でマージしません。
            {wf.tasks.some((t) => t.commit) ? <button type="button" className="btn sm" style={{ marginLeft: 8 }} onClick={() => p.onAct(`orgArtifacts:${wf.workflowId}`)}>成果物を見る</button> : null}</div></div>
        ) : null}
        {approvals.count > 0 && approvals.first && !ro ? (
          <div className="banner warn" role="status"><Icon name="alert" /><div>Codex の承認・質問の待ちが {approvals.count} 件あります。
            <button className="btn sm" style={{ marginLeft: 8 }} onClick={() => p.onOpenChat(approvals.first!.key)}>要求へ</button>
            <span className="small muted"> 回答はそのチャットで、あなたが行います。</span></div></div>
        ) : null}

        {wf.planApproval && !ro ? (
          <section className="org-card" aria-label="計画承認">
            <h4><Icon name="eye" />計画の承認待ち</h4>
            <p>タスク {wf.tasks.length} 件（{roleCounts(wf.tasks).map((r) => `${r.role} ${r.n}件`).join("、") || "—"}）。承認するまで、タスクは送られません。</p>
            <p className="small">作業場所: 対象リポジトリ <span className="mono">{wf.repo}</span>。Worker はタスクごとの専用 worktree で作業し、あなたのブランチ・作業ツリーには書き込みません（自動マージもしません）。</p>
            <div>
              <b className="small">自動承認できなかった理由</b>
              {!wf.planApproval.reasonsKnown
                ? <p className="small muted">理由は、再起動で失われたため表示できません（計画が承認待ちである事実だけが分かります）。内容を確認してから承認してください。</p>
                : wf.planApproval.reasons.length === 0
                  ? <p className="small muted">理由は記録されていません。</p>
                  : <ul className="check-list">{wf.planApproval.reasons.map((r, i) => <li key={i}>{r.task ? <b>{r.task}: </b> : null}{r.text}</li>)}</ul>}
            </div>
            <details className="about" open>
              <summary>計画の内容（{wf.tasks.length}件）</summary>
              <ul className="org-plan">
                {wf.tasks.map((t) => (
                  <li key={t.id}><b className="mono">{t.id}</b> <span className="muted">[{t.role}]</span>{t.kind === "investigate" ? <> <span className="tag">調査</span></> : null} {t.title}
                    {t.deps.length ? <span className="small muted">　依存: {t.deps.join(",")}</span> : null}
                    {t.effects.length ? <div className="small">効果の申告（AIの自己申告）: {t.effects.map((e) => EFFECT_TEXT[e]).join("、")}</div> : null}</li>
                ))}
              </ul>
            </details>
            {wf.notesAfterPlan > 0 ? (
              <div className="banner info" role="status"><Icon name="help" /><div>
                計画の作成後に追加した指示が {wf.notesAfterPlan}件あります（計画には反映されていません。各タスクには届きます）。
                <button className="btn sm" style={{ marginLeft: 8 }} disabled={busy} title="追加指示の内容を理由にして、計画を作り直します" onClick={() => setConfirm({ kind: "replan", note: wf.notes.filter((n) => !n.withdrawn && n.pendingFor.length > 0).map((n) => n.text).join("\n\n").slice(0, 2000) })}>この指示を理由に計画を作り直す…</button>
              </div></div>
            ) : null}
            <div className="acts end org-card-acts">
              <button className="btn danger-line" disabled={busy} onClick={() => setConfirm({ kind: "cancelWorkflow" })}>取り消す…</button>
              <button className="btn" disabled={busy} onClick={() => setConfirm({ kind: "replan", note: "" })}>差し戻して再計画…</button>
              <button className="btn primary" disabled={busy} title="承認すると、この計画のタスクが送られ始めます" onClick={() => void go("承認", async () => { await host.orgApprovePlan(wf.workflowId); return "計画を承認しました。"; })}>{busy ? "承認しています…" : "承認"}</button>
            </div>
          </section>
        ) : null}

        <section className="org-sum" aria-label="概要">
          <p className="org-req" title={wf.request}>{wf.request}</p>
          <dl className="org-dl">
            <dt>リポジトリ</dt><dd className="mono">{wf.repo}</dd>
            <dt>タスク</dt><dd>完了 {doneN} / {wf.tasks.length}{sum.attention > 0 ? `　要対応 ${sum.attention}件` : ""}</dd>
            <dt>並列数</dt><dd>Worker 同時 {lim.maxWriters}・全体 {lim.maxTotal}</dd>
            <dt>turn</dt><dd>使用 {use.turnsUsed} / 上限 {lim.maxTurns}（残り {use.turnsRemaining}）</dd>
            <dt>再計画</dt><dd>自動 {use.autoReplansUsed} / {lim.maxAutoReplans} 回、差し戻しはタスクごとに最大 {lim.maxRework} 回</dd>
            <dt>トークン</dt>
            <dd>{use.knownTokens === null ? "未取得" : `${use.knownTokens.toLocaleString()}（取得できた分の合計）`}{use.rowsWithoutTokens > 0 ? `　取得できなかった行 ${use.rowsWithoutTokens}/${use.rows}` : ""}</dd>
          </dl>
        </section>

        {wf.graph ? <GraphSection graph={wf.graph} wf={wf} ro={ro} busy={busy} chatExists={chatExists} onOpenChat={p.onOpenChat} onGate={(n, c) => setConfirm({ kind: "gate", task: n.task!, node: n.label, choice: c.id, label: c.label, needsNote: c.needsNote, note: "" })} onExtra={(n) => setConfirm({ kind: "extraRound", task: n.task!, node: n.label, note: "" })} onAccept={(n) => setConfirm({ kind: "acceptRound", task: n.task!, node: n.label })} /> : null}

        <section className="org-wait-band" aria-label="待ち・作業中・完了">
          {(() => {
            const b = waitBand(snap, wf);
            const detail = waitingDetail(b.waiting);
            return (
              <>
                <button type="button" className={`btn${b.waiting.total > 0 ? " attn" : ""}`} disabled={!b.waiting.firstId} title={b.waiting.firstId ? "対応が要る最初のタスクの行へ移動します" : "あなたの対応待ちのタスクはありません"} onClick={() => jumpTo(b.waiting.firstId)}>あなたの対応待ち {b.waiting.total}{detail ? `（${detail}）` : ""}</button>
                <button type="button" className="btn" disabled={!b.working.firstId} title={b.working.firstId ? "作業中の最初のタスクの行へ移動します" : "作業中のタスクはありません"} onClick={() => jumpTo(b.working.firstId)}>作業中 {b.working.total}</button>
                <button type="button" className="btn" disabled={b.done.done === 0} title={b.done.done > 0 ? "完了したタスクの行へ移動します" : "完了したタスクはまだありません"} onClick={() => jumpTo(wf.tasks.find((t) => t.status.phase === "completed")?.id ?? null)}>完了 {b.done.done}／全 {b.done.all}</button>
              </>
            );
          })()}
        </section>

        <section className="org-activity" aria-label="いまの動き">
          <h4 className="org-h">いまの動き</h4>
          {actLines.length === 0 ? <div className="small muted">動いているタスクはありません{throttledAct.done > 0 ? `（完了 ${throttledAct.done}件）` : ""}</div> : (
            <ul className="org-act-list">
              {actLines.map((l) => (
                <li key={l.task} className={`org-act ${l.group}`} title={lineText(l, nowTick)}>
                  <button type="button" className="org-act-main" onClick={() => jumpTo(l.task)}>
                    <b>{l.roleLabel}・{l.task}『{l.title}』</b>
                    <span className="org-act-state">{l.state}</span>
                    <span className="org-act-detail">{l.detail}</span>
                    {l.since !== null ? <span className="small muted">{elapsedText(nowTick - l.since)}</span> : null}
                    {l.stale ? <span className="mark wait" title="接続が切れているなど、この活動は最新ではない可能性があります（状態の文字は変えていません）">最新ではありません（接続断等）</span> : null}
                  </button>
                  {l.chat && chatExists(l.chat) ? <button type="button" className="btn sm subtle" title="このタスクのチャットを開きます（見るだけ。何も送りません）" onClick={() => p.onOpenChat(l.chat!)}>チャット</button> : null}
                </li>
              ))}
            </ul>
          )}
          {throttledAct.done > 0 && actLines.length > 0 ? <div className="small muted">完了 {throttledAct.done}件</div> : null}
        </section>

        <div className="org-tabs" role="tablist" aria-label="タスクと経過">
          <button type="button" role="tab" aria-selected={tab === "tasks"} className={`btn sm${tab === "tasks" ? " primary" : ""}`} onClick={() => setTab("tasks")}>タスク</button>
          <button type="button" role="tab" aria-selected={tab === "timeline"} className={`btn sm${tab === "timeline" ? " primary" : ""}`} onClick={() => setTab("timeline")}>経過</button>
        </div>
        {tab === "timeline" ? <OrgTimeline workflowId={wf.workflowId} tick={p.timelineTick} live={p.live} task={tlTask} onClearTask={() => setTlTask(null)} /> : null}

        <section aria-label="タスク" hidden={tab !== "tasks"}>
          <h4 className="org-h">タスク</h4>
          {groups.length === 0 ? <div className="empty-note">タスクはまだありません</div> : groups.map((g) => (
            <div key={g.key} className="org-group">
              <div className="org-gh">{g.title}（{g.tasks.length}）</div>
              <div className="org-scroll">
                <table className="org-tbl">
                  <thead><tr><th scope="col">ID</th><th scope="col">役割</th><th scope="col">タスク</th><th scope="col">業務状態</th><th scope="col">確認状態</th><th scope="col">待ち先</th><th scope="col" title="Reviewer の差し戻しによる試行の回数（1 + 差し戻し回数）と上限">差し戻し</th><th scope="col" title="あなたが押した「新しい試行で再送」の回数と上限">再送</th><th scope="col">成果物</th><th scope="col">操作</th></tr></thead>
                  <tbody>
                    {g.tasks.map((t) => {
                      const w = taskRequestWait(snap, t);
                      const waitMsg = requestWaitText(w);
                      return (
                      <Fragment key={t.id}>
                        <tr id={taskRowId(t.id)} tabIndex={-1}>
                          <th scope="row" className="mono">{t.id}</th>
                          <td>{t.role}{t.kind === "investigate" ? <> <span className="tag" title="ファイルを変えない調査のタスク。結果は報告（成果物の「調査報告」）に出ます">調査</span></> : null}</td>
                          <td className="org-title">{t.title}</td>
                          <td><StatusChip s={t.status} /></td>
                          <td><ConfirmChip c={t.confirm} /></td>
                          <td>{waitMsg ? <b>{waitMsg}</b> : waitText(t)}</td>
                          <td>{attemptText(t)}</td>
                          <td>{retryText(t, wf.limits.maxRetries)}</td>
                          <td>{t.commit ? <button type="button" className="linkish" title="成果物の中身を開きます（見るだけ。何も書きません）" onClick={() => p.onAct(`orgArtifacts:${wf.workflowId}:${t.id}`)}>{artifactText(t)}</button> : artifactText(t)}</td>
                          <td>{taskActions(t)}</td>
                        </tr>
                        {t.reconcile || t.stallNotified || t.needsChatChoice || t.attentionNote || w.pending.length > 0 || w.gaps.length > 0 ? (
                          <tr key={`${t.id}:n`} className="org-note-row"><td /><td colSpan={9} className="small">
                            {w.pending.map((r) => <div key={`${r.key.source}/${r.key.requestId}`} className="org-req-card">{p.renderRequest(r, agentName)}</div>)}
                            {w.gaps.map((r) => <RequestGapLine key={`gap:${r.key.source}/${r.key.requestId}`} r={r} guide="org" />)}
                            {t.attentionNote ? <div>止まった理由: {t.attentionNote}</div> : null}
                            {t.stallNotified ? <div>実行中のまま新しい出力がないと通知済みです（自動では止めません）。</div> : null}
                            {t.needsChatChoice ? <div>チャットの作成結果を確認できていません。「チャットを選ぶ」で結び付けるまで進めません。</div> : null}
                            {t.reconcile ? <div>照合（読取りのみ）: {t.reconcile.text}{t.reconcile.worktree.label ? `　作業場所: ${t.reconcile.worktree.label}` : ""}</div> : null}
                          </td></tr>
                        ) : null}
                      </Fragment>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          ))}
        </section>

        <ArtifactsSection wf={wf} live={p.live} onAct={p.onAct} />

        {shownNotices.length > 0 ? (
          <details className="about">
            <summary>経過（直近 {shownNotices.length} 件）</summary>
            <ul className="org-notices">
              {shownNotices.map((n) => (
                <li key={n.serial}><span className={`mark ${n.severity === "attention" ? "wait" : "pend"}`}>{n.severity === "attention" ? "要確認" : "情報"}</span> {n.task ? <b>{n.task}: </b> : null}{n.text}</li>
              ))}
            </ul>
          </details>
        ) : null}
        {ro ? null : <WorkflowNotes wf={wf} run={p.run} />}
      </div>
    </>
  );
}

/** グラフで動くワークフローの流れ（一覧形式・読み取り専用）。ノードは先へ進む順。判断（gate）と周回上限のカードは、確認を経る操作だけを呼ぶ。 */
function GraphSection({ graph, wf, ro, busy, chatExists, onOpenChat, onGate, onExtra, onAccept }: {
  graph: OrgGraphView; wf: OrgWorkflowView; ro: boolean; busy: boolean;
  chatExists: (k: ChatKey | null) => boolean; onOpenChat: (k: ChatKey) => void;
  onGate: (n: OrgGraphNodeView, c: { id: string; label: string; needsNote: boolean }) => void;
  onExtra: (n: OrgGraphNodeView) => void;
  onAccept: (n: OrgGraphNodeView) => void;
}) {
  const taskOf = (n: OrgGraphNodeView): OrgTaskView | undefined => wf.tasks.find((t) => t.id === n.task);
  const nameOf = (id: string): string => graph.nodes.find((x) => x.id === id)?.label ?? id;
  const cards = graph.nodes.filter((n) => (n.gate && n.status?.phase === "blocked") || n.roundLimit);
  return (
    <>
      {!ro ? cards.map((n) => {
        if (n.roundLimit) {
          const r = n.roundLimit;
          return (
            <section key={`${n.id}:limit`} className="org-card" aria-label="やり直しの上限">
              <h4><Icon name="alert" />やり直しが上限に達しました（{n.label}）</h4>
              <p title={`辺 ${r.edge}`}>工程「{n.label}」へのやり直しは、最大 {r.allowed} 回のうち {r.used} 回を使い切りました（{reworkLimitText(r.used, r.allowed)}）。理由を書いてもう1周するか、この結果で受け入れるか、タスクを取り消すかを選んでください。何もしなければ、このまま止まっています（自動では進みません）。</p>
              {r.awaiting
                ? <div className="acts end org-card-acts"><button className="btn" disabled={busy} onClick={() => onAccept(n)}>この結果で受け入れる…</button><button className="btn primary" disabled={busy} onClick={() => onExtra(n)}>理由を書いてもう1周…</button></div>
                : <p className="small muted">確認状態が確定ではないため、確認できるまで選べません（下の表の確認状態を見てください）。</p>}
            </section>
          );
        }
        const g = n.gate!;
        return (
          <section key={`${n.id}:gate`} className="org-card" aria-label="判断待ち">
            <h4><Icon name="help" />判断をお願いします（{n.label}）</h4>
            <p style={{ whiteSpace: "pre-wrap" }}>{g.prompt}</p>
            {g.shows.map((x, i) => (
              <details key={x.slot} className="about" open>
                <summary title={x.slot}>見せる内容{g.shows.length > 1 ? `（${i + 1}）` : ""}</summary>
                {x.text === null ? <p className="small muted">未取得（まだ出力がないか、読めませんでした）</p> : <pre className="mono small org-pre">{x.text}{x.truncated ? "\n…（長いため途中まで）" : ""}</pre>}
              </details>
            ))}
            {g.awaiting
              ? <div className="acts end org-card-acts">{g.choices.map((c) => <button key={c.id} className="btn" disabled={busy} onClick={() => onGate(n, c)}>{c.label}{c.needsNote ? "（理由を書く）" : ""}…</button>)}</div>
              : <p className="small muted">いまは選べません（確認状態が確定ではないか、判断を受け付けていない状態です）。</p>}
          </section>
        );
      }) : null}
      <section aria-label="流れ">
        <h4 className="org-h">流れ（{graph.name}）</h4>
        <ol className="org-flow">
          {graph.nodes.map((n) => {
            const t = taskOf(n);
            const rt = roundText(n);
            const out = graph.edges.filter((e) => e.from === n.id);
            const chat = t?.chat ?? null;
            return (
              <li key={n.id} className="org-flow-n">
                <div className="org-flow-h">
                  <b title={n.id}>{n.label}</b>
                  <span className="small muted" title={n.roleId ?? undefined}>{NODE_KIND_TEXT[n.kind]}{n.roleId ? `：${roleLabel(n.roleId)}` : ""}</span>
                  {n.status ? <StatusChip s={n.status} /> : <Chip tone="idle" icon="circle" label="未実行" />}
                  {n.confirm && n.confirm.state !== "confirmed" ? <ConfirmChip c={n.confirm} /> : null}
                  {rt ? <span className="small">{rt}</span> : null}
                  {n.lastVerdict ? <span className="small">判定: <b>{verdictText(n.lastVerdict)}</b></span> : null}
                  {chat ? <button className="btn sm" disabled={!chatExists(chat)} title={chatExists(chat) ? "この工程のチャットを開きます（見るだけ。何も送りません）" : "チャットがまだ一覧にありません"} onClick={() => onOpenChat(chat)}>チャットを見る</button> : null}
                </div>
                {out.length > 0 ? (
                  <ul className="org-flow-e">
                    {out.map((e) => {
                      const x = edgeText(e, nameOf);
                      return <li key={`${e.from}->${e.to}`} className={x.untaken ? "untaken" : undefined} title={x.title}><span>{x.head}</span>　{x.detail}{x.untaken ? "　（未実行）" : ""}</li>;
                    })}
                  </ul>
                ) : <div className="small muted org-flow-e">この先はありません（ここで終わります）</div>}
              </li>
            );
          })}
        </ol>
        {graph.rounds.length > 0 ? (
          <div className="org-rounds">
            <b className="small">周回の履歴</b>
            <ul className="org-flow-e">
              {graph.rounds.map((r) => (
                <li key={r.edge}>
                  <span title={r.edge}>{nameOf(r.from)} ↺ {nameOf(r.to)}</span>　やり直し 使用 {r.used}／最大 {r.maxRounds + r.extra} 回{r.extra > 0 ? `（あなたが ${r.extra} 回足しました）` : ""}。上限に達したら{r.onExhausted === "ask" ? "あなたに聞きます" : "戻り先を失敗にします"}。
                  <div className="small">{r.history.length === 0 ? "判定: まだありません" : r.history.map((h) => `${h.index}回目の判定: ${verdictText(h.verdict)}`).join("／")}</div>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
      </section>
    </>
  );
}

/** ワークフローへの追加指示（DESIGN_ORG_R2 2.2）。実行中の turn には届かず、次にエンジンが送る turn に、タスクごと1回だけ添える。 */
function WorkflowNotes({ wf, run }: { wf: OrgWorkflowView; run: OrgPaneProps["run"] }) {
  // 依頼整理のチャットから写された本文があれば入れておく（写すだけ。送るのは「追加する」）。
  const [text, setText] = useState(() => takeNoteDraft());
  const [busy, setBusy] = useState(false);
  const live = wf.tasks.filter((t) => !["completed", "failed", "cancelled"].includes(t.status.phase));
  const running = live.filter((t) => t.status.phase === "running");
  const next = live.filter((t) => t.status.phase !== "running");
  const notes = wf.notes.filter((n) => !n.withdrawn);
  const open = notes.filter((n) => n.pendingFor.length > 0);
  const blank = text.trim() === "";
  const add = async () => {
    if (blank || busy) return;
    setBusy(true);
    const ok = await run("追加指示", async () => { await host.orgAddNote(wf.workflowId, text.trim(), null); return `追加指示を追加しました。${NEXT_TURN_NOTE}（実行中のタスクには届きません）`; });
    setBusy(false);
    if (ok) setText("");
  };
  return (
    <section className="org-notes" aria-label="ワークフローへの追加指示">
      <h4 className="org-h">ワークフローへの追加指示</h4>
      {open.length > 0 ? (
        <ul className="org-note-list">
          {open.map((n) => (
            <li key={n.id}>
              <span className="org-note-text">{n.text}</span>
              <span className="small muted">　{n.task ? `${n.task} へ` : "全体へ"}。未達: {n.pendingFor.join(",")}{n.deliveredTo.length > 0 ? `　届いた: ${n.deliveredTo.join(",")}（届いた分は戻せません）` : ""}</span>
              <button className="btn sm" onClick={() => void run("追加指示の取り消し", async () => { await host.orgWithdrawNote(wf.workflowId, n.id); return "取り消しました。以後は添えません（すでに届いた分は戻せません）。"; })}>取り消す</button>
            </li>
          ))}
        </ul>
      ) : <div className="small muted">まだ届いていない追加指示はありません。</div>}
      <textarea rows={2} style={{ width: "100%" }} value={text} maxLength={4200} disabled={busy} aria-label="追加指示の内容" placeholder="例: コメントは日本語で書いてください" onChange={(e) => setText(e.target.value)} />
      <div className="small muted">
        {NEXT_TURN_NOTE}{" "}
        {next.length > 0 ? <>次に送るタスクへ添えます: {next.map((t) => t.id).join(", ")}（これから計画されるタスクにも添えます）。</> : <>これから送るタスクがあれば添えます。</>}
        {running.length > 0 ? <>実行中の {running.map((t) => t.id).join(", ")} には届きません（届けるなら、そのタスクの「指示を挟む」を使ってください）。</> : null}
        {wf.halt?.kind === "awaitingPlanApproval" ? " 計画に反映するなら、Planner への割り込み指示か「計画を作り直す」を使ってください。" : ""}
      </div>
      <div className="acts"><span className="grow" /><button className="btn" disabled={blank || busy || text.trim().length > 4000} onClick={() => void add()}>{busy ? "追加しています…" : "追加する"}</button></div>
    </section>
  );
}

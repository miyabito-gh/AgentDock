// 組織実行の周回・判定・辺の文言（純粋関数。node --test で単体テストできるよう org.ts から分けている）。
import type { ChatKey, OrgGraphEdgeView, OrgGraphNodeView, OrgStopAction, OrgStopEvidence, OrgStopKind, OrgStopView, OrgVerdict } from "../ipc/types";

/** 判定値の日本語訳。未知の値は元の値をそのまま出す（推測で訳さない）。ホスト側の辺の条件の訳（view.rs の verdict_ja）と同じ対応。 */
const VERDICT_JA: Record<string, string> = { rework: "やり直し", pass: "合格", replan: "計画からやり直し" };
export const verdictJa = (v: string): string => VERDICT_JA[v] ?? v;

/** 判定の文言。出力がまだなければ「—」、値を読めなければ「未取得」（空や既定値にしない）。値は訳して出す。 */
export const verdictText = (v: OrgVerdict | null | undefined): string => (!v ? "" : v.kind === "pending" ? "—" : v.kind === "notObtained" ? "未取得" : verdictJa(v.text));

/**
 * 周回の回数の文言。単位は「実行」（最初の実行を含む）と「やり直し」（差し戻して戻る回数）に分ける。
 * 実行 = やり直し + 1。`used` は使ったやり直し、`allowed` はやり直しの許容回数（利用者が足した分を含む）。
 */
export function reworkLimitText(used: number, allowed: number): string {
  const left = Math.max(0, allowed - used);
  return `実行 ${used + 1}回目／最大 ${allowed + 1}回（やり直しは最大${allowed}回、残り${left}回）`;
}

/** 周回の中の工程の回数文言。周回の外・まだ動いていない工程は null。`round`＝実行回数、`maxRound`＝最大の実行回数。 */
export const roundText = (n: OrgGraphNodeView): string | null => (n.round !== null && n.maxRound !== null ? reworkLimitText(n.round - 1, n.maxRound - 1) : null);

/** 辺の一行。通っていない辺は「未実行」を付ける（薄く表示する側の目印）。`nameOf` は工程IDから表示名（ないときはIDのまま）。 */
export function edgeText(e: OrgGraphEdgeView, nameOf: (id: string) => string = (id) => id): { head: string; detail: string; untaken: boolean; title: string } {
  const title = `${e.from}->${e.to}`;
  if (e.kind === "feedback") {
    const allowed = e.allowed;
    return { head: `↺ ${nameOf(e.to)}`, detail: allowed == null ? `${e.label}、やり直し 上限は確認できません・使用${e.used}回` : `${e.label}、やり直し 最大${allowed}回・使用${e.used}回・残り${Math.max(0, allowed - e.used)}回`, untaken: e.used === 0, title };
  }
  return { head: `→ ${nameOf(e.to)}`, detail: e.label, untaken: e.used === 0, title };
}

/** 「差し戻し n/m」の列: 差し戻しの回数／上限。ホストの試行数・上限は初回を含む（1 + 差し戻し）ので、1を引いて出す。 */
export function reworkCellText(attempts: number, maxAttempts: number): string {
  return `${Math.max(0, attempts - 1)}/${Math.max(0, maxAttempts - 1)}`;
}
export const REWORK_COLUMN_TIP = "Reviewer の差し戻しの回数／上限です（初回の実行は数えません。0/2 は差し戻しなし）";

/** 「Workerが変更を書きませんでした」の停止理由に報告の要旨が付いていないとき、その旨の1行。付いている・別の理由なら null。 */
export function noReportHeadNote(attentionNote: string | null | undefined): string | null {
  if (!attentionNote || !attentionNote.includes("変更を書きませんでした")) return null;
  return attentionNote.includes("報告の要旨") ? null : "報告の要旨は記録されていません";
}

/**
 * 調査だけのワークフロー。調査タスクがあり、コミット（取り込める変更）が1つも無い。
 * 役割（Reviewer 等）のタスクの kind は調査と限らないので、全タスクが調査かどうかでは判定しない。
 * 変更タスクがあってもコミット無しで終わったときも、取り込むものは無い（調査報告があるときだけ案内を切り替える）。
 */
export const isInvestigationOnly = (tasks: ReadonlyArray<{ kind: string; commit?: string | null }>): boolean =>
  tasks.some((t) => t.kind === "investigate") && !tasks.some((t) => t.commit);

const INVESTIGATION_DONE = "調査報告を確認してください。取り込む変更はありません（AgentDock は何もマージしません）。";

/** 完了後の帯の文。 */
export const finishedBannerText = (tasks: ReadonlyArray<{ kind: string; commit?: string | null }>): string =>
  isInvestigationOnly(tasks) ? INVESTIGATION_DONE : "成果物を確認して取り込んでください。AgentDock は自動でマージしません。";

/** 完了通知（お知らせ一覧）の文。ホストの文言（view.rs）と同じ前半。 */
export const completedNoticeText = (tasks: ReadonlyArray<{ kind: string; commit?: string | null }>): string =>
  isInvestigationOnly(tasks) ? `ワークフローが完了しました。${INVESTIGATION_DONE}` : "ワークフローが完了しました。成果物を確認して取り込んでください（AgentDock は自動でマージしません）";

// ── 画面の整形（表示時だけ。保存名・Plannerへ送る本文は変えない） ──

/** ワークフロー名の表示。先頭の Markdown の見出し記号（`# `）だけを除く。名前が無ければ null。 */
export function displayWorkflowTitle(title: string | null | undefined): string | null {
  if (title === null || title === undefined) return null;
  const s = title.replace(/^\s*#{1,6}\s+/, "").trim();
  return s === "" ? null : s;
}

/** Planner への依頼文の前置き（rules/org_intake.rs の PLANNER_REQUEST_NOTE と同じ文）。画面には出さない。 */
export const PLANNER_REQUEST_NOTE = "以下は依頼整理を終えた確定依頼である。依頼整理の段階の制約（ファイルを変更しない・組織実行を開始しない 等）はこの依頼には適用しない。";
const NOTE_HEAD = "以下は依頼整理を終えた確定依頼である。";

/** 概要に出す依頼文。前置きの行（定数との一致、または先頭一致）と、先頭の見出し記号を除く。残りが空なら元の文のまま（隠さない）。 */
export function displayRequest(request: string): string {
  const lines = request.split(/\r?\n/).filter((l) => { const t = l.trim(); return t !== PLANNER_REQUEST_NOTE && !t.startsWith(NOTE_HEAD); });
  const s = lines.join("\n").replace(/^\s*#{1,6}\s+/, "").replace(/\n{3,}/g, "\n\n").trim();
  return s === "" ? request : s;
}

/** 状態に応じた出し分け。終了済み（完了・取り消し済み・失敗）では、動きの節・0件のチップ・設定値の表を前面に出さない。 */
export function sectionVisibility(finished: boolean): { activity: boolean; waitBand: boolean; settingsFolded: boolean } {
  return { activity: !finished, waitBand: !finished, settingsFolded: finished };
}

/**
 * 行の「…」メニューの「手動に切り替え…」。終了したワークフローでも出す（ホストは終了後も拒否しない）。
 * 出すのは、チャットのあるタスクだけ。過去の記録（読み取り専用）では操作できないので、無効にして理由を添える。
 */
export function releaseMenuState(hasChat: boolean, readonly: boolean): { show: boolean; disabled: boolean; reason: string | null } {
  if (!hasChat) return { show: false, disabled: false, reason: null };
  return readonly
    ? { show: true, disabled: true, reason: "過去の記録（読み取り専用）のため、ここからは切り替えられません" }
    : { show: true, disabled: false, reason: null };
}

/** 組織実行のチャットへ手入力できない理由。手動に切り替えられるときだけ、その手順を案内する（使えないときは事実だけ）。 */
export function orgSendLock(releasable: boolean): string {
  const base = "組織実行のチャットです。ここから通常の送信はできません（やりとりを始めるのは組織実行だけです）。中央の入力欄は「割り込み指示」で、実行中のやりとりに届けるか、次の送信に添えます。";
  return releasable ? `${base}通常のチャットとして送るには、ワークフロー画面のタスクの「…」メニューから「手動に切り替え…」を選んでください。` : "このチャットは見るだけです（過去のワークフローの記録のため、ここからは送れません）。";
}

/** タスク行の「成果物」の補足。コミットがあれば変更あり、調査の報告があれば「報告あり」。取れていないものは「なし」と書かない（null＝出さない／未取得は文字で）。 */
export function taskArtifactNote(t: { kind: string; commit?: string | null; phase: string }, report: "value" | "notObtained" | "none" | "unknown"): string | null {
  if (t.commit) return `変更あり（${t.commit.slice(0, 8)}）`;
  if (t.kind !== "investigate") return null;
  if (report === "value") return "報告あり";
  if (report === "notObtained") return t.phase === "completed" ? "報告 未取得" : null;
  return null;
}

/** タスク行の状態の下の補足（値があるものだけ）。差し戻しは0回なら出さない。 */
export function taskSubparts(x: { reworkUsed: number; reworkMax: number; retriesUsed: number; retryMax: number | null; hasRetry: boolean; waitingOn: string[]; artifact: string | null }): string[] {
  const out: string[] = [];
  if (x.reworkUsed > 0) out.push(`差し戻し ${x.reworkUsed}/${x.reworkMax}`);
  if (x.retriesUsed > 0 || x.hasRetry) out.push(`再送 ${x.retriesUsed}/${x.retryMax ?? "—"}`);
  if (x.waitingOn.length > 0) out.push(`待ち: ${x.waitingOn.join(",")}`);
  if (x.artifact) out.push(x.artifact);
  return out;
}

/**
 * 結果として先頭に出す報告のタスク。統合タスクが1件に定まるときだけ:
 * 他の調査タスクの依存先になっていない末端の調査がちょうど1件で、それが別の調査タスクを統合している（依存先に調査がある）こと。
 * 並列の調査（統合なし）や末端が複数のときは null（全報告を並べる）。末端の報告を読めなければ null。
 */
export function finalReportTask(tasks: ReadonlyArray<{ id: string; kind: string; deps: string[] }>, withReport: ReadonlySet<string>): string | null {
  const inv = tasks.filter((t) => t.kind === "investigate");
  const depended = new Set(inv.flatMap((t) => t.deps));
  const leaves = inv.filter((t) => !depended.has(t.id));
  if (leaves.length !== 1) return null;
  const leaf = leaves[0];
  const ids = new Set(inv.map((t) => t.id));
  if (!leaf.deps.some((d) => ids.has(d)) || !withReport.has(leaf.id)) return null;
  return leaf.id;
}

/** 完了後の帯の文（過去の記録では取り込み操作を促さない。調査のみは調査用の文）。 */
export function finishedBannerFor(tasks: ReadonlyArray<{ kind: string; commit?: string | null }>, readonly: boolean): string {
  if (isInvestigationOnly(tasks)) return "調査が終わりました。結果は下で読めます。取り込む変更はありません。";
  if (readonly) return "終了した記録です。成果物と取り込みの記録は下で確認できます。";
  return "成果物を確認して取り込んでください。AgentDock は自動でマージしません。";
}

/**
 * 完了後のタスク表から外す計画タスク（planner）と、概要の「計画: 承認済み…」の1行。
 * 外すのは、ワークフローが終了済みで、planner 以外のタスクもあり、完了した planner だけ（実行中・停止中・失敗は従来どおり表に残す）。
 * 自動承認か利用者承認かは、通知の種類から取れるときだけ（最後に起きたほう）。取れなければ「計画: 承認済み」。
 */
export function planSummary(
  tasks: ReadonlyArray<{ id: string; role: string; chat: ChatKey | null; status: { phase: string } }>,
  finished: boolean,
  noticeKinds: ReadonlyArray<string>,
): { hiddenIds: ReadonlySet<string>; line: string | null; chat: ChatKey | null } {
  const none = { hiddenIds: new Set<string>(), line: null, chat: null };
  if (!finished || !tasks.some((t) => t.role !== "planner")) return none;
  const hidden = tasks.filter((t) => t.role === "planner" && t.status.phase === "completed");
  if (hidden.length === 0) return none;
  let how = "";
  for (const k of noticeKinds) if (k === "planAutoApproved") how = "（自動）"; else if (k === "planNeedsApproval") how = "（利用者）";
  return { hiddenIds: new Set(hidden.map((t) => t.id)), line: `計画: 承認済み${how}`, chat: [...hidden].reverse().find((t) => t.chat)?.chat ?? null };
}

// ── 「止まっています」カード（純粋関数）。理由・手がかり・操作は、ホストが返す stop（OrgTaskView.stop）から作る ──

/** カードに要るタスクの最小の形（OrgTaskView の一部）。 */
export interface StopTaskLike {
  id: string;
  title: string;
  stop: OrgStopView | null;
  chat: unknown | null;
  sendBackRefusal: string | null;
}
export type StopButtonKey = OrgStopAction | "release" | "answer";
export interface StopButton { key: StopButtonKey; label: string; enabled: boolean; title: string }
export interface StopCard {
  /** 先頭のタスク（承認・質問の待ちだけのときは null）。 */
  task: string | null;
  kind: OrgStopKind | "approval";
  /** 確認状態が確定でない（「止まった」ではなく事実どおりに書く）。 */
  unconfirmed: boolean;
  heading: string;
  /** 事実の行（先頭にタスクの名前）。 */
  facts: string[];
  evidence: OrgStopEvidence[];
  hint: string;
  buttons: StopButton[];
  /** ほかに止まっているもの・待ちは何件あるか。 */
  others: number;
}

const BUTTON_TEXT: Record<StopButtonKey, { label: string; title: string }> = {
  sendBack: { label: "もう一度直させる…", title: "理由を書いて、もう1周だけ直させます（確認があります）" },
  acceptAsIs: { label: "このまま受け入れる…", title: "いまの結果のまま完了にします（確認があります）" },
  fail: { label: "失敗として確定…", title: "あなたの判断で失敗として確定します（元に戻せません。確認があります）" },
  retryNew: { label: "新しい試行で再送…", title: "前のやりとりの続きを、同じチャットへ新しい試行として1回送ります（確認があります）" },
  retryUnsent: { label: "やり直す…", title: "まだ送られていないことを確認できているタスクを、もう一度送る状態に戻します" },
  cancel: { label: "取り消す…", title: "このタスクを取り消します（確認があります）" },
  adoptManual: { label: "結果を取り込む…", title: "手動に切り替えたチャットの最新の回答を、このタスクの結果として取り込みます（チャットへは何も送りません）" },
  acceptManual: { label: "手動で完了にする…", title: "このタスクを手動で完了にします（確認があります）" },
  chooseChat: { label: "チャットを選ぶ…", title: "作成結果を確認できていないチャットを結び付けます" },
  openChat: { label: "チャットを見る", title: "このタスクのチャットを開きます（見るだけ。何も送りません）" },
  history: { label: "経過を見る", title: "このタスクの経過（記録）を開きます" },
  release: { label: "手動に切り替え…", title: "組織実行から切り離し、手入力できる通常のチャットにします（元に戻す操作はありません）" },
  answer: { label: "承認に答える", title: "承認・質問のカードへ移動します（答えるのはあなたです）" },
};

/** 止まったタスクの見出し。確定でない状態は「止まっています」と書かず、事実の見出しをそのまま使う。 */
export const stopHeading = (s: OrgStopView): string => (s.unconfirmed ? s.headline : `止まっています: ${s.headline}`);

/** タスク行の補足1行（見出しの短縮）。通知の理由があれば添える（長いものは切る）。 */
export function stopRowNote(s: OrgStopView, attentionNote: string | null): string {
  const head = s.unconfirmed ? s.headline : `止まった理由: ${s.headline}`;
  if (!attentionNote) return head;
  const flat = attentionNote.replace(/\s+/g, " ").trim();
  return `${head}（${flat.length > 120 ? `${flat.slice(0, 120)}…` : flat}）`;
}

/**
 * 止まっているタスクがあれば、先頭1件のカードを作る（複数は残り件数）。承認・質問の待ち（approvals 件）はタスクの停止の後ろに並ぶ。
 * `canRelease` は「手動に切り替え…」を操作として出せるタスクか（行の「…」メニューと同じ判定）。
 */
export function stopCard(tasks: StopTaskLike[], finished: boolean, approvals: number, canRelease: (t: StopTaskLike) => boolean): StopCard | null {
  if (finished) return null;
  // 操作できる止まりを先頭に、待ち先が止まっているだけのもの（経過を見る以外に操作がない）は後ろへ。同順位は元の並び。
  const rank = (t: StopTaskLike): number => (t.stop!.kind === "dependencyBlocked" ? 2 : t.stop!.actions.some((a) => a !== "openChat" && a !== "history" && a !== "cancel") ? 0 : 1);
  const hit = tasks.filter((t) => t.stop !== null).map((t, i) => ({ t, i })).sort((a, b) => rank(a.t) - rank(b.t) || a.i - b.i).map((x) => x.t);
  const t = hit[0];
  if (!t || !t.stop) {
    if (approvals <= 0) return null;
    return {
      task: null, kind: "approval", unconfirmed: false,
      heading: "承認・質問の返事を待っています",
      facts: [`返事待ち ${approvals}件`],
      evidence: [],
      hint: "承認・質問のカードで、許可・拒否や回答を選んでください（答えるのはあなたです。代わりには答えません）。",
      buttons: [{ key: "answer", ...BUTTON_TEXT.answer, enabled: true }],
      others: 0,
    };
  }
  const s = t.stop;
  const has = (a: OrgStopAction): boolean => s.actions.includes(a);
  const keys: StopButtonKey[] = [];
  // 差し戻しの上限・テスト失敗は、3択を常に並べる（選べないものは理由つきで無効）
  if (s.kind === "reworkLimit" || s.kind === "checkFailed") keys.push("sendBack", "acceptAsIs", "fail");
  for (const a of s.actions) if (!keys.includes(a) && a !== "openChat" && a !== "history" && a !== "cancel") keys.push(a);
  if (canRelease(t) && !s.unconfirmed) keys.push("release");
  if (has("cancel")) keys.push("cancel");
  if (has("openChat")) keys.push("openChat");
  keys.push("history");
  const buttons: StopButton[] = keys.map((key) => {
    const enabled = key === "release" ? true : has(key as OrgStopAction);
    const base = BUTTON_TEXT[key];
    const refused = key === "sendBack" && !enabled ? t.sendBackRefusal ?? "いまは選べません" : null;
    return { key, label: base.label, enabled, title: enabled ? base.title : refused ?? "いまは選べません（確認状態が確定ではないなど）" };
  });
  return {
    task: t.id,
    kind: s.kind,
    unconfirmed: s.unconfirmed,
    heading: stopHeading(s),
    facts: [`${t.id}『${t.title}』`, ...s.facts],
    evidence: s.evidence.slice(0, 3),
    hint: s.hint,
    buttons,
    others: hit.length - 1 + (approvals > 0 ? 1 : 0),
  };
}

// ドックの「タスク」表示の純粋ロジック（DESIGN_IMPROVE §3・§4）。状態は書き換えず、並びと絞り込みだけを決める。
// node --test で直接読めるよう、型だけを import する（他の ui ファイルへ依存しない）。
import type { AgentView, Chat, HostSnapshot, PendingRequest } from "../ipc/types";

/** タスク列の最大件数（超えた分は「ほかのタスク」へ1行で出す）。 */
export const TASK_LIMIT = 5;
/** 子孫を直接見せる件数（残りは「ほかN件」）。 */
export const DESCENDANT_LIMIT = 3;
/** 1行表示へ切り替える境界（調整項目）。 */
export const DENSITY_MIN_HEIGHT = 520;
export const DENSITY_MIN_WIDTH = 260;

const chatId = (c: Chat): string => `${c.key.backend}:${c.key.id}`;
const chainKey = (k: { backend: string; id: string }): string => `${k.backend}:${k.id}`;
const isActive = (v: AgentView): boolean => { const s = v.status.state; return s === "running" || s === "waiting" || s === "initializing"; };

const agentsOf = (snap: HostSnapshot, c: Chat): AgentView[] => snap.agents.filter((a) => chainKey(a.agent.chat) === chatId(c));
const pendingOf = (snap: HostSnapshot, c: Chat): PendingRequest[] =>
  snap.requests.filter((r) => chainKey(r.chat) === chatId(c) && r.state.kind === "pending");

/**
 * チャットの変化の署名。全エージェントの（キー・状態・turn・待ち理由・活動の要約）＋保留中の要求＋lastUsedAt。
 * 観測時刻（observedAt）は入れない（走査の再観測で並びが揺れないように）。
 */
export function chatSignature(snap: HostSnapshot, c: Chat): string {
  const ags = agentsOf(snap, c).map((a) => {
    const w = a.status.wait;
    const wait = w ? (w.reason.kind === "other" ? `other:${w.reason.raw}` : w.reason.kind) : "";
    const act = a.currentActivity;
    const sum = act ? (act.summary.kind === "value" ? `v:${act.summary.value}` : act.summary.kind) : "-";
    return [chainKey(a.agent.key), a.status.state, a.status.turn ?? "", wait, sum].join("\u001f");
  }).sort();
  const reqs = pendingOf(snap, c).map((r) => `${r.key.source}/${r.key.requestId}`).sort();
  return [ags.join("\u001e"), reqs.join(","), c.lastUsedAt ?? ""].join("\u001d");
}

export type RecencyEntry = { sig: string; at: number };
export type Recency = Map<string, RecencyEntry>;

/** 初回の時刻: max(lastUsedAt, 親の現在の活動の observedAt, createdAt)。無ければ 0。 */
function initialAt(snap: HostSnapshot, c: Chat): number {
  const root = agentsOf(snap, c).find((a) => a.agent.parent.kind === "root");
  const created = c.createdAt.kind === "value" ? c.createdAt.value : 0;
  return Math.max(c.lastUsedAt ?? 0, root?.currentActivity?.evidence.observedAt ?? 0, created);
}

/** 署名が変わったチャットだけ at=now に更新。消えたチャットは捨てる。prev は書き換えない。 */
export function trackRecency(prev: Recency, snap: HostSnapshot, now: number): Recency {
  const next: Recency = new Map();
  for (const c of snap.chats) {
    const id = chatId(c);
    const sig = chatSignature(snap, c);
    const old = prev.get(id);
    if (!old) next.set(id, { sig, at: initialAt(snap, c) });
    else next.set(id, old.sig === sig ? old : { sig, at: now });
  }
  return next;
}

/** 新しい順。同時刻はチャットIDで安定。記録の無いものは最後。 */
export function orderByRecency(chats: Chat[], recency: Recency): Chat[] {
  const at = (c: Chat): number => recency.get(chatId(c))?.at ?? -1;
  return [...chats].sort((a, b) => at(b) - at(a) || (chatId(a) < chatId(b) ? -1 : chatId(a) > chatId(b) ? 1 : 0));
}

export type TaskSelection = {
  /** 保留中の要求があるチャット（要求を受けた古い順）。5件の上限に数えない。 */
  attention: Chat[];
  /** 最近動かした順の先頭 limit 件（attention と重ならない）。 */
  tasks: Chat[];
  /** 6件目以降（1行で出す。消さない）。 */
  overflow: Chat[];
  /** 状態不明のため出さないチャット（隠すだけで消さない）。 */
  hiddenUnknown: Chat[];
};

/**
 * タスク表示の対象選び。
 * - 帯: 保留中の要求があるもの（アーカイブ済みでも）。
 * - タスク: アーカイブ済みでないもの（アーカイブ済みは isWorking のときだけ）。閉じたもの（署名が閉じた時のまま）は除く。
 * `isWorking` は呼び出し側の「親または子孫が作業中・対応待ち」判定（isWorkingForDock）。`closed` は chatId→閉じた時の署名。
 * 並びは「作業中が先、同じ区分の中は最近動かした順」。`isUnknown`（親の状態が不明）で作業中でも対応待ちでもないチャットは、タスクにもほかのタスクにも出さない
 * （hiddenUnknown。状態は変えず隠すだけ。「選択中」「全」では出る。回復・作業開始で次の選定に戻る）。
 */
export function selectTasks(
  snap: HostSnapshot, recency: Recency, closed: ReadonlyMap<string, string>,
  isWorking: (c: Chat) => boolean, limit: number = TASK_LIMIT, isUnknown: (c: Chat) => boolean = () => false,
): TaskSelection {
  const firstPending = (c: Chat): number => Math.min(...pendingOf(snap, c).map((r) => r.receivedAt));
  const attention = snap.chats.filter((c) => pendingOf(snap, c).length > 0)
    .sort((a, b) => firstPending(a) - firstPending(b) || (chatId(a) < chatId(b) ? -1 : 1));
  const inBand = new Set(attention.map(chatId));
  const candidates = snap.chats.filter((c) => {
    const id = chatId(c);
    if (inBand.has(id)) return false;
    if (c.archived.kind === "value" && c.archived.value && !isWorking(c)) return false;
    const cs = closed.get(id);
    return cs === undefined || cs !== (recency.get(id)?.sig ?? chatSignature(snap, c));
  });
  const hiddenUnknown = candidates.filter((c) => !isWorking(c) && isUnknown(c));
  const hidden = new Set(hiddenUnknown.map(chatId));
  const byRecency = orderByRecency(candidates.filter((c) => !hidden.has(chatId(c))), recency);
  const ordered = [...byRecency.filter((c) => isWorking(c)), ...byRecency.filter((c) => !isWorking(c))];
  return { attention, tasks: ordered.slice(0, limit), overflow: ordered.slice(limit), hiddenUnknown };
}

/**
 * 子孫（親以外）の直近 n 件。作業中・対応待ちを優先し、続けて最近の変化順（活動→状態の観測時刻の新しい順）。
 * 残り件数 rest を返す。
 */
export function pickRecentDescendants(snap: HostSnapshot, c: Chat, n: number = DESCENDANT_LIMIT): { shown: AgentView[]; rest: number } {
  const at = (v: AgentView): number => Math.max(v.currentActivity?.evidence.observedAt ?? 0, v.status.evidence.observedAt);
  const kids = agentsOf(snap, c).filter((a) => a.agent.parent.kind !== "root");
  const sorted = kids
    .map((v, i) => ({ v, i }))
    .sort((a, b) => Number(isActive(b.v)) - Number(isActive(a.v)) || at(b.v) - at(a.v) || a.i - b.i)
    .map((x) => x.v);
  return { shown: sorted.slice(0, n), rest: Math.max(0, sorted.length - n) };
}

/** ドックが低い・狭いときは1行表示。 */
export function cardDensity(d: { dockH: number; dockW: number }): "line" | "card" {
  return d.dockH < DENSITY_MIN_HEIGHT || d.dockW < DENSITY_MIN_WIDTH ? "line" : "card";
}

/** 入力中（フォーカス中で、変換中か文字がある）は、スクロール・展開・並べ替えをしない。 */
export function mayRedirect(s: { focusInInput: boolean; composing: boolean; hasText: boolean }): boolean {
  return !(s.focusInInput && (s.composing || s.hasText));
}

/**
 * 帯とタスク列の所属の凍結。frozen（入力中）の間は、前回その容器にあった（今も有る）チャットを同じ容器に保つ。
 * 入力中のカードが帯・タスク列・「ほか」へ移ると React がカードを作り直し、フォーカスと IME 変換が失われるため。
 * 新しい対応待ちは、すでにタスク列にあるチャットなら移さない（印だけ。呼び出し側）。タスク列の新規は上限までだけ足す（残りは「ほか」）。
 * frozen でなければ next をそのまま返す。
 */
export function stableMembership(
  prevBand: string[], prevTask: string[], nextBand: string[], nextTask: string[],
  frozen: boolean, exists: ReadonlySet<string>, limit: number = TASK_LIMIT,
): { band: string[]; task: string[] } {
  if (!frozen) return { band: nextBand, task: nextTask };
  const band = prevBand.filter((id) => exists.has(id));
  const bandSet = new Set(band);
  const task = prevTask.filter((id) => exists.has(id) && !bandSet.has(id));
  const known = new Set([...band, ...task]);
  for (const id of nextBand) if (!known.has(id)) { band.push(id); known.add(id); }
  for (const id of nextTask) if (!known.has(id) && task.length < limit) { task.push(id); known.add(id); }
  return { band, task };
}

/**
 * 並びの凍結。frozen の間は前回の並び（今も有る ID）を保ち、新しい ID は末尾に足す。frozen でなければ next。
 */
export function stableOrder(prev: string[], next: string[], frozen: boolean): string[] {
  if (!frozen) return next;
  const now = new Set(next);
  const kept = prev.filter((id) => now.has(id));
  const have = new Set(kept);
  return [...kept, ...next.filter((id) => !have.has(id))];
}

/** カード内の子ツリー展開部の高さ上限（ドック高さに対する分母。1/2）。styles.css の .tc-kids-box と揃える。 */
export const KIDS_BOX_FRACTION = 2;

export type DescendantSummary = {
  total: number;
  working: number; waiting: number; idle: number; done: number; interrupted: number; failed: number;
  /** 状態不明（履歴で終端を確認できていないもの）。完了に数えない。 */
  unknown: number;
  /** 状態不明のうち、履歴の読取りで終端を確認したもの（live確認ではない）。 */
  history: number;
  /** 子孫がスナップショットに無い。0件と「未取得」は区別できないので、無ければ未取得側に倒す（ドックからは読みに行かない）。 */
  notFetched: boolean;
};

/** 子孫（親以外）の内訳。履歴確認済みは完了・失敗に混ぜず別に数える（§3.5・§4.1）。outcomeOf は呼び出し側の historyOutcomeOf。 */
export function descendantSummary(snap: HostSnapshot, c: Chat, outcomeOf: (v: AgentView) => string | null): DescendantSummary {
  const s: DescendantSummary = { total: 0, working: 0, waiting: 0, idle: 0, done: 0, interrupted: 0, failed: 0, unknown: 0, history: 0, notFetched: false };
  for (const v of agentsOf(snap, c)) {
    if (v.agent.parent.kind === "root") continue;
    s.total++;
    switch (v.status.state) {
      case "running": case "initializing": s.working++; break;
      case "waiting": s.waiting++; break;
      case "idle": s.idle++; break;
      case "done": case "closed": s.done++; break;
      case "interrupted": s.interrupted++; break;
      case "failed": s.failed++; break;
      case "unknown": if (outcomeOf(v)) s.history++; else s.unknown++; break;
    }
  }
  s.notFetched = s.total === 0;
  return s;
}

/** 「子 N件（作業中a・対応待ちb…）」。0件の内訳は省く。未取得のときは取得を促す文言（読みには行かない）。 */
export function descendantSummaryText(s: DescendantSummary): string {
  if (s.notFetched) return "子孫: 未取得（中央で開くと読み込みます）";
  const parts: [string, number][] = [
    ["作業中", s.working], ["対応待ち", s.waiting], ["待機", s.idle], ["完了", s.done], ["中断", s.interrupted],
    ["失敗", s.failed], ["状態不明", s.unknown], ["履歴で確認", s.history],
  ];
  return `子 ${s.total}件（${parts.filter(([, n]) => n > 0).map(([l, n]) => `${l}${n}`).join("・")}）`;
}

/**
 * カード内ツリーの根（メイン直下の子。親が一覧に無い・親不明のものも含む）の優先順。
 * 枝の中に作業中・対応待ちがあるものを先、続けて枝内の最新の観測時刻の新しい順、同じなら元の順。キーの配列を返す。
 */
export function branchOrder(snap: HostSnapshot, c: Chat): string[] {
  const all = agentsOf(snap, c);
  const inChat = new Set(all.map((a) => chainKey(a.agent.key)));
  const childrenOf = new Map<string, AgentView[]>();
  for (const a of all) {
    if (a.agent.parent.kind !== "explicit") continue;
    const p = chainKey(a.agent.parent.parent);
    childrenOf.set(p, [...(childrenOf.get(p) ?? []), a]);
  }
  const at = (v: AgentView): number => Math.max(v.currentActivity?.evidence.observedAt ?? 0, v.status.evidence.observedAt);
  const walk = (v: AgentView, seen: Set<string>): { active: boolean; at: number } => {
    const k = chainKey(v.agent.key);
    if (seen.has(k)) return { active: false, at: 0 };
    seen.add(k);
    let r = { active: isActive(v), at: at(v) };
    for (const ch of childrenOf.get(k) ?? []) {
      const x = walk(ch, seen);
      r = { active: r.active || x.active, at: Math.max(r.at, x.at) };
    }
    return r;
  };
  const rootKeys = new Set(all.filter((a) => a.agent.parent.kind === "root").map((a) => chainKey(a.agent.key)));
  const tops = all.filter((a) => {
    const p = a.agent.parent;
    if (p.kind === "root") return false;
    if (p.kind === "unknown") return true;
    const pk = chainKey(p.parent);
    return rootKeys.has(pk) || !inChat.has(pk);
  });
  return tops
    .map((v, i) => ({ k: chainKey(v.agent.key), i, ...walk(v, new Set()) }))
    .sort((a, b) => Number(b.active) - Number(a.active) || b.at - a.at || a.i - b.i)
    .map((x) => x.k);
}

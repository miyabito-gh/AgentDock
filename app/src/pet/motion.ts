// ペットの動き（DESIGN_PET「データ互換」「状態と動き」）。純粋ロジック。Node で直接テストできるよう、型だけに依存する。
// 状態の材料は既存のスナップショット（エージェントの状態・承認待ちの要求）だけで、監視のための追加取得はしない。
import type { AgentState } from "../ipc/types";

// ── 画像（Codex V2: 1536×2288、8列×11行、1コマ192×208） ──
export const FRAME_W = 192;
export const FRAME_H = 208;
export const SHEET_COLS = 8;
export const SHEET_ROWS = 11;
export const SHEET_W = FRAME_W * SHEET_COLS;
export const SHEET_H = FRAME_H * SHEET_ROWS;

export type MotionName = "idle" | "runningRight" | "runningLeft" | "waving" | "jumping" | "failed" | "waiting" | "running" | "review";

/** 行と、1コマごとの表示時間（ms）。コマ数＝durations の長さ（列は 0 から）。出典は hatch-pet の animation-rows。 */
export interface MotionDef { row: number; durations: number[] }

const steady = (frames: number, ms: number, last: number): number[] => [...Array<number>(frames - 1).fill(ms), last];

export const MOTIONS: Record<MotionName, MotionDef> = {
  idle: { row: 0, durations: [280, 110, 110, 140, 140, 320] },
  runningRight: { row: 1, durations: steady(8, 120, 220) },
  runningLeft: { row: 2, durations: steady(8, 120, 220) },
  waving: { row: 3, durations: steady(4, 140, 280) },
  jumping: { row: 4, durations: steady(5, 140, 280) },
  failed: { row: 5, durations: steady(8, 140, 240) },
  waiting: { row: 6, durations: steady(6, 150, 260) },
  running: { row: 7, durations: steady(6, 120, 220) },
  review: { row: 8, durations: steady(6, 150, 280) },
};

const sum = (a: number[]): number => a.reduce((x, y) => x + y, 0);
export const motionLength = (name: MotionName): number => sum(MOTIONS[name].durations);

/** 経過時間から、表示する列と次のコマまでの残り時間。ループする。 */
export function frameAt(name: MotionName, elapsedMs: number): { col: number; remaining: number } {
  const d = MOTIONS[name].durations;
  const total = sum(d);
  let t = ((elapsedMs % total) + total) % total;
  for (let i = 0; i < d.length; i++) {
    if (t < d[i]) return { col: i, remaining: d[i] - t };
    t -= d[i];
  }
  return { col: 0, remaining: d[0] };
}

// ── 目線（行9-10）。16方向、0°＝上から時計回りに22.5°刻み。方向0-7が行9、8-15が行10（列＝方向を8で割った余り） ──
export const GAZE_DIRECTIONS = 16;

/** 中心からのずれ（右が+x、下が+y）を方向番号（0〜15）にする。 */
export function gazeIndex(dx: number, dy: number): number {
  const deg = ((Math.atan2(dx, -dy) * 180) / Math.PI + 360) % 360;
  return Math.round(deg / 22.5) % GAZE_DIRECTIONS;
}
export const gazeCell = (index: number): { row: number; col: number } => ({ row: 9 + Math.floor(index / 8), col: index % 8 });

// ── 時間 ──
export const JUMP_MS = motionLength("jumping");
/** 正常終了: review → waving（それぞれ約2回）→ idle。 */
export const REVIEW_MS = motionLength("review") * 2;
export const WAVE_MS = motionLength("waving") * 2;
/** 失敗の動きを見せる長さ。吹き出し・チャット側の印が本体で、動きは一時的。 */
export const FAILED_MS = 8000;

// ── 状態の追跡 ──
export interface AgentLite {
  /** エージェントの識別（チャットID＋エージェントID）。 */
  key: string;
  /** チャットの親（ルート）か。完了・失敗の動きはルートだけで決める（子は2秒集約のOS通知・吹き出し側）。 */
  root: boolean;
  state: AgentState;
  /** 接続断・要照合など、いまの状態が最新と確認できていない。 */
  stale: boolean;
}

export interface PetTracker {
  seeded: boolean;
  /** 前回のエージェント状態。 */
  prev: Record<string, AgentState>;
  /** 作業中だったのに状態不明・接続断になったもの。完了とは表示せず、印だけ出す。 */
  lost: string[];
}
export const emptyTracker = (): PetTracker => ({ seeded: false, prev: {}, lost: [] });

export const isActive = (st: AgentState): boolean => st === "running" || st === "waiting" || st === "initializing";

export interface TrackResult { tracker: PetTracker; completed: boolean; failed: boolean }

/**
 * 新しい観測を取り込み、作業中→完了／失敗の遷移を検出する。`ready`＝スナップショットを受け取った後。最初の観測は取り込むだけで遷移とみなさない。
 * 完了と数えるのはルートが `done` に変わったときだけ。状態不明・接続断は完了にも失敗にもせず、`lost` に印を残す。
 * 失敗は `failed`/`interrupted`（停止未確認は状態が作業中のままなので、ここでは失敗に数えない）。
 */
export function trackAgents(t: PetTracker, agents: AgentLite[], ready: boolean): TrackResult {
  if (!ready) return { tracker: t, completed: false, failed: false };
  const prev: Record<string, AgentState> = {};
  let completed = false;
  let failed = false;
  const lost = new Set(t.lost);
  const present = new Set<string>();
  for (const a of agents) {
    prev[a.key] = a.state;
    present.add(a.key);
    const before = t.prev[a.key];
    const wasActive = before !== undefined && isActive(before);
    if (t.seeded) {
      if (a.root && wasActive && a.state === "done") completed = true;
      if (a.root && wasActive && (a.state === "failed" || a.state === "interrupted")) failed = true;
    }
    if (a.state === "unknown" || a.stale) {
      if (wasActive) lost.add(a.key);
    } else {
      lost.delete(a.key);
    }
  }
  for (const k of [...lost]) if (!present.has(k)) lost.delete(k);
  return { tracker: { seeded: true, prev, lost: [...lost] }, completed, failed };
}

// ── 動きの選択 ──
export interface PetInputs {
  now: number;
  reducedMotion: boolean;
  /** ドラッグ中の向き（窓の移動方向）。 */
  drag: "left" | "right" | null;
  /** クリックで跳ねた時刻。 */
  jumpAt: number | null;
  /** 承認・質問待ちの要求がある。 */
  pending: boolean;
  failedAt: number | null;
  doneAt: number | null;
  /** いずれかのエージェントが作業中（複数でも1つにまとめる）。 */
  running: boolean;
  /** 目線の方向番号（待機中のときだけ使う）。 */
  gaze: number | null;
}

export type Pose = { kind: "motion"; name: MotionName; since: number; still: boolean } | { kind: "gaze"; index: number };

const within = (now: number, from: number | null, ms: number): boolean => from !== null && now >= from && now < from + ms;

/**
 * 優先順位: 操作（ドラッグ・クリック）＞ 待ち ＞ 失敗 ＞ 完了 ＞ 実行中 ＞ 待機。動きを減らす設定のときは idle の先頭コマ固定。
 * 状態不明・接続断は完了にならないので、ここへは入らない（待機のまま。印は画面側が出す）。
 */
export function choosePose(i: PetInputs): Pose {
  if (i.reducedMotion) return { kind: "motion", name: "idle", since: 0, still: true };
  const m = (name: MotionName, since = 0): Pose => ({ kind: "motion", name, since, still: false });
  if (i.drag) return m(i.drag === "left" ? "runningLeft" : "runningRight");
  if (within(i.now, i.jumpAt, JUMP_MS)) return m("jumping", i.jumpAt!);
  if (i.pending) return m("waiting");
  if (within(i.now, i.failedAt, FAILED_MS)) return m("failed", i.failedAt!);
  if (within(i.now, i.doneAt, REVIEW_MS)) return m("review", i.doneAt!);
  if (within(i.now, i.doneAt === null ? null : i.doneAt + REVIEW_MS, WAVE_MS)) return m("waving", i.doneAt! + REVIEW_MS);
  if (i.running) return m("running");
  if (i.gaze !== null) return { kind: "gaze", index: i.gaze };
  return m("idle");
}

/** 表示するセルと、次のコマまでの残り時間（ms。静止なら null）。 */
export function cellOf(p: Pose, now: number): { row: number; col: number; remaining: number | null } {
  if (p.kind === "gaze") return { ...gazeCell(p.index), remaining: null };
  if (p.still) return { row: MOTIONS[p.name].row, col: 0, remaining: null };
  const f = frameAt(p.name, now - p.since);
  return { row: MOTIONS[p.name].row, col: f.col, remaining: f.remaining };
}

/** 待機（他の動きがない状態）か。状態不明の印と目線を出す条件。 */
export const isIdlePose = (p: Pose): boolean => p.kind === "gaze" || (p.kind === "motion" && p.name === "idle");

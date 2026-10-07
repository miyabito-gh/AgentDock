// ホストイベントの適用をまとめる（P3-9、DESIGN_P3 #18）。状態の判定はしない。順序を変えず、欠落させない。
// - DeltaCoalescer: 逐次本文（activityDelta）を同一itemで一定時間（既定50ms）まとめる。delta以外のイベントが来たら先に全部流す（終端・状態変化の前に本文が出揃う）。
// - FrameBatcher: 適用を1フレーム（requestAnimationFrame）にまとめる。非表示で止まっても溜め込まないよう、タイマーでも流す。
// テストは app/scripts/batch.test.mjs（node の型除去で実行）。型除去できる構文だけを使う。
import type { ItemKey } from "./types";

type DeltaLike = { kind: "activityDelta"; item: ItemKey; delta: string };
type EventLike = { kind: string };

const itemKeyOf = (i: ItemKey): string => `${i.agent.id}|${i.turnId ?? ""}|${i.itemId}`;
const isDelta = (e: EventLike): e is DeltaLike => e.kind === "activityDelta";

type Timers = { set: (f: () => void, ms: number) => ReturnType<typeof setTimeout>; clear: (t: ReturnType<typeof setTimeout>) => void };

export const DELTA_WINDOW_MS = 50;

export class DeltaCoalescer<E extends EventLike> {
  private buf = new Map<string, DeltaLike>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private emit: (e: E) => void;
  private windowMs: number;
  private timers: Timers;
  constructor(emit: (e: E) => void, windowMs: number = DELTA_WINDOW_MS, timers: Timers = { set: (f, ms) => setTimeout(f, ms), clear: (t) => clearTimeout(t) }) {
    this.emit = emit; this.windowMs = windowMs; this.timers = timers;
  }

  push(e: E): void {
    if (!isDelta(e)) {
      this.flush();
      this.emit(e);
      return;
    }
    const k = itemKeyOf(e.item);
    const cur = this.buf.get(k);
    if (cur) cur.delta += e.delta;
    else this.buf.set(k, { kind: "activityDelta", item: e.item, delta: e.delta });
    if (this.timer === null) this.timer = this.timers.set(() => { this.timer = null; this.flush(); }, this.windowMs);
  }

  /** 溜めている本文を、最初に来た順で流す。 */
  flush(): void {
    if (this.timer !== null) { this.timers.clear(this.timer); this.timer = null; }
    if (this.buf.size === 0) return;
    const items = [...this.buf.values()];
    this.buf.clear();
    for (const d of items) this.emit(d as unknown as E);
  }

  /** 溜めているものを捨てる（スナップショットで置き換えるとき。それ以前のイベントはスナップショットに含まれる）。 */
  clear(): void {
    if (this.timer !== null) { this.timers.clear(this.timer); this.timer = null; }
    this.buf.clear();
  }
}

export class FrameBatcher<T> {
  private q: T[] = [];
  private raf: number | null = null;
  private fallback: ReturnType<typeof setTimeout> | null = null;
  private apply: (items: T[]) => void;
  private fallbackMs: number;
  constructor(apply: (items: T[]) => void, fallbackMs: number = 100) {
    this.apply = apply; this.fallbackMs = fallbackMs;
  }

  push(item: T): void {
    this.q.push(item);
    if (this.raf !== null || this.fallback !== null) return;
    if (typeof requestAnimationFrame === "function") this.raf = requestAnimationFrame(() => this.flush());
    this.fallback = setTimeout(() => this.flush(), this.fallbackMs);
  }

  flush(): void {
    if (this.raf !== null && typeof cancelAnimationFrame === "function") cancelAnimationFrame(this.raf);
    if (this.fallback !== null) clearTimeout(this.fallback);
    this.raf = null;
    this.fallback = null;
    if (this.q.length === 0) return;
    const items = this.q;
    this.q = [];
    this.apply(items);
  }

  clear(): void {
    if (this.raf !== null && typeof cancelAnimationFrame === "function") cancelAnimationFrame(this.raf);
    if (this.fallback !== null) clearTimeout(this.fallback);
    this.raf = null;
    this.fallback = null;
    this.q = [];
  }
}

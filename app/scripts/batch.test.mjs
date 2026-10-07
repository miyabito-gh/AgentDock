// P3-9 の純粋ロジックのテスト。実行: node app/scripts/batch.test.mjs（Node 22.6+ の型除去。依存追加なし）
import assert from "node:assert/strict";
import { DeltaCoalescer, FrameBatcher } from "../src/ipc/batch.ts";
import { windowRange, scrollTopToReveal, ROW_HEIGHT } from "../src/ui/listWindow.ts";

const item = (a, i, t = "t1") => ({ agent: { backend: "codex", id: a }, turnId: t, itemId: i });
const delta = (a, i, d) => ({ kind: "activityDelta", item: item(a, i), delta: d });

// 手動タイマー
function fakeTimers() {
  let next = 1; const pending = new Map();
  return {
    set: (f) => { const id = next++; pending.set(id, f); return id; },
    clear: (id) => { pending.delete(id); },
    fire: () => { const fs = [...pending.values()]; pending.clear(); fs.forEach((f) => f()); },
    count: () => pending.size,
  };
}

// 1. 同一itemは合流し、順序が保たれる
{
  const out = []; const t = fakeTimers();
  const c = new DeltaCoalescer((e) => out.push(e), 50, t);
  c.push(delta("a", "i1", "he")); c.push(delta("a", "i1", "llo")); c.push(delta("b", "i9", "X")); c.push(delta("a", "i1", " world"));
  assert.equal(out.length, 0); assert.equal(t.count(), 1, "one timer per window");
  t.fire();
  assert.deepEqual(out.map((e) => [e.item.agent.id, e.delta]), [["a", "hello world"], ["b", "X"]]);
}
// 2. delta以外（終端など）が来たら、先に本文を全部流してから流す。欠落・入れ替わりなし
{
  const out = []; const t = fakeTimers();
  const c = new DeltaCoalescer((e) => out.push(e), 50, t);
  c.push(delta("a", "i1", "p1")); c.push(delta("a", "i1", "p2"));
  c.push({ kind: "turnUpdated", end: "completed" });
  c.push(delta("a", "i1", "p3"));
  t.fire();
  assert.deepEqual(out.map((e) => e.kind === "activityDelta" ? e.delta : e.kind), ["p1p2", "turnUpdated", "p3"]);
  assert.equal(t.count(), 0);
}
// 3. 同じitemでもturnが違えば別
{
  const out = []; const t = fakeTimers();
  const c = new DeltaCoalescer((e) => out.push(e), 50, t);
  c.push({ kind: "activityDelta", item: item("a", "i1", "t1"), delta: "1" }); c.push({ kind: "activityDelta", item: item("a", "i1", "t2"), delta: "2" });
  c.flush(); assert.equal(out.length, 2);
}
// 4. clear は溜めたものを捨てる
{
  const out = []; const t = fakeTimers();
  const c = new DeltaCoalescer((e) => out.push(e), 50, t);
  c.push(delta("a", "i1", "x")); c.clear(); t.fire(); c.flush(); assert.equal(out.length, 0);
}
// 5. FrameBatcher: 1回のflushにまとまり、順序が保たれる（rAFなしの環境ではタイマーで流れる）
{
  const batches = [];
  const f = new FrameBatcher((items) => batches.push(items), 5);
  f.push(1); f.push(2); f.push(3);
  assert.equal(batches.length, 0);
  await new Promise((r) => setTimeout(r, 30));
  assert.deepEqual(batches, [[1, 2, 3]]);
  f.push(4); f.clear(); await new Promise((r) => setTimeout(r, 30)); assert.equal(batches.length, 1, "clear drops queued items");
  f.push(5); f.flush(); assert.deepEqual(batches[1], [5]);
}
// 6. ウィンドウ範囲
{
  assert.deepEqual(windowRange(0, 0, 400), { start: 0, end: 0 });
  const r0 = windowRange(1000, 0, 520);
  assert.equal(r0.start, 0); assert.ok(r0.end >= 10 && r0.end <= 30);
  const r = windowRange(1000, ROW_HEIGHT * 500, 520);
  assert.ok(r.start <= 500 && r.end > 500 + 10 - 1 && r.end - r.start < 40);
  assert.deepEqual(windowRange(1000, ROW_HEIGHT * 5000, 520).end, 1000, "clamped to total");
  assert.ok(windowRange(1000, ROW_HEIGHT * 5000, 520).start <= 1000);
  // 全行が必ずどこかのスクロール位置で描かれる
  const seen = new Set();
  for (let top = 0; top < 300 * ROW_HEIGHT; top += 37) { const w = windowRange(300, top, 500); for (let i = w.start; i < w.end; i++) seen.add(i); }
  assert.equal(seen.size, 300);
  // revealのスクロール位置
  assert.equal(scrollTopToReveal(9, 0, 520), 0);
  assert.equal(scrollTopToReveal(20, 0, 520), 21 * ROW_HEIGHT - 520);
  assert.equal(scrollTopToReveal(2, 10 * ROW_HEIGHT, 520), 2 * ROW_HEIGHT);
}
console.log("batch/listWindow tests passed");

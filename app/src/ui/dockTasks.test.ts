// 実行: node --test src/ui/dockTasks.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { cardDensity, chatSignature, mayRedirect, orderByRecency, pickRecentDescendants, selectTasks, stableOrder, stableMembership, trackRecency } from "./dockTasks.ts";

const chat = (id: string, o: any = {}): any => ({ key: { backend: "codex", id }, lastUsedAt: null, createdAt: { kind: "notFetched" }, archived: { kind: "value", value: false }, ...o });
const agent = (cid: string, id: string, state: string, o: any = {}): any => ({
  agent: { key: { backend: "codex", id }, chat: { backend: "codex", id: cid }, parent: o.parent ?? { kind: "root" } },
  status: { state, turn: o.turn ?? null, wait: o.wait ?? null, evidence: { observedAt: o.obs ?? 1 } },
  currentActivity: o.act === undefined ? null : { summary: { kind: "value", value: o.act }, evidence: { observedAt: o.actObs ?? 1 } },
});
const req = (cid: string, id: string, at = 1, state = "pending"): any => ({ key: { source: "s", requestId: id }, chat: { backend: "codex", id: cid }, state: { kind: state }, receivedAt: at });
const snap = (chats: any[], agents: any[] = [], requests: any[] = []): any => ({ chats, agents, requests });
const ids = (cs: any[]) => cs.map((c) => c.key.id);

test("署名: observedAtだけの変化では変わらない", () => {
  const c = chat("a");
  const s1 = snap([c], [agent("a", "r", "running", { act: "x", obs: 1, actObs: 1 })]);
  const s2 = snap([c], [agent("a", "r", "running", { act: "x", obs: 99, actObs: 99 })]);
  assert.equal(chatSignature(s1, c), chatSignature(s2, c));
});
test("署名: 状態・turn・待ち理由・要約・要求・lastUsedAtで変わる", () => {
  const c = chat("a");
  const base = chatSignature(snap([c], [agent("a", "r", "running", { act: "x" })]), c);
  const diff = (s: any, cc = c) => assert.notEqual(chatSignature(s, cc), base);
  diff(snap([c], [agent("a", "r", "done", { act: "x" })]));
  diff(snap([c], [agent("a", "r", "running", { act: "x", turn: "t2" })]));
  diff(snap([c], [agent("a", "r", "running", { act: "x", wait: { reason: { kind: "approval" } } })]));
  diff(snap([c], [agent("a", "r", "running", { act: "y" })]));
  diff(snap([c], [agent("a", "r", "running", { act: "x" })], [req("a", "q")]));
  const c2 = chat("a", { lastUsedAt: 5 });
  diff(snap([c2], [agent("a", "r", "running", { act: "x" })]), c2);
});
test("trackRecency: 初回はmax、署名が変わったものだけnowに", () => {
  const a = chat("a", { lastUsedAt: 10, createdAt: { kind: "value", value: 5 } });
  const b = chat("b");
  const s1 = snap([a, b], [agent("a", "ra", "running", { act: "x", actObs: 50 }), agent("b", "rb", "done")]);
  const r1 = trackRecency(new Map(), s1, 1000);
  assert.equal(r1.get("codex:a")!.at, 50);
  assert.equal(r1.get("codex:b")!.at, 0);
  const s2 = snap([a, b], [agent("a", "ra", "running", { act: "x", actObs: 70 }), agent("b", "rb", "failed")]);
  const r2 = trackRecency(r1, s2, 2000);
  assert.equal(r2.get("codex:a")!.at, 50);
  assert.equal(r2.get("codex:b")!.at, 2000);
});
test("orderByRecency: 新しい順、同時刻はID順", () => {
  const rec = new Map([["codex:a", { sig: "", at: 5 }], ["codex:b", { sig: "", at: 9 }], ["codex:c", { sig: "", at: 5 }]]);
  assert.deepEqual(ids(orderByRecency([chat("c"), chat("a"), chat("b")], rec)), ["b", "a", "c"]);
});

const many = (n: number) => { const cs = Array.from({ length: n }, (_, i) => chat(`c${i}`)); const rec = new Map(cs.map((c, i) => [`codex:${c.key.id}`, { sig: "s", at: i + 1 }])); return { cs, rec }; };
test("selectTasks: 5件上限、6件目以降はoverflow、新しい順", () => {
  const { cs, rec } = many(7);
  const r = selectTasks(snap(cs), rec as any, new Map(), () => false);
  assert.deepEqual(ids(r.tasks), ["c6", "c5", "c4", "c3", "c2"]);
  assert.deepEqual(ids(r.overflow), ["c1", "c0"]);
});
test("selectTasks: 帯とタスクは重ならず、帯は上限に数えず古い要求順、アーカイブ済みでも帯に入る", () => {
  const { cs, rec } = many(7);
  cs[0].archived = { kind: "value", value: true };
  const r = selectTasks(snap(cs, [], [req("c3", "q1", 20), req("c0", "q2", 10)]), rec as any, new Map(), () => false);
  assert.deepEqual(ids(r.attention), ["c0", "c3"]);
  assert.equal(r.tasks.length, 5);
  assert.ok(![...r.tasks, ...r.overflow].some((c) => ["c0", "c3"].includes(c.key.id)));
  assert.equal(r.tasks.length + r.overflow.length, 5);
});
test("selectTasks: 保留でない要求は帯に入らない", () => {
  const { cs, rec } = many(2);
  assert.equal(selectTasks(snap(cs, [], [req("c0", "q", 1, "answered")]), rec as any, new Map(), () => false).attention.length, 0);
});
test("selectTasks: アーカイブ済みは作業中のときだけ", () => {
  const { cs, rec } = many(2);
  cs[0].archived = { kind: "value", value: true };
  cs[1].archived = { kind: "value", value: true };
  const r = selectTasks(snap(cs), rec as any, new Map(), (c) => c.key.id === "c1");
  assert.deepEqual(ids(r.tasks), ["c1"]);
});
test("selectTasks: 閉じたものは署名が変わるまで出ない", () => {
  const { cs, rec } = many(2);
  const closed = new Map([["codex:c1", "s"]]);
  assert.deepEqual(ids(selectTasks(snap(cs), rec as any, closed, () => false).tasks), ["c0"]);
  const rec2 = new Map(rec); rec2.set("codex:c1", { sig: "s2", at: 9 });
  assert.deepEqual(ids(selectTasks(snap(cs), rec2 as any, closed, () => false).tasks), ["c1", "c0"]);
});

test("pickRecentDescendants: 作業中を優先し、残り件数を返す", () => {
  const c = chat("a");
  const kid = (id: string, st: string, obs: number) => agent("a", id, st, { parent: { kind: "explicit" }, obs });
  const s = snap([c], [agent("a", "r", "running"), kid("k1", "done", 100), kid("k2", "running", 1), kid("k3", "done", 50), kid("k4", "waiting", 2), kid("k5", "failed", 200)]);
  const r = pickRecentDescendants(s, c);
  assert.deepEqual(r.shown.map((v: any) => v.agent.key.id), ["k4", "k2", "k5"]);
  assert.equal(r.rest, 2);
  assert.equal(pickRecentDescendants(snap([c], [agent("a", "r", "done")]), c).rest, 0);
});

test("cardDensity: 高さ520未満・幅260未満で1行", () => {
  assert.equal(cardDensity({ dockH: 520, dockW: 260 }), "card");
  assert.equal(cardDensity({ dockH: 519, dockW: 400 }), "line");
  assert.equal(cardDensity({ dockH: 800, dockW: 259 }), "line");
});
test("mayRedirect: 入力欄にフォーカスし、変換中か文字ありなら横取りしない", () => {
  assert.equal(mayRedirect({ focusInInput: false, composing: true, hasText: true }), true);
  assert.equal(mayRedirect({ focusInInput: true, composing: false, hasText: false }), true);
  assert.equal(mayRedirect({ focusInInput: true, composing: true, hasText: false }), false);
  assert.equal(mayRedirect({ focusInInput: true, composing: false, hasText: true }), false);
});
test("stableOrder: 凍結中は前回の並びを保ち、消えたものを除き新規は末尾", () => {
  assert.deepEqual(stableOrder(["a", "b", "c"], ["c", "x", "a"], false), ["c", "x", "a"]);
  assert.deepEqual(stableOrder(["a", "b", "c"], ["c", "x", "a"], true), ["a", "c", "x"]);
});

test("stableMembership: 入力中は帯・タスクの所属を保つ（帯へ移さない・5件から押し出さない）", () => {
  const ex = new Set(["a", "b", "c", "d", "e", "f", "g"]);
  // 入力中のカード b に対応待ちが来ても、タスク列に残す
  assert.deepEqual(stableMembership([], ["a", "b", "c"], ["b"], ["a", "c"], true, ex), { band: [], task: ["a", "b", "c"] });
  // 上位5件が入れ替わっても、前回のカードは残り、新規は上限まで
  assert.deepEqual(stableMembership([], ["a", "b", "c", "d", "e"], [], ["f", "a", "b", "c", "d"], true, ex), { band: [], task: ["a", "b", "c", "d", "e"] });
  assert.deepEqual(stableMembership([], ["a", "b"], [], ["f", "g", "a", "b"], true, ex, 4), { band: [], task: ["a", "b", "f", "g"] });
  // 新しい対応待ち（タスクに無い）は帯へ。無くなったチャットは外す
  assert.deepEqual(stableMembership(["x"], ["a"], ["g"], ["a"], true, new Set(["a", "g"])), { band: ["g"], task: ["a"] });
  // 凍結しなければ next
  assert.deepEqual(stableMembership([], ["a", "b"], ["b"], ["a"], false, ex), { band: ["b"], task: ["a"] });
});

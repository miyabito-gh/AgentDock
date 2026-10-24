// 実行: node --test src/ui/dockTasks.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { orderWorkingFirst, branchOrder, descendantSummary, descendantSummaryText, isCompletedCard, completedKidsNote, cardDensity, chatSignature, mayRedirect, holdByTyping, nextHeldSig, orderByRecency, pickRecentDescendants, selectTasks, stableOrder, stableMembership, trackRecency, workSignature } from "./dockTasks.ts";

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
test("selectTasks: 作業中が古くても、完了より必ず上（最近動かした順は同区分の中だけ）", () => {
  const { cs, rec } = many(7); // c6が最新、c0が最古
  const r = selectTasks(snap(cs), rec as any, new Map(), (c) => c.key.id === "c0");
  assert.deepEqual(ids(r.tasks), ["c0", "c6", "c5", "c4", "c3"]);
  assert.deepEqual(ids(r.overflow), ["c2", "c1"]);
});
test("selectTasks: 状態不明は全体5件の余り枠だけ。作業中・取得済みを押し出さず、溢れはoverflowに出さず隠す（件数に含めない）", () => {
  const { cs, rec } = many(8); // c7が最新
  const unk = new Set(["c7", "c6", "c5", "c4", "c3", "c0"]);
  const r = selectTasks(snap(cs), rec as any, new Map(), (c) => c.key.id === "c0", 5, (c) => unk.has(c.key.id));
  assert.deepEqual(ids(r.tasks), ["c0", "c2", "c1", "c7", "c6"]);
  assert.deepEqual(ids(r.overflow), []);
  assert.deepEqual(ids(r.hiddenUnknown), ["c5", "c4", "c3"]);
});
test("selectTasks: 枠が余れば状態不明も出る。回復すれば最近順に戻る", () => {
  const { cs, rec } = many(3);
  const unk = new Set(["c2"]);
  assert.deepEqual(ids(selectTasks(snap(cs), rec as any, new Map(), () => false, 5, (c) => unk.has(c.key.id)).tasks), ["c1", "c0", "c2"]);
  unk.clear();
  assert.deepEqual(ids(selectTasks(snap(cs), rec as any, new Map(), () => false, 5, (c) => unk.has(c.key.id)).tasks), ["c2", "c1", "c0"]);
});
test("selectTasks: 状態不明でも保留中の要求があれば帯に残る", () => {
  const { cs, rec } = many(3);
  const r = selectTasks(snap(cs, [], [req("c1", "q")]), rec as any, new Map(), () => false, 5, () => true);
  assert.deepEqual(ids(r.attention), ["c1"]);
  assert.deepEqual(ids(r.tasks), ["c2", "c0"]);
});

test("orderWorkingFirst: 作業中が先、同区分は最近順", () => {
  const rec = new Map([["codex:a", { sig: "", at: 1 }], ["codex:b", { sig: "", at: 9 }], ["codex:c", { sig: "", at: 5 }], ["codex:d", { sig: "", at: 3 }]]);
  assert.deepEqual(ids(orderWorkingFirst([chat("a"), chat("b"), chat("c"), chat("d")], rec, (c) => c.key.id === "a" || c.key.id === "d")), ["d", "a", "b", "c"]);
  assert.deepEqual(ids(orderWorkingFirst([chat("a"), chat("b")], rec, () => false)), ["b", "a"]);
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

const kid = (cid: string, id: string, state: string, parent: string | null, o: any = {}): any =>
  agent(cid, id, state, { ...o, parent: parent ? { kind: "explicit", parent: { backend: "codex", id: parent } } : { kind: "root" } });

test("descendantSummary: 内訳。状態不明は完了に数えず、履歴確認は別", () => {
  const c = chat("a");
  const s = snap([c], [
    kid("a", "r", "done", null), kid("a", "k1", "running", "r"), kid("a", "k2", "done", "r"), kid("a", "k3", "unknown", "r"),
    kid("a", "k4", "unknown", "r"), kid("a", "k5", "failed", "k1"), kid("a", "k6", "waiting", "k1"),
  ]);
  const d = descendantSummary(s, c, (v) => (v.agent.key.id === "k4" ? "completed" : null));
  assert.deepEqual([d.total, d.working, d.waiting, d.done, d.failed, d.unknown, d.history, d.notFetched], [6, 1, 1, 1, 1, 1, 1, false]);
  assert.equal(descendantSummaryText(d), "サブエージェント 6件（作業中1・対応待ち1・完了1・失敗1・状態不明1・履歴で確認1）");
});
test("descendantSummary: 子孫が無ければ未取得（0件とは言わない）", () => {
  const c = chat("a");
  const d = descendantSummary(snap([c], [kid("a", "r", "done", null)]), c, () => null);
  assert.equal(d.notFetched, true);
  assert.match(descendantSummaryText(d), /まだ読み込んでいません/);
});
test("branchOrder: 枝に作業中があるものが先、メイン直下だけが根", () => {
  const c = chat("a");
  const s = snap([c], [
    kid("a", "r", "done", null),
    kid("a", "x", "done", "r", { obs: 100 }),
    kid("a", "y", "done", "r", { obs: 5 }), kid("a", "y1", "running", "y", { obs: 5 }),
    kid("a", "z", "done", "r", { obs: 50 }),
  ]);
  assert.deepEqual(branchOrder(s, c), ["codex:y", "codex:x", "codex:z"]);
});
test("branchOrder: 親が一覧に無い子は根に出し、入力中は stableOrder で並びを保てる", () => {
  const c = chat("a");
  const s = snap([c], [kid("a", "r", "done", null), kid("a", "o", "done", "missing", { obs: 1 }), kid("a", "k", "done", "r", { obs: 9 })]);
  assert.deepEqual(branchOrder(s, c), ["codex:k", "codex:o"]);
  assert.deepEqual(stableOrder(["codex:o", "codex:k"], branchOrder(s, c), true), ["codex:o", "codex:k"]);
});
test("selectTasks: 取得済みの溢れはoverflow、状態不明の溢れは隠す。作業中の状態不明は隠さない", () => {
  const { cs, rec } = many(8);
  const unk = new Set(["c7", "c6", "c0"]);
  const r = selectTasks(snap(cs), rec as any, new Map(), (c) => c.key.id === "c0", 5, (c) => unk.has(c.key.id));
  assert.deepEqual(ids(r.tasks), ["c0", "c5", "c4", "c3", "c2"]);
  assert.deepEqual(ids(r.overflow), ["c1"]);
  assert.deepEqual(ids(r.hiddenUnknown), ["c7", "c6"]);
});

test("selectTasks: 作業中6件以上は上位5件が作業中、作業中でないものは押し出さない", () => {
  const { cs, rec } = many(9); // c0..c8、新しいほど大きい
  const working = new Set(["c0", "c1", "c2", "c3", "c4", "c5"]);
  const r = selectTasks(snap(cs), rec as any, new Map(), (c) => working.has(c.key.id));
  assert.deepEqual(ids(r.tasks), ["c5", "c4", "c3", "c2", "c1"]);
  assert.deepEqual(ids(r.overflow), ["c0", "c8", "c7", "c6"]); // 6件目の作業中が先
});
test("selectTasks: 親unknownで子が作業中(isWorking真)はrank 0、子孫が作業中でない不明は従来どおり隠す", () => {
  const { cs, rec } = many(8);
  const isW = (c: any) => c.key.id === "c0";
  const isU = (c: any) => ["c0", "c1", "c2"].includes(c.key.id);
  const r = selectTasks(snap(cs), rec as any, new Map(), isW, 5, isU);
  assert.equal(ids(r.tasks)[0], "c0");
  assert.ok(!ids(r.overflow).some((i) => i === "c1" || i === "c2"));
  assert.deepEqual(ids(r.hiddenUnknown).sort(), ["c1", "c2"].filter((i) => ids(r.tasks).indexOf(i) < 0).sort());
});
test("凍結: 入力中は署名を更新せず、入力が終わると変化を検出して再計算する", () => {
  let last = "A";
  assert.equal(holdByTyping(true, "B", last), true);
  last = nextHeldSig(true, "B", last);
  assert.equal(last, "A");
  assert.equal(holdByTyping(false, "B", last), false); // 入力終了: 再計算
  last = nextHeldSig(false, "B", last);
  assert.equal(last, "B");
  assert.equal(holdByTyping(false, "B", last), true); // 変化なしは保持
});
test("親が作業中になった遷移: 子孫が作業中で既に作業中扱いでも署名が変わり、親が作業中のものが最上位(rank 0)", () => {
  const { cs, rec } = many(8);
  const working = new Set(["c0", "c3"]); // c0=子孫だけ作業中(履歴の古い子)、c3=別の作業中
  const rootActive = new Set<string>();
  const w = (c: any) => working.has(c.key.id);
  const ra = (c: any) => rootActive.has(c.key.id);
  const id = (c: any) => c.key.id;
  const before = workSignature(cs, id, w, ra);
  rootActive.add("c0"); // 状態不明だった親に初回送信が受理され、親が実行中になる（作業中であることは変わらない）
  assert.notEqual(workSignature(cs, id, w, ra), before);
  const r = selectTasks(snap(cs), rec as any, new Map(), w, 5, () => false, ra);
  assert.equal(ids(r.tasks)[0], "c0");
  assert.deepEqual(ids(orderWorkingFirst(cs, rec as any, w, ra)).slice(0, 2), ["c0", "c3"]);
});
test("親の作業中判定を渡さない従来の呼び出しは、既存の順位のまま", () => {
  const { cs, rec } = many(8);
  const r = selectTasks(snap(cs), rec as any, new Map(), (c) => c.key.id === "c0", 5, (c) => c.key.id === "c1");
  assert.equal(ids(r.tasks)[0], "c0");
});

test("isCompletedCard: 完了・終了済みで作業中でなく対応待ちもないときだけ1行", () => {
  assert.equal(isCompletedCard("done", false, 0), true);
  assert.equal(isCompletedCard("closed", false, 0), true);
  assert.equal(isCompletedCard("done", true, 0), false); // 子孫が作業中
  assert.equal(isCompletedCard("done", false, 1), false); // 対応待ち
  for (const st of ["running", "waiting", "interrupted", "failed", "unknown", "idle"] as const) assert.equal(isCompletedCard(st, false, 0), false);
  assert.equal(isCompletedCard(null, false, 0), false);
});
test("isCompletedCard: サブエージェントに失敗・中断・状態不明があれば1行にしない", () => {
  const k = (o: any) => ({ failed: 0, interrupted: 0, unknown: 0, ...o });
  assert.equal(isCompletedCard("done", false, 0, k({})), true);
  for (const o of [{ failed: 1 }, { interrupted: 1 }, { unknown: 1 }]) assert.equal(isCompletedCard("done", false, 0, k(o)), false);
});

test("completedKidsNote: 完了カードで子が未取得のときだけ「サブエージェント未確認」", () => {
  assert.equal(completedKidsNote(true, { notFetched: true }), "サブエージェント未確認");
  assert.equal(completedKidsNote(true, { notFetched: false }), null);
  assert.equal(completedKidsNote(false, { notFetched: true }), null);
});

// 実行: node --test src/ui/sendStatus.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { formatElapsed, queueCounts, sendStatusLines, slowNote, type StatusInput } from "./sendStatus.ts";
import type { PendingSend } from "./pendingSend.ts";

const p = (o: Partial<PendingSend> = {}): PendingSend => ({ id: "p1", text: "x", startedAt: 1000, steer: false, phase: null, outcome: null, ...o });
const input = (o: Partial<StatusInput> = {}): StatusInput => ({ pending: [], running: false, turnHasOutput: null, queue: null, ...o });
const ids = (i: StatusInput) => sendStatusLines(i).map((l) => l.id);

test("何も待っていなければ空", () => assert.deepEqual(sendStatusLines(input()), []));

test("送信直後は段階がなくても「送信しました」を出す（起点は送信時刻）", () => {
  const [l] = sendStatusLines(input({ pending: [p()] }));
  assert.equal(l.id, "send:p1");
  assert.match(l.text, /送信しました/);
  assert.equal(l.since, 1000);
  assert.equal(l.spin, true);
});

test("段階: 準備・再開・受理待ち、追加指示は文言が変わる", () => {
  assert.match(sendStatusLines(input({ pending: [p({ phase: "preparing" })] }))[0].text, /準備/);
  assert.match(sendStatusLines(input({ pending: [p({ phase: "resuming" })] }))[0].text, /再開/);
  assert.match(sendStatusLines(input({ pending: [p({ phase: "awaitingAcceptance" })] }))[0].text, /受理待ち/);
  assert.match(sendStatusLines(input({ pending: [p({ steer: true })], running: true, turnHasOutput: true }))[0].text, /追加指示を送信しました/);
});

test("受理不明は警告で、完了・失敗にしない", () => {
  const [l] = sendStatusLines(input({ pending: [p({ outcome: { kind: "unknown", attemptId: "a" } })] }));
  assert.equal(l.tone, "warn");
  assert.match(l.text, /受理を確認できません/);
  assert.match(l.text, /再送していません/);
});

test("受理後: 新しいturnは開始待ち、追加指示は受理の表示、未送信は出さない", () => {
  assert.match(sendStatusLines(input({ pending: [p({ outcome: { kind: "accepted", turnId: "t" } })] }))[0].text, /開始を待って/);
  assert.match(sendStatusLines(input({ pending: [p({ steer: true, outcome: { kind: "accepted", turnId: "t" } })], running: true, turnHasOutput: true }))[0].text, /追加指示を Codex が受理/);
  assert.deepEqual(ids(input({ pending: [p({ outcome: { kind: "rejected", attemptId: "a", message: "m" } })] })), []);
  assert.deepEqual(ids(input({ pending: [p({ outcome: { kind: "failed", message: "m" } })] })), []);
});

test("実行中: 出力前は応答待ち、出力後は処理中、取れないときは応答待ちにしない", () => {
  assert.deepEqual(ids(input({ running: true, turnHasOutput: false })), ["wait-first"]);
  assert.deepEqual(ids(input({ running: true, turnHasOutput: true })), ["working"]);
  assert.deepEqual(ids(input({ running: true, turnHasOutput: null })), ["working"]);
});

test("キュー: 登録済み・送信中・受理不明を区別する", () => {
  const q = queueCounts([{ state: { kind: "waiting" } }, { state: { kind: "waiting" } }, { state: { kind: "sending" } }, { state: { kind: "acceptanceUnknown" } }, { state: { kind: "cancelled" } }]);
  assert.deepEqual(q, { waiting: 2, sending: 1, unknown: 1 });
  assert.deepEqual(ids(input({ queue: q })), ["queue-unknown", "queue-sending", "queue-waiting"]);
  assert.equal(queueCounts([{ state: { kind: "sent" } }]), null);
  assert.equal(queueCounts(undefined), null);
});

test("長い無反応でも段階は残り、補足だけが付く", () => {
  assert.equal(slowNote("wait-first", 29_999), null);
  assert.match(slowNote("wait-first", 30_000) ?? "", /完了・失敗とは判断していません/);
  assert.match(slowNote("send:p1", 10_000) ?? "", /再送はしていません/);
  assert.equal(slowNote("working", 999_999), null);
  assert.deepEqual(ids(input({ running: true, turnHasOutput: false })), ["wait-first"]);
});

test("経過時間の表記", () => {
  assert.equal(formatElapsed(-5), "0秒");
  assert.equal(formatElapsed(59_999), "59秒");
  assert.equal(formatElapsed(61_000), "1分01秒");
});

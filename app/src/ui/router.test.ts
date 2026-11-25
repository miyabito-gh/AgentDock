// 実行: node --test src/ui/router.test.ts （tscの対象外）
import test from "node:test";
import assert from "node:assert/strict";
import { ROUTER_ENVELOPE_END, ROUTER_ENVELOPE_HEAD, baseText, canToggleOriginal, chipNeedsAttention, dispatchChip, dispatchesOfTurn, foldedLine, proposedProvider, providerLabel, resultBody, resultChip, noticeView, originalRequest, pickableChats, routerEnvelopeLabel, splitRouterEnvelope, targetLabel } from "./router.ts";

const wrap = (user: string) => `${ROUTER_ENVELOPE_HEAD}\n## 指示（第1版）\nx\n${ROUTER_ENVELOPE_END}\n${user}`;

test("資料つきの依頼は終端の後ろだけを利用者の文にする", () => {
  const s = splitRouterEnvelope(wrap("議事録を作って"));
  assert.equal(s.userText, "議事録を作って");
  assert.ok(s.envelope?.startsWith(ROUTER_ENVELOPE_HEAD) && s.envelope.endsWith(ROUTER_ENVELOPE_END));
});
test("利用者の文に終端と同じ文字列があっても、最初の終端で切る", () => {
  assert.equal(splitRouterEnvelope(wrap(`a\n${ROUTER_ENVELOPE_END}\nb`)).userText, `a\n${ROUTER_ENVELOPE_END}\nb`);
});
test("見出しがない・終端がない本文はそのまま", () => {
  assert.deepEqual(splitRouterEnvelope("ふつうの依頼"), { envelope: null, userText: "ふつうの依頼" });
  const noEnd = `${ROUTER_ENVELOPE_HEAD}\nだけ`;
  assert.deepEqual(splitRouterEnvelope(noEnd), { envelope: null, userText: noEnd });
});
test("資料の字数の表示", () => { assert.equal(routerEnvelopeLabel("あいう"), "資料（3字）"); });

const key = { backend: "codex", id: "c1" };
const dv = (o: Record<string, unknown> = {}) => ({
  id: "d1", sourceTurn: "t1", routeIndex: 0, target: { kind: "existing", chat: key }, suggested: { kind: "existing", chat: key },
  state: { kind: "proposed" }, reported: false, targetTitle: "週次議事録", suggestedTitle: "週次議事録", targetRunning: false, targetProblem: null,
  proposal: { text: "整えた文", textIsOriginal: false, reason: "r", confidence: "high", newChat: null }, ...o,
}) as never;

test("送り先の見出し: 既存は名前、新規は題名とフォルダ", () => {
  assert.equal(targetLabel(dv()), "週次議事録");
  assert.equal(targetLabel(dv({ targetTitle: null })), "送り先の会話");
  const n = dv({ target: { kind: "new", chat: null }, targetTitle: null, proposal: { text: "t", textIsOriginal: true, reason: "", confidence: "low", newChat: { title: "議事録", folder: "C:/work", purpose: "" } } });
  assert.equal(targetLabel(n), "新しいチャット：議事録（C:/work）");
  assert.equal(targetLabel(dv({ target: { kind: "new", chat: null }, proposal: null })), "新しいチャット");
});
test("送った後の1行は状態ごとの文。未確認は警告で、見送りは開けない", () => {
  assert.equal(foldedLine(dv()), null);
  assert.deepEqual(foldedLine(dv({ state: { kind: "sent", at: 1, clientMessageId: "m" } })), { text: "→『週次議事録』へ送りました", kind: "ok", canOpen: true });
  assert.equal(foldedLine(dv({ state: { kind: "queued", at: 1, queueEntry: "q" } }))?.text, "→『週次議事録』の順番待ちに入れました");
  const u = foldedLine(dv({ state: { kind: "unconfirmed", at: 1, clientMessageId: "m" } }));
  assert.equal(u?.text, "送れたか確認できません"); assert.equal(u?.kind, "warn");
  const x = foldedLine(dv({ state: { kind: "dismissed", at: 1 } }));
  assert.equal(x?.text, "見送りました"); assert.equal(x?.canOpen, false);
});
test("新規で作成前は開けない・作成後は開ける", () => {
  assert.equal(foldedLine(dv({ target: { kind: "new", chat: null }, targetTitle: null, state: { kind: "unconfirmed", at: 1, clientMessageId: "m" } }))?.canOpen, false);
  assert.equal(foldedLine(dv({ target: { kind: "new", chat: key }, targetTitle: "新", state: { kind: "sent", at: 1, clientMessageId: "m" } }))?.canOpen, true);
});
test("本文の既定は案の文。原文の切り替えは案が原文と違うときだけ", () => {
  assert.equal(baseText(dv(), false, "原文"), "整えた文");
  assert.equal(baseText(dv(), true, "原文"), "原文");
  assert.equal(baseText(dv(), true, null), "整えた文");
  assert.equal(canToggleOriginal(dv(), "原文"), true);
  assert.equal(canToggleOriginal(dv(), "  "), false);
  assert.equal(canToggleOriginal(dv({ proposal: { text: "原文", textIsOriginal: true, reason: "", confidence: "high", newChat: null } }), "原文"), false);
});
test("返答ごとのカードは routeIndex 順", () => {
  const v = { dispatches: [dv({ id: "b", routeIndex: 1 }), dv({ id: "a", routeIndex: 0 }), dv({ id: "z", sourceTurn: "t2" })] } as never;
  assert.deepEqual(dispatchesOfTurn(v, "t1").map((d) => d.id), ["a", "b"]);
  assert.deepEqual(dispatchesOfTurn(null, "t1"), []);
});
test("ルール更新の行: 取り消せる／取り消し済み／書かなかった", () => {
  assert.deepEqual(noticeView({ revision: 3, text: "ルールを更新：追加", undone: false }), { text: "ルールを更新：追加", canUndo: true, undone: false });
  assert.deepEqual(noticeView({ revision: 3, text: "ルールを更新：追加", undone: true }), { text: "ルールを更新：追加（取り消し済み）", canUndo: false, undone: true });
  assert.deepEqual(noticeView({ revision: null, text: "ルールは更新していません（理由）", undone: false }), { text: "ルールは更新していません（理由）", canUndo: false, undone: false });
});
test("送り先の候補はアーカイブ・組織所有・受付係自身を除き、名前とフォルダで絞る", () => {
  const mk = (id: string, o: Record<string, unknown> = {}) => ({ key: { backend: "codex", id }, archived: { kind: "value", value: false }, ...o }) as never;
  const chats = [mk("a"), mk("r"), mk("arc", { archived: { kind: "value", value: true } }), mk("org"), mk("loc"), mk("sup")];
  const locals: Record<string, unknown> = { org: { org: { released: false }, visibility: { kind: "visible" } }, loc: { visibility: { kind: "archived" } }, sup: { supersededBy: {}, visibility: { kind: "visible" } } };
  const title = (c: { key: { id: string } }) => `名前${c.key.id}`;
  const ids = (q: string) => pickableChats(chats, (c) => locals[(c as { key: { id: string } }).key.id] as never, { backend: "codex", id: "r" } as never, title as never, (c) => ((c as { key: { id: string } }).key.id === "a" ? "C:\\Work\\Minutes" : null), q).map((c) => (c as { key: { id: string } }).key.id);
  assert.deepEqual(ids(""), ["a"]);
  assert.deepEqual(ids("minutes"), ["a"]);
  assert.deepEqual(ids("work/min"), ["a"]);
  assert.deepEqual(ids("zzz"), []);
});
test("最初の利用者発話から原文を取り出す", () => {
  const e = [{ kind: { kind: "agentMessage" }, text: { kind: "value", value: "x" } }, { kind: { kind: "userMessage" }, text: { kind: "value", value: wrap("依頼") } }];
  assert.equal(originalRequest(e), "依頼");
  assert.equal(originalRequest([]), null);
});

test("担当の結果文から状態の印を引く（形＋文字で区別。知らない文は印なし）", () => {
  assert.deepEqual(resultChip("担当完了\n結果"), { tone: "done", icon: "check", label: "完了" });
  assert.equal(resultChip("担当完了（最終回答の記録なし）")?.label, "完了");
  assert.equal(resultChip("担当の実行が失敗しました")?.label, "失敗");
  assert.equal(resultChip("送信失敗: 理由")?.label, "送信失敗");
  assert.equal(resultChip("担当の実行が中断されました")?.label, "中断");
  assert.deepEqual(resultChip("担当への受理を照合中です"), { tone: "unk", icon: "help", label: "受理不明" });
  assert.equal(resultChip("キューは停止中です。担当チャットで詳細を確認してください")?.label, "要確認");
  assert.equal(resultChip("キュー記録を照合できません")?.label, "照合不能");
  assert.equal(resultChip("キューで取り消されました")?.label, "取り消し");
  assert.equal(resultChip("未知の文"), null);
  assert.equal(resultChip(null), null);
});
test("カードの印: 結果があれば結果、なければ送信の状態。見送りは印なし", () => {
  const q = { kind: "queued", at: 1, queueEntry: "q" };
  assert.deepEqual(dispatchChip(dv({ state: q })), { tone: "idle", icon: "hourglass", label: "送信待ち" });
  assert.equal(dispatchChip(dv({ state: q, executionResult: "送信失敗: x" }))?.label, "送信失敗");
  assert.equal(dispatchChip(dv({ state: { kind: "unconfirmed", at: 1, clientMessageId: "m" } }))?.label, "受理不明");
  assert.deepEqual(dispatchChip(dv({ state: { kind: "sent", at: 1, clientMessageId: "m" } })), { tone: "run", icon: "play", label: "送信済み" });
  // 結果文があるのに判定できないときは、送信の状態（送信待ち）に戻らず「状態不明」（完了を送信待ちと見せない）
  assert.deepEqual(dispatchChip(dv({ state: q, executionResult: "知らない文言" })), { tone: "unk", icon: "help", label: "状態不明" });
  assert.equal(dispatchChip(dv({ state: { kind: "dismissed", at: 1 } })), null);
  assert.equal(dispatchChip(dv()), null);
});
test("枠を警告色にするのは失敗・受理不明・要確認だけ。完了の1行目の語は本文から省く", () => {
  assert.equal(chipNeedsAttention(resultChip("送信失敗: x")), true);
  assert.equal(chipNeedsAttention(resultChip("担当への受理を照合中です")), true);
  assert.equal(chipNeedsAttention(resultChip("キューは停止中です")), true);
  assert.equal(chipNeedsAttention(resultChip("担当完了\n結果")), false);
  assert.equal(chipNeedsAttention(resultChip("担当の実行が中断されました")), false);
  assert.equal(chipNeedsAttention(null), false);
  assert.equal(resultBody("担当完了\n最終回答"), "最終回答");
  assert.equal(resultBody("担当完了（最終回答の記録なし）"), "担当完了（最終回答の記録なし）");
  assert.equal(resultBody("送信失敗: x\n2行目"), "送信失敗: x\n2行目");
});
test("カードの担当AI: 新規は案の provider（省略は Codex）、既存は実際のチャットのキー", () => {
  const nc = (provider?: string) => dv({ target: { kind: "new", chat: null }, proposal: { text: "t", textIsOriginal: true, reason: "", confidence: "high", newChat: { title: "x", folder: null, purpose: "", ...(provider ? { provider } : {}) } } });
  assert.equal(proposedProvider(nc("antigravity")), "antigravity");
  assert.equal(proposedProvider(nc()), "codex");
  assert.equal(proposedProvider(dv({ target: { kind: "existing", chat: { backend: "antigravity", id: "c1" } } })), "antigravity");
  assert.equal(proposedProvider(dv()), "codex");
  assert.equal(proposedProvider(dv({ target: { kind: "new", chat: null }, proposal: null })), "codex");
  assert.equal(providerLabel("antigravity"), "Antigravity（Gemini）");
  assert.equal(providerLabel("codex"), "Codex");
});

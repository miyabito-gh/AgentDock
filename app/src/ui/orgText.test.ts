// 実行: node --test src/ui/orgText.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { planSummary, orgSendLock, releaseMenuState, displayRequest, displayWorkflowTitle, finalReportTask, finishedBannerFor, PLANNER_REQUEST_NOTE, sectionVisibility, taskArtifactNote, taskSubparts, completedNoticeText, finishedBannerText, isInvestigationOnly, noReportHeadNote, reworkCellText, edgeText, reworkLimitText, verdictJa, verdictText, stopCard, stopRowNote } from "./orgText.ts";

test("周回の文言: 実行回数＝やり直し＋1。単位を分けて同じ数え方", () => {
  assert.equal(reworkLimitText(1, 3), "実行 2回目／最大 4回（やり直しは最大3回、残り2回）");
  assert.equal(reworkLimitText(3, 3), "実行 4回目／最大 4回（やり直しは最大3回、残り0回）");
  assert.equal(reworkLimitText(5, 3), "実行 6回目／最大 4回（やり直しは最大3回、残り0回）");
});

test("判定値の訳: 既知は日本語、未知はそのまま", () => {
  assert.equal(verdictJa("rework"), "やり直し");
  assert.equal(verdictJa("pass"), "合格");
  assert.equal(verdictJa("replan"), "計画からやり直し");
  assert.equal(verdictJa("custom"), "custom");
  assert.equal(verdictText({ kind: "value", text: "pass" }), "合格");
  assert.equal(verdictText({ kind: "pending" }), "—");
});

test("辺: 表示名で出し、IDは title へ", () => {
  const e = { from: "verify", to: "build", kind: "feedback", label: "判定 = やり直し", used: 1, allowed: 3 } as never;
  const x = edgeText(e, (id) => ({ build: "実装" })[id] ?? id);
  assert.equal(x.head, "↺ 実装");
  assert.equal(x.title, "verify->build");
  assert.match(x.detail, /やり直し 最大3回・使用1回・残り2回/);
});

test("差し戻し列: 初回を数えず、差し戻し回数／上限", () => {
  assert.equal(reworkCellText(1, 3), "0/2");
  assert.equal(reworkCellText(2, 3), "1/2");
  assert.equal(reworkCellText(3, 3), "2/2");
  assert.equal(reworkCellText(0, 3), "0/2");
});

test("停止理由: 変更なしで要旨が無いときだけ補足", () => {
  assert.equal(noReportHeadNote("Workerが変更を書きませんでした。必要なら"), "報告の要旨は記録されていません");
  assert.equal(noReportHeadNote("Workerが変更を書きませんでした（報告の要旨: 調べた）。"), null);
  assert.equal(noReportHeadNote("コミットできませんでした"), null);
  assert.equal(noReportHeadNote(null), null);
});

test("調査のみ: 変更タスクが無いときだけ取り込みの案内を出さない", () => {
  assert.equal(isInvestigationOnly([{ kind: "investigate" }, { kind: "investigate" }]), true);
  // 役割のタスク（Reviewer 等）が変更種別でも、コミットが無ければ調査のみ
  assert.equal(isInvestigationOnly([{ kind: "investigate" }, { kind: "change" }]), true);
  assert.equal(isInvestigationOnly([{ kind: "investigate", commit: null }, { kind: "change", commit: "abc" }]), false);
  assert.equal(isInvestigationOnly([{ kind: "change", commit: null }]), false, "調査報告が無ければ従来文言");
  assert.equal(isInvestigationOnly([]), false);
  assert.ok(finishedBannerText([{ kind: "investigate" }]).includes("調査報告"));
  assert.ok(finishedBannerText([{ kind: "change" }]).includes("取り込んで"));
});

test("完了通知の文も調査のみで切り替わる", () => {
  assert.ok(completedNoticeText([{ kind: "investigate" }]).includes("取り込む変更はありません"));
  assert.ok(completedNoticeText([{ kind: "change", commit: "a" }]).includes("取り込んでください"));
});

test("名前の整形: 先頭の見出し記号だけ除く", () => {
  assert.equal(displayWorkflowTitle("# 依頼（依頼整理で確定・第1版）"), "依頼（依頼整理で確定・第1版）");
  assert.equal(displayWorkflowTitle("調べる # 1"), "調べる # 1");
  assert.equal(displayWorkflowTitle(null), null);
  assert.equal(displayWorkflowTitle("#  "), null);
});

test("依頼の整形: 前置きの行を除く（完全一致・先頭一致）。他は残す", () => {
  const body = `# 依頼（依頼整理で確定・第1版）\n${PLANNER_REQUEST_NOTE}\n\n## 目的\n構成を調べる`;
  assert.equal(displayRequest(body), "依頼（依頼整理で確定・第1版）\n\n## 目的\n構成を調べる");
  assert.equal(displayRequest("以下は依頼整理を終えた確定依頼である。別の言い方\n本文"), "本文");
  assert.equal(displayRequest("ふつうの依頼"), "ふつうの依頼");
  assert.equal(displayRequest(PLANNER_REQUEST_NOTE), PLANNER_REQUEST_NOTE, "残りが空なら隠さない");
});

test("終了後の出し分け", () => {
  assert.deepEqual(sectionVisibility(true), { activity: false, waitBand: false, settingsFolded: true });
  assert.deepEqual(sectionVisibility(false), { activity: true, waitBand: true, settingsFolded: false });
});

test("成果物の補足: 報告あり／変更あり。取れていないものを「なし」で埋めない", () => {
  assert.equal(taskArtifactNote({ kind: "investigate", phase: "completed" }, "value"), "報告あり");
  assert.equal(taskArtifactNote({ kind: "investigate", phase: "completed" }, "notObtained"), "報告 未取得");
  assert.equal(taskArtifactNote({ kind: "investigate", phase: "running" }, "none"), null);
  assert.equal(taskArtifactNote({ kind: "investigate", phase: "completed" }, "unknown"), null);
  assert.equal(taskArtifactNote({ kind: "change", phase: "completed", commit: "0123456789" }, "none"), "変更あり（01234567）");
  assert.equal(taskArtifactNote({ kind: "change", phase: "completed" }, "none"), null);
});

test("状態の下の補足行: 値があるものだけ", () => {
  const base = { reworkUsed: 0, reworkMax: 2, retriesUsed: 0, retryMax: 3, hasRetry: false, waitingOn: [], artifact: null };
  assert.deepEqual(taskSubparts(base), []);
  assert.deepEqual(taskSubparts({ ...base, reworkUsed: 1, retriesUsed: 1, waitingOn: ["planner"] }), ["差し戻し 1/2", "再送 1/3", "待ち: planner"]);
  assert.deepEqual(taskSubparts({ ...base, hasRetry: true }), ["再送 0/3"]);
});

test("結果にする報告: 他の依存先になっていない末端の調査で、報告を読めた最後のもの", () => {
  const tasks = [
    { id: "a", kind: "investigate", deps: [] }, { id: "b", kind: "investigate", deps: [] },
    { id: "report", kind: "investigate", deps: ["a", "b"] }, { id: "rev", kind: "change", deps: ["report"] },
  ];
  assert.equal(finalReportTask(tasks, new Set(["a", "b", "report"])), "report");
  assert.equal(finalReportTask(tasks, new Set(["a", "b"])), null, "末端の報告を読めなければ個別の報告を結果にしない");
  assert.equal(finalReportTask([{ id: "x", kind: "investigate", deps: [] }], new Set(["x"])), null, "統合のない1件は結果にしない");
  const par = ["a", "b", "c", "d"].map((id) => ({ id, kind: "investigate", deps: [] as string[] }));
  assert.equal(finalReportTask(par, new Set(["a", "b", "c", "d"])), null, "並列の調査（統合なし）は結果にしない");
  const two = [...tasks, { id: "report2", kind: "investigate", deps: ["a"] }];
  assert.equal(finalReportTask(two, new Set(["a", "b", "report", "report2"])), null, "統合が複数なら結果にしない");
  assert.equal(finalReportTask([], new Set()), null);
});

test("完了の帯: 過去の記録で取り込みを促さない／調査のみは調査用", () => {
  assert.ok(!finishedBannerFor([{ kind: "change", commit: "a" }], true).includes("取り込んでください"));
  assert.ok(finishedBannerFor([{ kind: "change", commit: "a" }], false).includes("取り込んでください"));
  assert.ok(finishedBannerFor([{ kind: "investigate" }], true).includes("取り込む変更はありません"));
  assert.ok(!finishedBannerFor([{ kind: "investigate" }], false).includes("取り込んでください"));
});

test("手動に切り替え: 終了後も出す／過去の記録は無効＋理由／ロック文は使えるときだけ手順を案内", () => {
  assert.deepEqual(releaseMenuState(false, false), { show: false, disabled: false, reason: null });
  assert.deepEqual(releaseMenuState(true, false), { show: true, disabled: false, reason: null });
  const ro = releaseMenuState(true, true);
  assert.ok(ro.show && ro.disabled && ro.reason);
  assert.ok(orgSendLock(true).includes("手動に切り替え"));
  assert.ok(!orgSendLock(false).includes("手動に切り替え"));
  assert.ok(orgSendLock(false).includes("見るだけ"));
  assert.ok(!orgSendLock(true).includes("turn"));
});

test("計画の1行: 終了後の完了した planner だけ表から外し、承認の別は通知から取れるときだけ", () => {
  const k = { id: "c", kind: "x" } as unknown as import("../ipc/types").ChatKey;
  const t = (id: string, role: string, phase: string, chat: typeof k | null = null) => ({ id, role, chat, status: { phase } });
  const tasks = [t("p", "planner", "completed", k), t("w", "worker", "completed")];
  const a = planSummary(tasks, true, ["planAutoApproved"]);
  assert.equal(a.line, "計画: 承認済み（自動）"); assert.ok(a.hiddenIds.has("p")); assert.equal(a.chat, k);
  assert.equal(planSummary(tasks, true, ["planNeedsApproval"]).line, "計画: 承認済み（利用者）");
  assert.equal(planSummary(tasks, true, ["planNeedsApproval", "planAutoApproved"]).line, "計画: 承認済み（自動）");
  assert.equal(planSummary(tasks, true, []).line, "計画: 承認済み", "取れなければ別を書かない");
  assert.equal(planSummary(tasks, false, ["planAutoApproved"]).line, null, "実行中・停止中は表に残す");
  assert.equal(planSummary([t("p", "planner", "failed"), t("w", "worker", "completed")], true, []).line, null, "失敗した計画は残す");
  assert.equal(planSummary([t("p", "planner", "completed")], true, []).line, null, "計画しか無いときは残す");
});

const stp = (kind: string, over: Record<string, unknown> = {}) => ({ kind, headline: "見出し", facts: ["差し戻し 2/2"], evidence: [], actions: ["sendBack", "acceptAsIs", "fail", "openChat", "history"], hint: "次の一手", unconfirmed: false, ...over });
const stt = (id: string, stop: unknown, over: Record<string, unknown> = {}) => ({ id, title: `title ${id}`, stop, chat: { id: "c" }, sendBackRefusal: null, ...over }) as never;

test("止まっているカード: 止まりが無ければ null、複数は先頭1件と残り件数", () => {
  assert.equal(stopCard([stt("a", null)], false, 0, () => false), null);
  assert.equal(stopCard([stt("a", stp("reworkLimit"))], true, 0, () => false), null, "終了後は出さない");
  const c = stopCard([stt("a", stp("reworkLimit")), stt("b", stp("turnFailed"))], false, 0, () => false)!;
  assert.equal(c.task, "a");
  assert.equal(c.others, 1);
  assert.equal(c.heading, "止まっています: 見出し");
  assert.deepEqual(c.buttons.map((b) => b.key), ["sendBack", "acceptAsIs", "fail", "openChat", "history"]);
});

test("止まっているカード: 3択は常に並び、選べないものは理由つきで無効", () => {
  const c = stopCard([stt("a", stp("checkFailed", { actions: ["acceptAsIs", "history"] }), { sendBackRefusal: "理由X" })], false, 0, () => false)!;
  const sb = c.buttons.find((b) => b.key === "sendBack")!;
  assert.equal(sb.enabled, false);
  assert.equal(sb.title, "理由X");
  assert.equal(c.buttons.find((b) => b.key === "fail")!.enabled, false);
  assert.equal(c.buttons.find((b) => b.key === "acceptAsIs")!.enabled, true);
});

test("止まっているカード: 受理不明・通信断は『止まった』『失敗』と書かず、手動切替も出さない", () => {
  const c = stopCard([stt("a", stp("needsReconcile", { headline: "通信が途切れたため、いまの状態を確認できていません", unconfirmed: true, actions: ["cancel", "openChat", "history"] }))], false, 0, () => true)!;
  assert.equal(c.unconfirmed, true);
  assert.ok(!/止まっています|失敗/.test(c.heading), c.heading);
  assert.ok(!c.buttons.some((b) => b.key === "release"));
  assert.equal(c.buttons.at(-3)!.key, "cancel");
});

test("止まっているカード: 承認・質問だけの待ち、停止があればその後ろに数える", () => {
  const a = stopCard([stt("a", null)], false, 2, () => false)!;
  assert.equal(a.kind, "approval");
  assert.deepEqual(a.buttons.map((b) => b.key), ["answer"]);
  assert.equal(stopCard([stt("a", stp("manual"))], false, 1, () => false)!.others, 1);
});

test("止まっているカード: 手がかりは3項目まで、行の補足は見出しの短縮（通知は120字まで）", () => {
  const ev = [1, 2, 3, 4].map((n) => ({ label: `e${n}`, lines: ["x"], known: true, truncated: false }));
  assert.equal(stopCard([stt("a", stp("reworkLimit", { evidence: ev }))], false, 0, () => false)!.evidence.length, 3);
  assert.equal(stopRowNote(stp("turnFailed") as never, null), "止まった理由: 見出し");
  assert.ok(stopRowNote(stp("turnFailed") as never, "あ".repeat(200)).includes("…"));
  assert.equal(stopRowNote(stp("sendUnknown", { headline: "届いたか確認できていません", unconfirmed: true }) as never, null), "届いたか確認できていません");
});

test("止まっているカード: 待ち先が止まっているだけのタスクは、操作できる止まりより後ろ", () => {
  const dep = stt("w2", stp("dependencyBlocked", { actions: ["history"] }));
  const act = stt("w1", stp("turnFailed", { actions: ["retryNew", "fail", "history"] }));
  const c = stopCard([dep, act], false, 0, () => false)!;
  assert.equal(c.task, "w1");
  assert.equal(c.others, 1);
  assert.equal(stopCard([dep], false, 0, () => false)!.task, "w2");
  const view = stt("w3", stp("needsReconcile", { actions: ["cancel", "history"] }));
  assert.equal(stopCard([view, act], false, 0, () => false)!.task, "w1", "見るだけ・取消しだけの止まりも、実行系の操作があるものより後ろ");
});

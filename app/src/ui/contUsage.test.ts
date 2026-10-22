// 実行: node --test src/ui/contUsage.test.ts （tscの対象外）
import { test } from "node:test";
import assert from "node:assert/strict";
import { charCount, draftIneligible, handoffIneligible, ledgerText, noticeText, observedText, purposeFirstLine, purposeSaveBlock, shouldSuggest, usageChip } from "./contUsage.ts";

const known = (percent: number, live = true) => ({ kind: "known", used: 1234, window: 200000, percent, observedAt: new Date(2026, 9, 18, 14, 2).getTime(), live }) as never;
const NOW = new Date(2026, 9, 18, 15, 0).getTime();

test("usageChip distinguishes every kind and never invents a number", () => {
  assert.equal(usageChip(known(62), 70, NOW).text, "文脈 62%");
  assert.match(usageChip(known(62), 70, NOW).tip, /使用 1,234 \/ 上限 200,000 トークン、14:02 の応答時点/);
  assert.equal(usageChip(known(62, false), 70, NOW).text, "文脈 62%（前回）");
  assert.equal(usageChip(known(62, false), 70, NOW).tone, "dim");
  assert.equal(usageChip({ kind: "windowUnknown", used: 5, observedAt: 1 } as never, 70, NOW).text, "文脈 未取得");
  assert.equal(usageChip({ kind: "notFetched" } as never, 70, NOW).text, "文脈 未取得");
  assert.equal(usageChip({ kind: "afterCompaction", observedAt: 1 } as never, 70, NOW).text, "文脈 圧縮後・未取得");
  assert.equal(usageChip({ kind: "unsupported" } as never, 70, NOW).text, "文脈 非対応");
});

test("usageChip tone follows the threshold, 90% is strong, no threshold means no warn", () => {
  assert.equal(usageChip(known(69), 70, NOW).tone, "normal");
  assert.equal(usageChip(known(70), 70, NOW).tone, "warn");
  assert.equal(usageChip(known(90), 70, NOW).tone, "strong");
  assert.equal(usageChip(known(80), null, NOW).tone, "normal");
  assert.equal(usageChip(known(95), null, NOW).tone, "strong");
  assert.equal(usageChip(known(120), 70, NOW).text, "文脈 120%");
});

test("shouldSuggest: only Known live at or above the threshold", () => {
  assert.equal(shouldSuggest(known(70), 70, null), true);
  assert.equal(shouldSuggest(known(69), 70, null), false);
  assert.equal(shouldSuggest(known(80, false), 70, null), false);
  assert.equal(shouldSuggest({ kind: "afterCompaction", observedAt: 1 } as never, 70, null), false);
  assert.equal(shouldSuggest({ kind: "windowUnknown", used: 1, observedAt: 1 } as never, 70, null), false);
  assert.equal(shouldSuggest(known(80), null, null), false);
});

test("shouldSuggest: declining silences it until 90% is reached", () => {
  assert.equal(shouldSuggest(known(75), 70, 72), false);
  assert.equal(shouldSuggest(known(89), 70, 72), false);
  assert.equal(shouldSuggest(known(90), 70, 72), true);
  assert.equal(shouldSuggest(known(95), 70, 91), false);
});

test("handoffIneligible: org, superseded and worktree each give a reason", () => {
  assert.equal(handoffIneligible({ orgOwned: false, supersededBy: false, worktree: false }), null);
  assert.match(handoffIneligible({ orgOwned: true, supersededBy: false, worktree: false })!, /組織実行/);
  assert.match(handoffIneligible({ orgOwned: false, supersededBy: true, worktree: false })!, /引き継ぎ済み/);
  assert.match(handoffIneligible({ orgOwned: false, supersededBy: false, worktree: true })!, /worktree/);
});

test("purposeSaveBlock: limit counts characters, empty is allowed, unchanged is blocked", () => {
  assert.equal(charCount("𠮷a"), 2);
  assert.equal(purposeSaveBlock("a".repeat(4000), "", false), null);
  assert.match(purposeSaveBlock("a".repeat(4001), "", false)!, /4,000 字まで/);
  assert.equal(purposeSaveBlock("", "メモ", false), null);
  assert.equal(purposeSaveBlock("メモ", "メモ", false), "変更がありません");
  assert.match(purposeSaveBlock("x", "", true)!, /組織実行/);
});

test("purposeFirstLine skips blank lines and truncates", () => {
  assert.equal(purposeFirstLine("\n\n  議事録を作る  \n2行目"), "議事録を作る");
  assert.equal(purposeFirstLine("  \n "), null);
  assert.equal(purposeFirstLine("a".repeat(100), 10), `${"a".repeat(10)}…`);
});

test("noticeText and observedText", () => {
  assert.equal(noticeText("threshold", 72, 3), "文脈が 72% です。次の送信で新しい thread へ引き継ぎます（目的メモ＋直近3往復）");
  assert.equal(noticeText("manual", null, 3), "次の送信で新しい thread へ引き継ぎます（目的メモ＋直近3往復）");
  assert.equal(observedText(new Date(2026, 9, 17, 9, 5).getTime(), NOW), "10/17 09:05");
  assert.equal(observedText(0, NOW), null);
});

test("ledgerText: プレビューは台帳を書かない旨と、有無・書込み予定・書けない場合を区別する", () => {
  const base = { tocRounds: 12, ledgerPath: "C:/x/会話台帳.md", ledgerExists: false, ledgerWillWrite: true };
  assert.match(ledgerText(base), /目次 12 往復.*押すと.*書き出します.*まだありません/);
  assert.match(ledgerText({ ...base, ledgerExists: true }), /更新します/);
  assert.match(ledgerText({ ...base, ledgerWillWrite: false, ledgerExists: true }), /あります。書き直しません/);
  assert.match(ledgerText({ ...base, ledgerWillWrite: false }), /見つかりません/);
  assert.match(ledgerText({ ...base, ledgerPath: null }), /付きません/);
});

test("draftIneligible: 画面で分かる理由だけ返す（作業中・不明は止めない）", () => {
  const ok = { orgOwned: false, supersededBy: false, structuredOutput: "supported" as const };
  assert.equal(draftIneligible(ok), null);
  assert.equal(draftIneligible({ ...ok, structuredOutput: "unknown" }), null);
  assert.equal(draftIneligible({ ...ok, structuredOutput: undefined }), null);
  assert.match(draftIneligible({ ...ok, orgOwned: true })!, /組織実行/);
  assert.match(draftIneligible({ ...ok, supersededBy: true })!, /以前の thread/);
  assert.match(draftIneligible({ ...ok, structuredOutput: "unsupported" })!, /構造化出力/);
});

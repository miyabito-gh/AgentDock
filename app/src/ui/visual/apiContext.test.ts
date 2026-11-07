import { test } from "node:test";
import assert from "node:assert/strict";
import { VISUAL_CONTEXT_MARK, directRefusal, friendlyVisualError, visualSendContext, splitVisualContext, parseDirectTurn } from "./api.ts";

// 実行: node --test src/ui/visual/apiContext.test.ts （tscの対象外）
test("送信文脈は直接編集の新文言で、編集用コピー・反映の記述を含まない", () => {
  const ctx = visualSendContext("C:/site/index.html", [{ tag: "h1", id: "", className: "", text: "題", selector: "h1", rect: { x: 0, y: 0, width: 1, height: 1 }, leaf: true }],
    [{ path: "index.html", access: "edit" }, { path: "lib.js", access: "readOnly" }]);
  assert.ok(ctx.sel.includes("編集対象: C:/site/index.html（元のファイルを直接編集します。変更は戻せます）"));
  assert.ok(ctx.sel.startsWith(VISUAL_CONTEXT_MARK));
  assert.ok(!/編集用コピー|反映/.test(ctx.sel + ctx.fixed));
  assert.ok(ctx.fixed.includes("編集してよいファイル: index.html"));
  assert.equal(splitVisualContext("依頼\n\n" + ctx.sel).count, 1);
});
test("拒否文言の先頭で conflict / running / nothing を判別する", () => {
  assert.deepEqual(directRefusal("conflict:\na.html\nb.css"), { kind: "conflict", paths: ["a.html", "b.css"] });
  assert.equal(directRefusal("running: busy")?.kind, "running");
  assert.equal(directRefusal("nothing:")?.kind, "nothing");
  assert.equal(directRefusal("other error"), null);
  assert.equal(friendlyVisualError("other error"), "other error");
});
test("ターンの応答は厳密に検査する", () => {
  const ok = { id: "t-0001", seq: 1, atMs: 1, state: "done", files: [{ path: "a.html", change: "change" }], canUndo: true, canRedo: false };
  assert.equal(parseDirectTurn(ok).files[0].path, "a.html");
  assert.throws(() => parseDirectTurn({ ...ok, state: "x" }));
  assert.throws(() => parseDirectTurn({ ...ok, files: [{ path: "a", change: "?" }] }));
  assert.throws(() => parseDirectTurn({ ...ok, canUndo: "yes" }));
});

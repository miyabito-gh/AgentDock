// 差分の表示部品（変更を確認・組織実行の成果物パネルで共有）。本文は加工しない。取れていない値は「未確認」と書き、0や空で代用しない。
import type { ChangeKind, Known } from "../ipc/types";
import { showKnown } from "./format";
import { DIFF_ROW_LIMIT, diffRows } from "./changesView";

/** 差分の一覧の1ファイル。`ChangedFile`（このチャットの変更）と `OrgArtifactFile`（成果物）の両方が構造的に合う。 */
export interface DiffFileLike {
  path: string;
  kind: ChangeKind;
  moveTo: string | null;
  additions: Known<number>;
  deletions: Known<number>;
  binary: boolean;
}

/** 統一diffの表示。行番号（旧・新）と記号の列つき。本文は加工しない。巨大な差分は先頭だけ。 */
export function DiffText({ text }: { text: string }) {
  const rows = diffRows(text);
  const shown = rows.length > DIFF_ROW_LIMIT ? rows.slice(0, DIFF_ROW_LIMIT) : rows;
  return (
    <div className="diffview" role="region" aria-label="差分" tabIndex={0}>
      {shown.map((r, i) => (
        <div key={i} className={`ln ${r.kind}`}>
          <span className="no" aria-hidden="true">{r.old ?? ""}</span><span className="no" aria-hidden="true">{r.new ?? ""}</span>
          <span className="sg" aria-hidden="true">{r.kind === "add" ? "+" : r.kind === "del" ? "−" : ""}</span>
          <span className="tx">{r.kind === "hunk" || r.kind === "meta" ? r.text : r.text === "" ? " " : r.text}</span>
        </div>))}
      {rows.length > shown.length ? <div className="ln meta"><span className="tx">先頭 {shown.length} 行のみ表示しています（全 {rows.length} 行）。</span></div> : null}
    </div>
  );
}

/** 種類アイコン（A/M/D/R）。色は追加=緑、削除=赤、変更=青、移動=灰。 */
export function kindIcon(f: Pick<DiffFileLike, "kind" | "moveTo">): { letter: string; cls: string; title: string } {
  if (f.moveTo) return { letter: "R", cls: "mov", title: "移動" };
  return f.kind === "added" ? { letter: "A", cls: "add", title: "追加" } : f.kind === "deleted" ? { letter: "D", cls: "del", title: "削除" } : { letter: "M", cls: "mod", title: "変更" };
}

export const pathText = (f: Pick<DiffFileLike, "path" | "moveTo">) => `${f.path}${f.moveTo ? ` → ${f.moveTo}` : ""}`;

/** 追加・削除の行数。バイナリは「バイナリ」、数えられないものは「未確認」。 */
export function Counts({ f }: { f: Pick<DiffFileLike, "additions" | "deletions" | "binary"> }) {
  if (f.binary) return <span className="chg-num muted">バイナリ</span>;
  return <span className="chg-num"><span className="chg-add">{showKnown(f.additions, (v) => `+${v}`)}</span> <span className="chg-del">{showKnown(f.deletions, (v) => `−${v}`)}</span></span>;
}

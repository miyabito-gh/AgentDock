// 子エージェントのモデル表示（記録値）。短い印とツールチップの文を作る純粋関数。
// 規則: 値はスレッドに記録されたもので、いま使用中とは限らない。取れていないものは「未確認」と書き、空や既定値で代用しない。
// 型は ipc/types のものだけを使う（実行時の import なし）。
import type { Known, SpawnSpec } from "../ipc/types";

const EFFORT_CHAR: Record<string, string> = { none: "無", minimal: "最小", low: "低", medium: "中", high: "高", xhigh: "最高" };

/** モデル名の短縮。末尾の識別語（`gpt-6.1-sol` → `Sol`）。英字だけの語でなければ先頭8文字。 */
export function shortModel(model: string): string {
  const m = model.trim();
  const last = m.split("-").pop() ?? "";
  if (/^[A-Za-z]{2,}$/.test(last) && m.includes("-")) {
    const w = last.length > 8 ? last.slice(0, 8) : last;
    return w[0].toUpperCase() + w.slice(1);
  }
  return m.slice(0, 8);
}

/** 推論強度の1字（程度）。未知の値は先頭4文字。 */
export function effortChar(effort: string): string {
  const e = effort.trim().toLowerCase();
  return EFFORT_CHAR[e] ?? effort.trim().slice(0, 4);
}

const valueOf = (k: Known<string>): string | null => (k.kind === "value" && k.value.trim() !== "" ? k.value : null);

/** カードに出す短い印（例 `Sol·高`）。モデルが取れていなければ null（印を出さない）。強度が無ければモデルだけ。 */
export function modelBadge(model: Known<string>, effort: Known<string>): string | null {
  const m = valueOf(model);
  if (m === null) return null;
  const e = valueOf(effort);
  return e === null ? shortModel(m) : `${shortModel(m)}·${effortChar(e)}`;
}

const absent = (k: Known<string>): string => (k.kind === "unsupported" ? "非対応" : "未確認");

/** 親の履歴に記録された起動時の指定の1行。親が未読込みなど確定できないときは「未確認」（空や既定で代用しない）。 */
export function spawnSpecLine(spec: Known<SpawnSpec> | undefined): string {
  if (!spec || spec.kind !== "value") return "起動時の指定: 未確認";
  const v = spec.value;
  const parts = [
    v.taskName ? `名前 ${v.taskName}` : null,
    v.model ? `モデル ${v.model}` : null,
    v.effort ? `推論強度 ${v.effort}` : null,
    v.forkTurns ? `引き継ぎ ${v.forkTurns}` : null,
  ].filter((x): x is string => x !== null);
  return parts.length === 0 ? "起動時の指定: 未確認" : `起動時の指定（記録値）: ${parts.join("・")}`;
}

/** ツールチップの全文。記録値であることを必ず添える。`spawn` を渡すと起動時の指定を続ける。 */
export function modelTooltip(model: Known<string>, effort: Known<string>, spawn?: Known<SpawnSpec>): string {
  const m = valueOf(model);
  const e = valueOf(effort);
  const tail = spawn === undefined ? "" : `
${spawnSpecLine(spawn)}`;
  if (m === null && e === null) return `モデル: ${absent(model)}${tail}`;
  const parts = [`モデル: ${m ?? absent(model)}`, `推論強度: ${e ?? absent(effort)}`];
  return `${parts.join("・")}（記録値。いま使用中とは限りません）${tail}`;
}

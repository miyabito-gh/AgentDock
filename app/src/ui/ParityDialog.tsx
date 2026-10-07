// 「同等性の確認状況」ダイアログの本体（段階③ P3-0）。操作ごとの経路・対応・確認状況の一覧（台帳）。
// 規則: 取れていない情報は「未取得」と書く。未確認・未実装を成功のように見せない。
import type { OpCapability, ParityOp } from "../ipc/types";
import { OP_LABEL, ROUTE_TEXT, SUPPORT_TEXT, VERIFY_TEXT } from "./commands";

const ORDER = Object.keys(OP_LABEL) as ParityOp[];

export function ParityBody({ caps }: { caps: OpCapability[] }) {
  if (caps.length === 0) {
    return <div className="content"><p>この AI の対応状況を取得できていません（接続前、または取得できていません）。</p></div>;
  }
  const by = new Map(caps.map((c) => [c.op, c]));
  return (
    <div className="content">
      <p className="small muted">VS Code 拡張と同じ操作を、AgentDock から使えるかの一覧です。「未確認」は、実際の Codex で動作を確認していない操作で、結果は受け付けた事実と観測した内容だけを表示します。</p>
      <table className="parity-table">
        <thead><tr><th>操作</th><th>経路</th><th>対応</th><th>確認状況</th><th>備考</th></tr></thead>
        <tbody>
          {ORDER.map((op) => {
            const c = by.get(op);
            return (
              <tr key={op}>
                <td>{OP_LABEL[op]}</td>
                {c ? (
                  <>
                    <td>{ROUTE_TEXT[c.route]}</td>
                    <td>{SUPPORT_TEXT[c.support]}</td>
                    <td>{c.verification === "unverified" ? <span className="tag unv">{VERIFY_TEXT[c.verification]}</span> : VERIFY_TEXT[c.verification]}</td>
                    <td className="small">{c.deprecated ? "非推奨。" : ""}{c.note ?? ""}</td>
                  </>
                ) : <td colSpan={4} className="small muted">未取得</td>}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

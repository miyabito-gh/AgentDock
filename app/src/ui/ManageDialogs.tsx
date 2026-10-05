// 削除確認・Markdownエクスポート・「保存と容量」の本体（P7）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 取得できていない値は「未確認」と書き、0で代用しない。保存・削除の成功は、ホストの結果を確認できたときだけ言う。
import type { Chat, ChatLocalView, DeleteOutcome, DeletePendingReason, DeletePreview, UsageBreakdown, UsageReport } from "../ipc/types";
import { chatName } from "./derive";
import { showKnown } from "./format";

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

const breakdownTotal = (b: UsageBreakdown): number => b.attachments + b.artifacts + b.workspace + b.activity + b.metadata;

/** 削除確認の状態。preview が null の間は取得中（error があれば取得失敗）。 */
export interface DeleteProps {
  preview: DeletePreview | null; error: string | null; running: boolean;
  /** 実行後の結果（deleted 以外のとき、ダイアログに残して理由を示す）。 */
  outcome: DeleteOutcome | null;
  /** 前回までの保留（再起動後も残る）。 */
  local: ChatLocalView | undefined;
  run: () => void;
}

export const PENDING_TEXT = (r: DeletePendingReason): string => {
  switch (r.kind) {
    case "stopUnconfirmed": return "作業の停止を確認できていないため、削除を保留しています。会話・添付・成果物はそのまま残っています。停止を確認できるまで、このチャットへの新しい送信は止めています。";
    case "ownershipUnknown": return "このアプリが起動した実行かどうかを確認できないため、削除を保留しています。会話・添付・成果物はそのまま残っています。";
    case "partialFailure": return "一部の削除に失敗しています。完了とは扱っていません。もう一度「削除」を選ぶと、残りをやり直します。";
    case "readyForUserRetry": return "停止を確認できました。もう一度「削除」を選ぶと削除します（自動では削除しません）。";
  }
};

export function DeleteBody({ chat, d }: { chat: Chat | undefined; d: DeleteProps }) {
  const pv = d.preview;
  const name = chat ? chatName(chat) : "（名前を確認できないチャット）";
  const pending = d.local?.deletePending ?? null;
  const out = d.outcome;
  return (
    <div className="content">
      <p><b>{name}</b> を削除します。</p>
      {d.error ? <p style={{ color: "var(--fail)" }}>{d.error}</p> : !pv ? <p>削除の対象を確認しています…</p> : (
        <>
          <p>削除するもの:</p>
          <ul className="check-list">
            {pv.deletesBackendHistory
              ? <li>会話の履歴（Codex の保存履歴）と、関連する子・孫の履歴{pv.descendants.kind === "value" ? `（取得済みの子・孫 ${pv.descendants.value} 件）` : "（子・孫の数は未確認）"}</li>
              : <li>AgentDock の一覧からの表示と、このチャットの補足情報（ピン・下書き・キュー）</li>}
            <li>このチャット用に保存した添付のコピー {pv.attachments} 件</li>
            <li>チャット専用領域内の成果物 {pv.artifactsInChatArea} 件（領域の大きさ: {showKnown(pv.chatAreaBytes, fmtBytes)}）</li>
          </ul>
          {pv.deletesBackendHistory ? null : (
            <p className="small muted">外部（CLI・VS Code など）で作成された会話です。元の履歴ファイルは参照扱いのため、削除しません。</p>
          )}
          <p>削除しないもの:</p>
          <ul className="check-list">
            <li>添付の元のファイル、別の場所に保存したファイル</li>
            <li>作業フォルダ・プロジェクト内のファイル</li>
            <li>他のチャットのファイル</li>
          </ul>
          {pv.requiresStop ? (
            <p style={{ color: "var(--fail)" }}>作業中、または停止を確認できていません。作業を中断し、停止を確認してから削除します。停止を確認できなければ、削除を保留します（会話・添付・成果物は残り、送信は止まります）。</p>
          ) : null}
        </>
      )}
      {pending && !out ? <p className="small" style={{ color: "var(--wait-ink)" }}>{PENDING_TEXT(pending.reason)}</p> : null}
      {pending && !out && pending.reason.kind === "partialFailure" ? (
        <ul className="quit-list">
          {pending.reason.done.map((s, i) => <li key={`d${i}`}>完了: {s}</li>)}
          {pending.reason.failed.map((s, i) => <li key={`f${i}`} style={{ color: "var(--fail)" }}>失敗: {s}</li>)}
        </ul>
      ) : null}
      {out?.kind === "pending" ? <p style={{ color: "var(--wait-ink)" }}><b>削除を保留しました。</b>{PENDING_TEXT(out.pending.reason)}</p> : null}
      {out?.kind === "partial" ? (
        <>
          <p style={{ color: "var(--fail)" }}><b>削除は完了していません。</b>一部だけ実行できました。</p>
          <ul className="quit-list">
            {out.done.map((s, i) => <li key={`d${i}`}>完了: {s}</li>)}
            {out.failed.map((s, i) => <li key={`f${i}`} style={{ color: "var(--fail)" }}>失敗: {s}</li>)}
          </ul>
        </>
      ) : null}
    </div>
  );
}

export interface ExportProps { include: boolean; setInclude: (b: boolean) => void; running: boolean; error: string | null; run: () => void }

export function ExportBody({ e }: { e: ExportProps }) {
  return (
    <div className="content">
      <p>会話本文と、添付・成果物のファイル名を書き出します。ファイルの中身は含めません。</p>
      <label><input type="checkbox" checked={e.include} onChange={(x) => e.setInclude(x.target.checked)} />監視活動の履歴も含める（子・孫を含む、取得済みの範囲）</label>
      <p className="small muted">会話本文は Codex の保存履歴から取り直します。取得できなかった部分は「未取得」と書きます。書き込みに成功したことを確認できたときだけ、完了と表示します。</p>
      {e.error ? <p style={{ color: "var(--fail)" }}>{e.error}</p> : null}
    </div>
  );
}

/** 使用量の取得状態。report が null の間は集計中（error があれば失敗）。 */
export interface UsageProps { report: UsageReport | null; error: string | null; loading: boolean; reload: () => void }

const LINES: Array<[keyof UsageBreakdown, string]> = [
  ["attachments", "添付"], ["artifacts", "成果物"], ["workspace", "作業領域"], ["activity", "監視活動"], ["metadata", "メタデータ"],
];

const breakdownText = (b: UsageBreakdown): string => LINES.map(([k, t]) => `${t} ${fmtBytes(b[k])}`).join("／");

/** 設定の「保存と容量」。全体とチャット別の使用量・内訳・空き容量・旧領域。 */
export function StorageBody({ u, chats, selected }: { u: UsageProps; chats: Chat[]; selected: string | null }) {
  const r = u.report;
  const nameOf = (id: string) => { const c = chats.find((x) => x.key.id === id); return c ? chatName(c) : "（名前を確認できないチャット）"; };
  const mine = r?.chats.find((c) => c.chat.id === selected);
  const sorted = r ? [...r.chats].sort((a, b) => breakdownTotal(b.breakdown) - breakdownTotal(a.breakdown)) : [];
  return (
    <>
      <div className="field"><span>使用量（全体）</span>
        {u.error ? <span style={{ color: "var(--fail)" }}>{u.error}</span> : !r ? <span>{u.loading ? "集計しています…" : "未集計"}</span> : (
          <span><b>{fmtBytes(breakdownTotal(r.total))}</b>（{breakdownText(r.total)}）</span>
        )}
        <span className="note"><button className="btn-line" disabled={u.loading} onClick={u.reload}>{u.loading ? "集計中…" : "再集計"}</button>　AgentDock の専用領域だけを数えます。Codex の履歴（~/.codex）は含みません。</span>
      </div>
      <div className="field"><span>このチャット</span>
        {!r ? <span className="muted">未集計</span> : mine ? <span><b>{fmtBytes(breakdownTotal(mine.breakdown))}</b>（{breakdownText(mine.breakdown)}）</span> : <span className="muted">専用領域はまだありません（保存したものがありません）</span>}
      </div>
      {r && sorted.length ? (
        <div className="field"><span>チャット別</span>
          <table className="small" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead><tr style={{ textAlign: "left", color: "var(--ink3)" }}><th>チャット</th><th>合計</th><th>内訳</th></tr></thead>
            <tbody>{sorted.map((c) => (
              <tr key={c.chat.id}><td>{nameOf(c.chat.id)}</td><td>{fmtBytes(breakdownTotal(c.breakdown))}</td><td className="muted">{breakdownText(c.breakdown)}</td></tr>
            ))}</tbody>
          </table>
        </div>
      ) : null}
      <div className="field"><span>旧領域</span>
        {r ? <span>{fmtBytes(r.legacyArea)}（一般チャットの作業領域。段階①の場所のまま移動しておらず、上の合計には含みません）</span> : <span className="muted">未集計</span>}
      </div>
      <div className="field"><span>空き容量</span><span>{r ? showKnown(r.freeSpace, fmtBytes) : "未確認"}</span>
        <span className="note">保存期限はありません。容量が足りなくても自動で削除しません。足りないときは、その操作を止めてお知らせします。</span></div>
      {r && r.unreadable.length ? (
        <div className="field"><span>読めなかった場所</span>
          <span style={{ color: "var(--fail)" }}>{r.unreadable.length} 件（上の合計に含まれていません）</span>
          <ul className="quit-list" style={{ gridColumn: 2 }}>{r.unreadable.slice(0, 8).map((p) => <li key={p} className="mono small">{p}</li>)}</ul>
        </div>
      ) : null}
      <div className="field"><span>保存場所</span><span className="mono">%LOCALAPPDATA%\com.agentdock.app</span></div>
    </>
  );
}

/** 左一覧のアーカイブの印の文言（Codex への反映の進み具合）。 */
export function archiveTag(l: ChatLocalView | undefined): { text: string; title: string } {
  const v = l?.visibility;
  if (!v || v.kind !== "archived") return { text: "アーカイブ", title: "" };
  switch (v.sync.kind) {
    case "waitingForWorkEnd": return { text: "アーカイブ（Codex 未反映）", title: "作業の終了と停止の確認を待って Codex へ反映します。アプリを再起動した場合は、メニューのアーカイブを選び直してください" };
    case "sending": return { text: "アーカイブ（反映中）", title: "Codex へアーカイブを反映しています" };
    case "failed": return { text: "アーカイブ（反映失敗）", title: `${v.sync.message}。アプリ上はアーカイブのままです。メニューのアーカイブを選ぶと再試行します` };
    case "unsupported": return { text: "アーカイブ（アプリのみ）", title: "この Codex はアーカイブに対応していないため、アプリの一覧で隠しているだけです" };
    case "synced": return { text: "アーカイブ", title: "" };
  }
}

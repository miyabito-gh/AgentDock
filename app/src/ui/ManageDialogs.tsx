// 削除確認・Markdownエクスポート・「保存と容量」の本体（P7）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 取得できていない値は「未確認」と書き、0で代用しない。保存・削除の成功は、ホストの結果を確認できたときだけ言う。
import { useState } from "react";
import type { Chat, ChatLocalView, DeleteOutcome, DeletePendingReason, DeletePreview, DeleteStep, UsageBreakdown, UsageReport } from "../ipc/types";
import { chatName } from "./derive";
import { showKnown } from "./format";
import { About } from "./DialogParts";

/** 削除の完了した工程の表示文（ホストは列挙値だけを持つ）。 */
export function stepText(s: DeleteStep): string {
  switch (s.kind) {
    case "backendHistory": return "会話履歴（AI側）の削除";
    case "backendHistoryNone": return "会話履歴（AI側）の削除（履歴がなく、対象なし）";
    case "chatArea": return "このチャット専用領域（添付・成果物・作業領域・監視活動の記録）の削除";
    case "areaItem": return `領域内の ${s.name}`;
    case "backendItem": return s.label;
  }
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

const breakdownTotal = (b: UsageBreakdown): number => b.attachments + b.artifacts + b.workspace + b.activity + b.metadata + b.baselines + b.revertBackups;

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
            {pv.worktreeRemoved ? <li>このチャットが使っていた worktree は既に削除されています（ブランチは残ります）</li> : null}
            {pv.worktreePath ? <li>このチャットが使っている worktree（<span className="mono">{pv.worktreePath}</span>）とそのブランチ（worktree は残ります。メニュー「作業 → worktree を管理」から削除できます）</li> : null}
          </ul>
          {pv.requiresStop ? (
            <p style={{ color: "var(--fail)" }}>作業中、または停止を確認できていません。作業を中断し、停止を確認してから削除します。停止を確認できなければ、削除を保留します（会話・添付・成果物は残り、送信は止まります）。</p>
          ) : null}
        </>
      )}
      {pending && !out ? <p className="small" style={{ color: "var(--wait-ink)" }}>{PENDING_TEXT(pending.reason)}</p> : null}
      {pending && !out && pending.reason.kind === "partialFailure" ? (
        <ul className="quit-list">
          {pending.reason.done.map((s, i) => <li key={`d${i}`}>完了: {stepText(s)}</li>)}
          {pending.reason.failed.map((s, i) => <li key={`f${i}`} style={{ color: "var(--fail)" }}>失敗: {s}</li>)}
        </ul>
      ) : null}
      {out?.kind === "pending" ? <p style={{ color: "var(--wait-ink)" }}><b>削除を保留しました。</b>{PENDING_TEXT(out.pending.reason)}</p> : null}
      {out?.kind === "partial" ? (
        <>
          <p style={{ color: "var(--fail)" }}><b>削除は完了していません。</b>一部だけ実行できました。</p>
          <ul className="quit-list">
            {out.done.map((s, i) => <li key={`d${i}`}>完了: {stepText(s)}</li>)}
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
      <About><p>会話本文は Codex の保存履歴から取り直します。取得できなかった部分は「未取得」と書きます。書き込みに成功したことを確認できたときだけ、完了と表示します。</p></About>
      {e.error ? <p style={{ color: "var(--fail)" }}>{e.error}</p> : null}
    </div>
  );
}

/** 使用量の取得状態。report が null の間は集計中（error があれば失敗）。 */
export interface UsageProps { report: UsageReport | null; error: string | null; loading: boolean; reload: () => void }

const LINES: Array<[keyof UsageBreakdown, string]> = [
  ["attachments", "添付"], ["artifacts", "成果物"], ["workspace", "作業領域"], ["activity", "監視活動"], ["metadata", "メタデータ"], ["baselines", "変更の控え"], ["revertBackups", "戻す前の控え"],
];

/** 内訳バーの色（トークン）。凡例の文字・値と対にして、色だけで区別させない。 */
const SERIES: Record<keyof UsageBreakdown, string> = {
  attachments: "var(--ser1)", artifacts: "var(--ser2)", workspace: "var(--ser3)", activity: "var(--ser4)", metadata: "var(--ser5)", baselines: "var(--ser6)", revertBackups: "var(--ser7)",
};

const breakdownText = (b: UsageBreakdown): string => LINES.map(([k, t]) => `${t} ${fmtBytes(b[k])}`).join("／");

/** 内訳バー＋凡例。取得できている集計値だけを並べる（0バイトの項目は凡例に値 0 として出し、バーの幅は 0）。 */
function UsageBar({ b }: { b: UsageBreakdown }) {
  const total = breakdownTotal(b);
  return (
    <div className="ubar-wrap">
      <div className="ubar" role="img" aria-label={`内訳: ${breakdownText(b)}`}>
        {total > 0 ? LINES.map(([k, t]) => b[k] > 0 ? <span key={k} title={`${t} ${fmtBytes(b[k])}`} style={{ flexGrow: b[k], background: SERIES[k] }} /> : null) : null}
      </div>
      <ul className="ulegend">
        {LINES.map(([k, t]) => <li key={k}><i style={{ background: SERIES[k] }} aria-hidden="true" />{t}<span className="n">{fmtBytes(b[k])}</span></li>)}
      </ul>
    </div>
  );
}

/** 設定の「保存と容量」。全体とチャット別の使用量・内訳・空き容量・旧領域。 */
/** 変更の控え（Git基準）の設定と、選択中チャットの控えの削除。 */
export interface BaselineProps {
  enabled: boolean;
  setEnabled: (on: boolean) => void;
  /** 選択中チャットの控えを削除する（確認の後だけ呼ぶ）。 */
  deleteForSelected: () => Promise<void>;
  /** 削除できない理由（チャット未選択・作業中・モックなど）。null なら削除できる。 */
  deleteBlocked: string | null;
}

export function StorageBody({ u, chats, selected, acked, bl }: { u: UsageProps; chats: Chat[]; selected: string | null; acked: { list: string[]; reset: () => void }; bl: BaselineProps }) {
  const [confirmDel, setConfirmDel] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const r = u.report;
  const nameOf = (id: string) => { const c = chats.find((x) => x.key.id === id); return c ? chatName(c) : "（名前を確認できないチャット）"; };
  const mine = r?.chats.find((c) => c.chat.id === selected);
  const sorted = r ? [...r.chats].sort((a, b) => breakdownTotal(b.breakdown) - breakdownTotal(a.breakdown)) : [];
  return (
    <>
      <div className="field"><span>使用量（全体）</span>
        {u.error ? <span style={{ color: "var(--fail)" }}>{u.error}</span> : !r ? <span>{u.loading ? "集計しています…" : "未集計"}</span> : (
          <div><b className="nw">{fmtBytes(breakdownTotal(r.total))}</b><UsageBar b={r.total} /></div>
        )}
        <span className="note"><button className="btn-line" disabled={u.loading} onClick={u.reload}>{u.loading ? "集計中…" : "再集計"}</button>　AgentDock の専用領域だけを数えます。Codex の履歴（~/.codex）は含みません。</span>
      </div>
      <div className="field"><span>このチャット</span>
        {!r ? <span className="muted">未集計</span> : mine ? <div><b className="nw">{fmtBytes(breakdownTotal(mine.breakdown))}</b><UsageBar b={mine.breakdown} /></div> : <span className="muted">専用領域はまだありません（保存したものがありません）</span>}
      </div>
      <div className="field"><span>変更の控え</span>
        <label><input type="checkbox" checked={bl.enabled} onChange={(e) => bl.setEnabled(e.target.checked)} />turnの開始時に変更の控えを取る（Gitリポジトリのみ）</label>
        <span className="note">オフにすると以後のturnは控えを取らず、「変更を戻す」の対象になりません。すでに取った控えは消えません（自動では削除しません）。</span></div>
      <div className="field"><span>このチャットの控え</span>
        {!mine ? <span className="muted">{r ? "控えはありません" : "未集計"}</span> : (
          <span>変更の控え <span className="nw">{fmtBytes(mine.breakdown.baselines)}</span>／戻す前の控え <span className="nw">{fmtBytes(mine.breakdown.revertBackups)}</span></span>)}
        {confirmDel ? (
          <div className="gbanner warn" role="alertdialog" aria-label="控えを削除する確認" style={{ gridColumn: 2 }}>
            <span className="grow">このチャットのturnは、以後「変更を戻す」の対象になりません（{mine ? fmtBytes(mine.breakdown.baselines) : "容量未確認"}を削除します）。削除した控えは元に戻せません。「戻す前の控え」とチャット本体は削除しません。</span>
            <button className="btn-danger" disabled={deleting} onClick={() => { setDeleting(true); bl.deleteForSelected().finally(() => { setDeleting(false); setConfirmDel(false); }); }}>{deleting ? "削除しています…" : "控えを削除"}</button>
            <button className="btn-line" disabled={deleting} onClick={() => setConfirmDel(false)}>やめる</button>
          </div>
        ) : (
          <span className="note"><button className="btn-line" disabled={!!bl.deleteBlocked || !mine} title={bl.deleteBlocked ?? undefined} onClick={() => setConfirmDel(true)}>控えを削除…</button>　{bl.deleteBlocked ?? "明示的に削除したときだけ消えます。"}</span>)}
      </div>
      {r && sorted.length ? (
        <div className="field"><span>チャット別</span>
          <div className="tblwrap"><table className="tbl">
            <thead><tr><th>チャット</th><th className="n">合計</th><th className="n">監視活動</th><th className="n">メタデータ</th><th className="n">変更の控え</th><th className="n">戻す前の控え</th><th className="n">添付ほか</th></tr></thead>
            <tbody>{sorted.map((c) => {
              const b = c.breakdown;
              const other = b.attachments + b.artifacts + b.workspace;
              return (
                <tr key={c.chat.id}><td className="nm" title={nameOf(c.chat.id)}>{nameOf(c.chat.id)}</td><td className="n">{fmtBytes(breakdownTotal(b))}</td><td className="n">{fmtBytes(b.activity)}</td><td className="n">{fmtBytes(b.metadata)}</td><td className="n">{fmtBytes(b.baselines)}</td><td className="n">{fmtBytes(b.revertBackups)}</td>
                  <td className="n" title={`添付 ${fmtBytes(b.attachments)}／成果物 ${fmtBytes(b.artifacts)}／作業領域 ${fmtBytes(b.workspace)}`}>{fmtBytes(other)}</td></tr>
              );
            })}</tbody>
          </table></div>
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
      <div className="field"><span>確認済みの警告</span>
        {acked.list.length === 0 ? <span className="muted">なし</span> : (
          <>
            <span>{acked.list.length} 件（起動時の保存データの警告を「閉じる」で確認済みにしたもの。同じ警告は出しません）</span>
            <ul className="quit-list" style={{ gridColumn: 2 }}>{acked.list.map((w) => <li key={w} className="small">{w}</li>)}</ul>
            <span className="note"><button className="btn-line" onClick={acked.reset}>確認済みを解除</button>　解除すると、次回起動から該当する警告が再び表示されます。対象のファイルは削除・作成しません。</span>
          </>
        )}
      </div>
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

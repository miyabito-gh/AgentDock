// クラウド委任（#13、段階③ P3-6）の画面。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: CLI補助（codex cloud、experimental）。状態の取得は開いたときとユーザーの「更新」だけ（定期取得しない）。
//       状態語は原文のまま表示し、AgentDockの状態へ置き換えない。鮮度は取得時刻で示す。
//       送信は確認画面の後だけ。送る前に記録を保存し、結果が不明でも再送しない（一覧で照合する）。
//       取込みはローカルのファイルを変えるため、対象と影響を確認文に書き、作業中のチャットがあれば実行されない。
//       出力が想定形式でなければ要約せず原文の先頭を表示する。成功を断定せず、終了コードとgit statusの差だけを示す。
import { useCallback, useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, CloudApplyResult, CloudCommandOutput, CloudRunStatus, CloudTaskInfo, CloudTaskList, CloudTaskRecord, CloudTaskState, OpCapability } from "../ipc/types";
import { UnverifiedTag } from "./PrefsDialogs";
import { showKnown } from "./format";

const errText = (e: unknown) => host.asIpcError(e).message;

const when = (ms: number) => new Date(ms).toLocaleString();

function stateText(s: CloudTaskState): { text: string; bad: boolean } {
  switch (s.kind) {
    case "submitting": return { text: "送信中（この状態のまま残っている場合は、結果を確認できていません）", bad: true };
    case "submitted": return { text: `送信しました（タスクID: ${s.taskId}）。クラウド側の進行は「更新」で確認してください`, bad: false };
    case "rejected": return { text: `送られていません: ${s.message}`, bad: true };
    case "unknown": return { text: `送られたか分かりません: ${s.message}。再送はしません。下の「更新」で一覧を取得して照合してください`, bad: true };
  }
}

function runText(s: CloudRunStatus): string {
  switch (s.kind) {
    case "exitedZero": return "コマンドは終了コード 0 で終わりました";
    case "exitedNonZero": return `コマンドは終了コード ${s.code} で終わりました`;
    case "unconfirmed": return `結果を確認できません: ${s.reason}`;
  }
}

function RecordView({ r }: { r: CloudTaskRecord }) {
  const st = stateText(r.state);
  return (
    <li className="small" style={{ marginBottom: 6 }}>
      <b>{when(r.since)}</b>　環境 <span className="mono">{r.envId}</span>{r.branch ? <>　ブランチ <span className="mono">{r.branch}</span></> : null}
      <div className="muted" style={{ whiteSpace: "pre-wrap" }}>{r.promptHead}</div>
      <div className={st.bad ? "why err" : undefined}>{st.text}</div>
    </li>
  );
}

export function CloudBody({ live, cap, chat, records }: { live: boolean; cap: OpCapability | undefined; chat: Chat | undefined; records: CloudTaskRecord[] }) {
  const folder = chat?.cwd.kind === "value" ? chat.cwd.value : "";
  const [prompt, setPrompt] = useState("");
  const [envId, setEnvId] = useState("");
  const [branch, setBranch] = useState("");
  const [hint, setHint] = useState<{ repoKey: string | null; branch: string | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [list, setList] = useState<CloudTaskList | null>(null);
  const [listErr, setListErr] = useState<string | null>(null);
  const [view, setView] = useState<{ title: string; out: CloudCommandOutput } | null>(null);
  const [applyFolder, setApplyFolder] = useState(folder);
  const [applied, setApplied] = useState<CloudApplyResult | null>(null);

  const supported = cap?.support === "supported";

  // 記憶した環境ID・現在のブランチは読取りだけで取得する（空欄のときだけ初期値にする）。
  useEffect(() => {
    if (!live || !supported || !folder) return;
    let stale = false;
    host.cloudEnvHint(folder).then((h) => {
      if (stale) return;
      setHint({ repoKey: h.repoKey, branch: h.branch });
      if (h.envId) setEnvId((cur) => (cur ? cur : h.envId ?? ""));
    }).catch(() => undefined);
    return () => { stale = true; };
  }, [live, supported, folder]);

  // 状態の取得は、開いたときと「更新」だけ。
  const refresh = useCallback(() => {
    setListErr(null);
    host.listCloudTasks().then(setList).catch((e) => { setList(null); setListErr(errText(e)); });
  }, []);
  useEffect(() => { if (live && supported) refresh(); }, [live, supported, refresh]);

  if (!live) return <p className="small muted">モックでは動きません（実接続で使えます）。</p>;
  if (!supported) return <p className="small">クラウド委任は、このCodex接続では使えません{cap?.note ? `（${cap.note}）` : ""}。</p>;

  const submit = async () => {
    const b = branch.trim();
    const text = [
      "次の内容でクラウドへ委任します。",
      `環境ID: ${envId.trim()}`,
      `ブランチ: ${b || (hint?.branch ? `${hint.branch}（現在のブランチ）` : "（指定なし。CLIの既定）")}`,
      `作業フォルダ: ${folder || "（なし）"}`,
      "",
      "依頼文はクラウド（外部）へ送られます。送信後の取消はAgentDockからはできません。",
      "結果が確認できなくても再送はしません（一覧で照合します）。",
      "",
      "送りますか？",
    ].join("\n");
    if (!window.confirm(text)) return;
    setBusy(true); setMsg(null);
    try {
      const r = await host.submitCloudTask(prompt, envId.trim(), b || null, folder || null, chat?.key ?? null);
      setMsg(stateText(r.state).text);
      if (r.state.kind === "submitted") setPrompt("");
    } catch (e) { setMsg(`送信しませんでした: ${errText(e)}`); } finally { setBusy(false); }
  };

  const show = async (t: CloudTaskInfo, what: "status" | "diff") => {
    setBusy(true); setMsg(null);
    try {
      const out = await (what === "status" ? host.cloudTaskStatus(t.taskId) : host.cloudTaskDiff(t.taskId));
      setView({ title: `${what === "status" ? "状態（原文）" : "差分"}: ${t.taskId}`, out });
    } catch (e) { setMsg(`取得できませんでした: ${errText(e)}`); } finally { setBusy(false); }
  };

  const apply = async (t: CloudTaskInfo) => {
    const target = applyFolder.trim();
    if (!target) { setMsg("取込み先の作業フォルダを入力してください。"); return; }
    const text = [
      `クラウドのタスク ${t.taskId} の差分を、ローカルの作業ツリーへ取り込みます。`,
      `取込み先: ${target}（Gitのリポジトリのルートで実行します）`,
      "",
      "ローカルのファイルが変更されます。AgentDockは自動では元に戻せません。",
      "そのフォルダで作業中のチャットがある場合は、実行されません。",
      "結果は終了コードと git status の差で表示します（成功の断定はしません）。",
      "",
      "取り込みますか？",
    ].join("\n");
    if (!window.confirm(text)) return;
    setBusy(true); setMsg(null); setApplied(null);
    try { setApplied(await host.applyCloudTask(t.taskId, target)); } catch (e) { setMsg(`取り込みませんでした: ${errText(e)}`); } finally { setBusy(false); }
  };

  const sorted = records.slice().sort((a, b) => b.since - a.since);
  const canSubmit = !busy && prompt.trim() !== "" && envId.trim() !== "";
  return (
    <div className="content">
      <p className="small muted">
        <UnverifiedTag cap={cap} /> codex cloud（experimental のCLI）で行います。出力の形式は実機で確認していないため、想定外の形式のときは要約せず原文の先頭を表示します。
        状態は自動では更新しません（開いたときと「更新」だけ）。{cap?.note ? ` ${cap.note}` : ""}
      </p>
      <h4 style={{ margin: "8px 0 4px" }}>委任する</h4>
      <div className="field"><span>依頼文</span><textarea rows={4} value={prompt} onChange={(e) => setPrompt(e.target.value)} aria-label="依頼文" /></div>
      <div className="field"><span>環境ID</span>
        <input type="text" className="mono" value={envId} onChange={(e) => setEnvId(e.target.value)} aria-label="環境ID" placeholder="クラウド環境のID（手入力）" />
        <span className="note">リポジトリごとに記憶します（送ったときだけ更新）。{hint?.repoKey ? `記憶の単位: ${hint.repoKey}` : "作業フォルダがGitのリポジトリでないため、記憶しません。"}</span></div>
      <div className="field"><span>ブランチ（任意）</span>
        <input type="text" className="mono" value={branch} onChange={(e) => setBranch(e.target.value)} aria-label="ブランチ" placeholder={hint?.branch ?? "空欄ならCLIの既定（現在のブランチ）"} /></div>
      <div className="field"><span>作業フォルダ</span><span className="mono small">{folder || "（チャットを選ぶと表示します）"}</span></div>
      <div className="acts" style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <button className="btn-main" disabled={!canSubmit} onClick={() => void submit()}>確認して送る…</button>
        {canSubmit ? null : <span className="small muted">{busy ? "処理中です" : prompt.trim() === "" && envId.trim() === "" ? "依頼文と環境IDを入力してください" : prompt.trim() === "" ? "依頼文を入力してください" : "環境IDを入力してください"}</span>}
      </div>
      {msg ? <p className="small" style={{ marginTop: 6, whiteSpace: "pre-wrap" }}>{msg}</p> : null}
      <hr />
      <h4 style={{ margin: "0 0 4px" }}>AgentDockから送った記録</h4>
      {sorted.length === 0 ? <p className="small">記録はありません。</p> : <ul style={{ margin: 0, paddingLeft: 18 }}>{sorted.map((r) => <RecordView key={r.id} r={r} />)}</ul>}
      <hr />
      <h4 style={{ margin: "0 0 4px" }}>クラウド側のタスク</h4>
      <div style={{ marginBottom: 6 }}>
        <button className="btn-line" disabled={busy} onClick={refresh}>更新</button>
        {list ? <span className="small muted">　取得時刻 {when(list.observedAt)}（以後は更新していません）</span> : null}
      </div>
      {listErr ? <div className="why err">一覧を取得できませんでした（未取得）: {listErr}</div> : null}
      {list?.raw ? <><p className="small">想定した形式ではなかったため、要約せず原文の先頭を表示します。</p><pre className="diffview" style={{ whiteSpace: "pre-wrap" }}>{list.raw}</pre></> : null}
      {list && !list.raw && list.tasks.length === 0 ? <p className="small">タスクはありません。</p> : null}
      {list && list.tasks.length > 0 ? (
        <>
          <div className="field"><span>取込み先</span><input type="text" className="mono" value={applyFolder} onChange={(e) => setApplyFolder(e.target.value)} aria-label="取込み先の作業フォルダ" placeholder="Gitリポジトリ内の作業フォルダ" /></div>
          <table className="parity-table">
            <thead><tr><th>タスク</th><th>状態（原文）</th><th>更新</th><th>操作</th></tr></thead>
            <tbody>
              {list.tasks.map((t) => (
                <tr key={t.taskId}>
                  <td><span className="mono">{t.taskId}</span>{t.title.kind === "value" ? <div className="small">{t.title.value}</div> : null}</td>
                  <td>{showKnown(t.stateText)}</td>
                  <td className="small">{showKnown(t.updatedText)}</td>
                  <td>
                    <button className="btn-line" disabled={busy} onClick={() => void show(t, "status")}>状態</button>
                    <button className="btn-line" disabled={busy} onClick={() => void show(t, "diff")}>差分</button>
                    <button className="btn-line" disabled={busy} onClick={() => void apply(t)}>取り込む…</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      ) : null}
      {view ? (
        <>
          <h4 style={{ margin: "10px 0 4px" }}>{view.title}</h4>
          <p className="small muted">取得時刻 {when(view.out.observedAt)}　{runText(view.out.status)}</p>
          <pre className="diffview" style={{ whiteSpace: "pre-wrap" }}>{view.out.text}</pre>
        </>
      ) : null}
      {applied ? (
        <>
          <h4 style={{ margin: "10px 0 4px" }}>取込みの結果</h4>
          <p className="small">{runText(applied.status)}。取込み先: <span className="mono">{applied.folder}</span></p>
          <p className="small">
            Gitの状態の変化（取込み前後の差）: {applied.changedPaths.kind === "value"
              ? (applied.changedPaths.value.length === 0 ? "変化したファイルはありません" : `${applied.changedPaths.value.length} 件`)
              : "取得できませんでした（未取得）"}
          </p>
          {applied.changedPaths.kind === "value" && applied.changedPaths.value.length > 0
            ? <ul className="mono small" style={{ margin: 0, paddingLeft: 18 }}>{applied.changedPaths.value.map((p) => <li key={p}>{p}</li>)}</ul> : null}
          <p className="small muted">出力の先頭（原文）:</p>
          <pre className="diffview" style={{ whiteSpace: "pre-wrap" }}>{applied.outputHead}</pre>
        </>
      ) : null}
    </div>
  );
}

// ヘルプメニューのダイアログ本体（DESIGN_MENUS §3.2）。キーボードショートカット・状態の見方・バージョン情報。
// 規則: 取れていない情報は「取得できません」と書き、空欄や0で代用しない。
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import type { SourceInfo } from "../ipc/types";
import { FRESH, STATE } from "./format";
import { shortcutsFor, type RunningInputMode } from "./shortcuts";

/** 表は SHORTCUTS から作る（ハンドラーのあるキーだけが載っている）。 */
export function ShortcutsBody({ runningInput = "queue" }: { runningInput?: RunningInputMode }) {
  return (
    <div className="content">
      <p className="small muted">日本語入力の変換中は、どのキーも反応しません。</p>
      <table className="tbl">
        <thead><tr><th>キー</th><th>動作</th><th>場面</th></tr></thead>
        <tbody>
          {shortcutsFor(runningInput).map((s) => <tr key={s.id}><td><span className="kbd">{s.keys}</span></td><td>{s.label}</td><td className="small">{s.where}</td></tr>)}
        </tbody>
      </table>
    </div>
  );
}

const STATE_NOTE: Record<keyof typeof STATE, string> = {
  running: "作業を進めています。",
  waiting: "承認・質問への回答を待っています。",
  failed: "失敗が確認できた状態です。",
  interrupted: "中断を要求した状態です。停止を確認できたときだけ「停止」と表示します。",
  done: "完了が確認できた状態です。",
  idle: "起動していて、いまは作業していません。",
  unknown: "取得できていない状態です。完了や失敗とは扱いません。",
  initializing: "起動の準備中です。",
  closed: "会話が閉じられています。",
};
const FRESH_NOTE: Record<keyof typeof FRESH, string> = {
  live: "いま Codex から状態を受信しています。",
  historyOnly: "保存履歴から読み取った内容です。いまの動きは分かりません。",
  needsReconcile: "再接続後などで、状態の照合が済んでいません。",
  disconnected: "Codex との接続が切れています。最後に受信した内容のままです。",
  unsupported: "この接続では状態を取得できません。",
};

/** 状態（§4.1）と鮮度（§4.2）の凡例。既存の表示文言（format.ts）から作る。 */
export function HowtoBody() {
  return (
    <div className="content">
      <p className="small">通信が切れた・取得できないことを「完了」や「失敗」に読み替えることはありません。</p>
      <h4>状態</h4>
      <table className="tbl">
        <tbody>
          {(Object.keys(STATE) as Array<keyof typeof STATE>).map((k) => <tr key={k}><td><b>{STATE[k].t}</b></td><td className="small">{STATE_NOTE[k]}</td></tr>)}
        </tbody>
      </table>
      <h4>情報の鮮度</h4>
      <table className="tbl">
        <tbody>
          {(Object.keys(FRESH) as Array<keyof typeof FRESH>).map((k) => <tr key={k}><td><b>{FRESH[k].t}</b></td><td className="small">{FRESH_NOTE[k]}</td></tr>)}
        </tbody>
      </table>
    </div>
  );
}

/** AgentDock の版（Tauri から。取れなければ「取得できません」）と、接続中の Codex の版。 */
export function AboutBody({ source, live }: { source: SourceInfo | undefined; live: boolean }) {
  const [app, setApp] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    if (!live) { setApp(null); return; }
    getVersion().then((v) => { if (alive) setApp(v); }).catch(() => { if (alive) setApp(null); });
    return () => { alive = false; };
  }, [live]);
  const v = source?.version;
  const codex = !source ? "接続していません（取得できません）"
    : v?.kind === "match" ? v.version
      : v?.kind === "mismatch" ? `${v.actual}（対象版は ${v.expected}）`
        : `取得できません${v?.kind === "unknown" ? `（${v.message}）` : ""}`;
  return (
    <div className="content">
      <table className="tbl">
        <tbody>
          <tr><td>AgentDock</td><td>{app === undefined ? "確認中…" : app ?? (live ? "取得できません" : "モック表示のため取得できません")}</td></tr>
          <tr><td>Codex</td><td>{codex}</td></tr>
        </tbody>
      </table>
    </div>
  );
}

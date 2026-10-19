// 組織実行のタスクへの割り込み指示の入力欄（DESIGN_ORG_R2 2.1・5）。
// 通常のチャット送信ではない。実行中の turn に今すぐ届けられるときだけ turn/steer（1回）で送り、そうでなければ「次の送信に添える」。
// 新しい turn は始めない。結果（届きました／届きませんでした／確認できません）は下のカードに残し、確認できないものは自動では送り直さない。
import { useState } from "react";
import * as host from "../ipc/client";
import type { OrgInterjectView, OrgTaskView } from "../ipc/types";

export interface InterjectComposerProps {
  workflowId: string;
  task: OrgTaskView;
  /** このタスクの割り込み指示の履歴（決着していないものを先に）。 */
  interjects: OrgInterjectView[];
  /** 命令の実行（App が実接続か確認し、失敗を通知し、成功したら取り直す）。 */
  run: (label: string, f: () => Promise<string | void>) => Promise<boolean>;
  /** 狭い置き場（行のメニュー）で、見出しを省く。 */
  compact?: boolean;
}

const MAX = 4000;

export function InterjectComposer({ workflowId, task, interjects, run, compact }: InterjectComposerProps) {
  const [text, setText] = useState("");
  // 既定は「届かなければ次の送信に添える」（確実に送っていないときだけ。受理不明では何もしない）。
  const [fallback, setFallback] = useState(true);
  const [busy, setBusy] = useState(false);
  const route = task.interject;
  const refused = route.route === "refused";
  const blank = text.trim() === "";
  const over = text.trim().length > MAX;
  const mine = interjects.filter((x) => x.task === task.id);
  const send = async () => {
    if (refused || blank || over || busy) return;
    setBusy(true);
    const ok = await run("割り込み指示", async () => {
      const a = await host.orgInterject(workflowId, task.id, text.trim(), fallback);
      return a.route === "steer" ? "実行中の turn に指示を送りました。結果は経過に出ます（届いたか確認できないときは、自動では送り直しません）。" : "次の送信に添えます（すぐには届きません）。";
    });
    setBusy(false);
    if (ok) setText("");
  };
  const decide = (id: string, add: boolean) => void run(add ? "次の送信に添える" : "割り込みの破棄", async () => {
    await host.orgInterjectFallback(workflowId, id, add);
    return add ? "次の送信に添えます。" : "破棄しました。";
  });
  return (
    <div className="interject" aria-label={`${task.id} への割り込み指示`}>
      {compact ? null : <div className="small muted">タスク {task.id}『{task.title}』の役割チャットへの指示です。通常の送信ではありません。</div>}
      <div className={`small interject-route ${route.route}`} role="status">{route.label}</div>
      <textarea rows={3} style={{ width: "100%" }} value={text} maxLength={MAX + 200} disabled={refused || busy} aria-label="割り込み指示の内容" placeholder={refused ? "いまは送れません" : "例: READMEにも一言追記してください"}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void send(); } }} />
      <div className="acts">
        {route.route === "steer" ? <label className="small"><input type="checkbox" checked={fallback} onChange={(e) => setFallback(e.target.checked)} /> 届かなければ次の送信に添える</label> : null}
        <span className="grow" />
        {over ? <span className="small err">{MAX}文字までにしてください</span> : null}
        <button className="btn primary" disabled={refused || blank || over || busy} title={refused ? route.label : "Ctrl+Enter でも送れます"} onClick={() => void send()}>{busy ? "送っています…" : route.route === "nextTurn" ? "次の送信に添える" : "指示を送る"}</button>
      </div>
      {mine.length > 0 ? (
        <ul className="interject-list">
          {mine.map((x) => (
            <li key={x.id} className={`interject-card ${x.outcome}`}>
              <b>割り込み指示</b>（{x.label}）<span className="small muted">　{x.textHead}</span>
              {x.canDecide ? (
                <div className="acts">
                  <button className="btn sm" onClick={() => decide(x.id, true)} title="同じ文を、次にこのタスクへ送るときに添えます（自動では送り直しません）">次の送信に添える</button>
                  <button className="btn sm" onClick={() => decide(x.id, false)}>破棄</button>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

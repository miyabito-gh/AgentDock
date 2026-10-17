// 組織実行の役割チャットの表示部品。本文・モデル出力はHTMLとして解釈しない（文字で出すか、既存のMarkdown経由）。
import { useId } from "react";
import { Markdown } from "./Markdown";
import { EFFECT_TEXT } from "./org";
import { roleLabel, type OrgReport } from "./orgChat";

/** 組織が自動送信した最初の指示。ラベルで利用者の発話と区別し、既定で畳む（全文はDOMに残る）。 */
export function OrgInstructionBubble({ text, forceOpen, open, onToggle }: { text: string; forceOpen: boolean; open: boolean; onToggle: () => void }) {
  const id = useId();
  const shown = open || forceOpen;
  return (
    <>
      <div className="org-autolabel">組織実行の指示（自動送信）</div>
      <div id={id} className={`bubble org-auto${shown ? "" : " clamp"}`} data-find="body">{text}</div>
      <button type="button" className="foldmsg" aria-expanded={shown} aria-controls={id} onClick={onToggle}>{shown ? "折りたたむ" : "展開"}</button>
    </>
  );
}

const TESTS_TEXT = (t: { ran: boolean; passed: boolean }): string => (!t.ran ? "実行していない" : t.passed ? "実行・成功" : "実行・失敗");

/** 構造化出力の読みやすい要約。下に元のJSONを折りたたみで残す。 */
export function OrgReportView({ report, raw }: { report: OrgReport; raw: string }) {
  return (
    <div className="org-report">
      {report.kind === "verdict" ? (
        <>
          <span className={`st ${report.pass ? "done" : "wait"}`}><i />判定: {report.pass ? "合格" : "差し戻し"}</span>
          {report.findings.length ? <ul>{report.findings.map((f, i) => <li key={i}><Markdown text={f} /></li>)}</ul> : <p className="muted small">指摘はありません。</p>}
        </>
      ) : report.kind === "worker" ? (
        <>
          <Markdown text={report.summary} />
          <div className="small"><b>変更ファイル</b>（{report.changedFiles.length}件）</div>
          {report.changedFiles.length ? <ul>{report.changedFiles.map((f, i) => <li key={i} className="mono">{f}</li>)}</ul> : null}
          <div className="small"><b>テスト</b>: {TESTS_TEXT(report.tests)}（AIの自己申告）</div>
        </>
      ) : (
        <>
          <div className="small"><b>計画</b>（タスク {report.tasks.length}件）</div>
          <ul>{report.tasks.map((t, i) => (
            <li key={i}><b className="mono">{t.id}</b> <span className="muted">[{roleLabel(t.role)}]</span> {t.title}
              {t.deps.length ? <span className="muted small">　{t.deps.join(",")}待ち</span> : null}
              {t.effects.length ? <div className="small">効果の申告: {t.effects.map((e) => (EFFECT_TEXT as Record<string, string>)[e] ?? e).join("、")}</div> : null}</li>
          ))}</ul>
        </>
      )}
      <details className="org-raw"><summary>元のJSONを表示</summary><pre className="mono">{raw}</pre></details>
    </div>
  );
}

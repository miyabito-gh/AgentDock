import { useState, type ReactNode } from "react";
import type { Chat, ModelInfo, SourceInfo } from "../ipc/types";
import { Icon } from "./Icon";

export type DialogState =
  | { type: "settings"; tab: string }
  | { type: "newChat" }
  | { type: "quit" }
  | { type: "force" }
  | { type: "unv"; why: string }
  | { type: "attach" };

function Shell({ title, children, foot, wide, onClose }: { title: string; children: ReactNode; foot?: ReactNode; wide?: boolean; onClose: () => void }) {
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className={`dialog ${wide ? "wide" : ""}`} role="dialog" aria-modal="true" aria-label={title}>
        <header><h3>{title}</h3><button aria-label="閉じる" onClick={onClose}><Icon name="x" /></button></header>
        {children}
        {foot ? <footer>{foot}</footer> : null}
      </div>
    </div>
  );
}

const TABS: Array<[string, string]> = [["general", "全般"], ["notify", "通知"], ["input", "入力"], ["model", "モデル"], ["codex", "Codex"], ["mcp", "MCP・Plugins"], ["skills", "Skills"], ["storage", "保存と容量"]];

function SettingsBody({ tab, enterMode, setEnterMode, top, source, models }: {
  tab: string; enterMode: "ctrl" | "enter"; setEnterMode: (m: "ctrl" | "enter") => void; top: { main: boolean; mini: boolean; setMain: (b: boolean) => void; setMini: (b: boolean) => void };
  source: SourceInfo | undefined; models: ModelInfo[];
}) {
  switch (tab) {
    case "general": return (
      <>
        <div className="field"><span>ログイン時に起動</span><label><input type="checkbox" />Windows にサインインしたら起動する</label><span className="note">オンにすると、通常画面は開かずトレイに入った状態で起動します。起動しただけで過去の依頼を再実行しません。</span></div>
        <div className="field"><span>最前面</span><div><label><input type="checkbox" checked={top.main} onChange={(e) => top.setMain(e.target.checked)} />通常画面</label>　<label><input type="checkbox" checked={top.mini} onChange={(e) => top.setMini(e.target.checked)} />監視窓</label></div><span className="note">最前面にしても入力フォーカスは奪いません。</span></div>
        <div className="field"><span>閉じるボタン</span><span>トレイに格納して作業を続ける（終了はメニューの「AgentDock を終了」）</span></div>
      </>);
    case "notify": return (
      <>
        <div className="field"><span>通知</span><label><input type="checkbox" defaultChecked />通知を使う</label></div>
        <div className="field"><span>種類</span><div><label><input type="checkbox" defaultChecked />承認・質問待ち</label>　<label><input type="checkbox" defaultChecked />完了</label>　<label><input type="checkbox" defaultChecked />失敗</label></div><span className="note">子・孫の完了と失敗は、チャットごとに2秒間まとめて通知します。承認と質問は待たずに通知します。</span></div>
        <div className="field"><span>音</span><label><input type="checkbox" defaultChecked />Windows の標準の音を鳴らす</label></div>
      </>);
    case "input": return (
      <>
        <div className="field"><span>送信キー</span><div>
          <label><input type="radio" name="ek" checked={enterMode === "ctrl"} onChange={() => setEnterMode("ctrl")} />Ctrl＋Enter で送信／Enter で改行</label><br />
          <label><input type="radio" name="ek" checked={enterMode === "enter"} onChange={() => setEnterMode("enter")} />Enter で送信／Shift＋Enter で改行</label></div></div>
        <div className="field"><span>実行中の入力</span><span>初期値は「完了後に送信」</span></div>
      </>);
    case "model": return (
      <div className="field"><span>新しいチャットの既定</span>
        <div style={{ display: "flex", gap: 6 }}><select>{models.map((m) => <option key={m.id}>{m.displayName}</option>)}</select></div>
        <span className="note">既定値を変えても、既存のチャットの設定は変わりません。選択肢は Codex から取得します。</span></div>);
    case "codex": {
      const v = source?.version;
      return (
        <>
          <div className="field"><span>codex.exe の場所</span><div style={{ display: "flex", gap: 6 }}><input type="text" className="mono" style={{ flex: 1 }} defaultValue="" aria-label="codex.exe のパス" /><button className="btn-line">参照…</button></div><span className="note">空欄なら PATH の codex を使います。</span></div>
          <div className="field"><span>検出結果</span><span>{v?.kind === "match" ? `codex-cli ${v.version}（対象版）` : v?.kind === "mismatch" ? `${v.actual}（対象版 ${v.expected} と不一致）` : v?.kind === "unknown" ? `確認できません（${v.message}）` : "未確認"}</span></div>
          <div className="field"><span>接続</span><span>App Server（stdio）</span></div>
        </>);
    }
    case "mcp": return <p className="small muted">導入・有効・信頼・認証・接続・会話への反映を分けて表示します（T5以降で取得）。</p>;
    case "skills": return <p className="small muted">AGENTS.md と Skills の確認（T5以降で取得）。</p>;
    default: return <div className="field"><span>保存期限</span><span>ありません。容量が足りなくても自動で削除しません。足りないときは、その操作を止めてお知らせします。</span></div>;
  }
}

export function Dialogs({ d, onClose, chats, source, models, enterMode, setEnterMode, top, setTab, onAct }: {
  d: DialogState; onClose: () => void; chats: Chat[]; source: SourceInfo | undefined; models: ModelInfo[];
  enterMode: "ctrl" | "enter"; setEnterMode: (m: "ctrl" | "enter") => void;
  top: { main: boolean; mini: boolean; setMain: (b: boolean) => void; setMini: (b: boolean) => void };
  setTab: (t: string) => void; onAct: (a: string) => void;
}) {
  const [kind, setKind] = useState("general");
  void chats;
  switch (d.type) {
    case "settings": return (
      <Shell title="設定" wide onClose={onClose} foot={<button className="btn-main" onClick={onClose}>閉じる</button>}>
        <div className="tabs" role="tablist">{TABS.map(([k, t]) => <button key={k} role="tab" aria-selected={d.tab === k} onClick={() => setTab(k)}>{t}</button>)}</div>
        <div className="content"><SettingsBody tab={d.tab} enterMode={enterMode} setEnterMode={setEnterMode} top={top} source={source} models={models} /></div>
      </Shell>);
    case "newChat": return (
      <Shell title="新しいチャット" onClose={onClose} foot={<><button className="btn-line" onClick={onClose}>キャンセル</button><button className="btn-main" onClick={() => onAct("createChat")}>作成</button></>}>
        <div className="content">
          <div className="field"><span>AI</span><div><label><input type="radio" defaultChecked />Codex</label><div className="small muted">他の AI は今後追加できるようにする予定です。</div></div></div>
          <div className="field"><span>作業フォルダ</span><div><label><input type="radio" name="k" checked={kind === "general"} onChange={() => setKind("general")} />指定しない（一般チャット）</label><br /><label><input type="radio" name="k" checked={kind === "dev"} onChange={() => setKind("dev")} />フォルダを指定する</label>　<button className="btn-line"><Icon name="folder" />選択…</button></div></div>
          <div className="field"><span>モデル</span><div style={{ display: "flex", gap: 6 }}><select>{models.map((m) => <option key={m.id}>{m.displayName}</option>)}</select></div><span className="note">新しいチャットの既定値です。</span></div>
        </div>
      </Shell>);
    case "quit": return (
      <Shell title="AgentDock を終了" onClose={onClose} foot={<><button className="btn-line" onClick={onClose}>終了を取り消す</button><button className="btn-danger" onClick={() => onAct("quitStop")}>作業を中断して終了</button></>}>
        <div className="content"><p>作業中のチャットがある場合、各対象の停止と保存を確認してから終了します。停止を確認できない対象があれば、その内容を表示して終了を止めます。</p></div>
      </Shell>);
    case "force": return (
      <Shell title="強制終了" onClose={onClose} foot={<><button className="btn-line" onClick={onClose}>やめる</button><button className="btn-danger" onClick={() => onAct("doForce")}>強制終了する</button></>}>
        <div className="content"><p>停止を確認できていない対象を強制終了します。保存されていない内容が失われたり、処理の一部が残ったりする可能性があります。このアプリが起動していない実行は対象にしません。強制終了の後も、停止を確認できるまでは「停止未確認」として扱います。</p></div>
      </Shell>);
    case "unv": return (
      <Shell title="この操作はまだ使えません" onClose={onClose} foot={<button className="btn-main" onClick={onClose}>閉じる</button>}>
        <div className="content"><p>{d.why}。</p><p className="small muted">Codex 側の経路と動作を確認できるまで、成功したように見せることはしません。</p></div>
      </Shell>);
    case "attach": return (
      <Shell title="追加" onClose={onClose}>
        <div className="content" style={{ display: "grid", gap: 6 }}>
          <button className="btn-line" onClick={() => onAct("stub")}><Icon name="clip" />ファイルを選ぶ（このチャット用にコピーします）</button>
          <button className="btn-line" onClick={() => onAct("stub")}><Icon name="chat" />過去の会話を参考に指定</button>
          <p className="small muted">フォルダは作業フォルダの指定として扱い、中身をコピーしません。</p>
        </div>
      </Shell>);
  }
}

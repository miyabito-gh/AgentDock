// 設定「返信・推敲」: ショートカット（キー入力で変更・登録失敗は理由を表示・再登録は即時）、文章ルール3欄（共通・返信用・推敲用）、モデルと推論強度。
// 文章ルールは会話ごとの指示として渡すだけで、~/.codex の設定は変更しない。保存された文章や結果の履歴は残さない。
import { useEffect, useState } from "react";
import type { ComposeSettings, ComposeShortcutStatus, ModelInfo } from "../ipc/types";
import { effortChoices, hasMainModifier, rulesDirty, RULES_MAX_CHARS, shortcutFromEvent } from "./replySettingsLogic";

export interface ReplySettingsProps {
  value: ComposeSettings;
  models: ModelInfo[];
  /** 実接続のときだけ使える。 */
  live: boolean;
  /** 保存する（ショートカットが変わるときは登録し直す）。成功は null、失敗は理由。 */
  save: (c: ComposeSettings) => Promise<string | null>;
  /** ショートカットの登録の状態を読む。 */
  status: () => Promise<ComposeShortcutStatus>;
}

const DEFAULT_SHORTCUT = "Ctrl+Alt+R";

export function ReplySettingsBody({ value, models, live, save, status }: ReplySettingsProps) {
  const [draft, setDraft] = useState<ComposeSettings>(value);
  const [key, setKey] = useState(value.shortcut);
  const [reg, setReg] = useState<ComposeShortcutStatus | null>(null);
  const [keyMsg, setKeyMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [saveMsg, setSaveMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (live) status().then(setReg).catch(() => setReg(null)); }, [live, value.shortcut]); // eslint-disable-line react-hooks/exhaustive-deps
  // 保存済みの値が（保存・他の経路で）変わったら、編集中の写しを合わせる。
  const sig = JSON.stringify(value);
  useEffect(() => { setDraft(value); setKey(value.shortcut); }, [sig]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!live) return <div className="field"><span>返信・推敲</span><span className="small">実接続のときだけ使えます。</span></div>;

  const dirty = rulesDirty(value, draft);
  const efforts = effortChoices(models, draft.model);
  const keyDirty = key !== value.shortcut;

  const applyKey = async (next: string) => {
    setBusy(true); setKeyMsg(null);
    const err = await save({ ...value, shortcut: next });
    setBusy(false);
    if (err) { setKeyMsg({ ok: false, text: err }); return; }
    setKeyMsg({ ok: true, text: next === "" ? "ショートカットを解除しました" : `「${next}」に登録しました` });
    status().then(setReg).catch(() => undefined);
  };
  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Tab") return; // フォーカス移動は妨げない
    e.preventDefault();
    if (e.key === "Escape") { setKey(value.shortcut); setKeyMsg(null); return; }
    if ((e.key === "Backspace" || e.key === "Delete") && !e.ctrlKey && !e.altKey && !e.metaKey) { setKey(""); setKeyMsg(null); return; }
    const s = shortcutFromEvent(e);
    if (!s) return;
    setKey(s);
    setKeyMsg(hasMainModifier(s) ? null : { ok: false, text: "Ctrl・Alt・Win のどれかと組み合わせてください" });
  };
  const saveRules = async () => {
    setBusy(true); setSaveMsg(null);
    const err = await save({ ...draft, shortcut: value.shortcut });
    setBusy(false);
    setSaveMsg(err ? { ok: false, text: `保存できませんでした: ${err}` } : { ok: true, text: "保存しました。次に作成するときから使います" });
  };
  const rules = (label: string, field: "commonRules" | "replyRules" | "polishRules", placeholder: string) => (
    <div className="field"><label htmlFor={`rs-${field}`}>{label}</label>
      <div>
        <textarea id={`rs-${field}`} rows={3} style={{ width: "100%", boxSizing: "border-box" }} value={draft[field]} placeholder={placeholder} onChange={(e) => setDraft({ ...draft, [field]: e.target.value })} />
        <span className="note">{[...draft[field]].length.toLocaleString("ja-JP")} / {RULES_MAX_CHARS.toLocaleString("ja-JP")}文字</span>
      </div>
    </div>);

  return (
    <>
      <div className="field"><label htmlFor="rs-key">ショートカット</label>
        <div>
          <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
            <input id="rs-key" type="text" readOnly className="mono" style={{ width: 200 }} value={key} placeholder="（解除）" onKeyDown={onKeyDown}
              aria-describedby="rs-key-help" aria-label="返信・推敲のショートカット。キーを押して指定します" />
            <button className="btn primary" disabled={busy || !keyDirty || (key !== "" && !hasMainModifier(key))} onClick={() => void applyKey(key)}>登録</button>
            <button className="btn-line" disabled={busy || key === DEFAULT_SHORTCUT} onClick={() => { setKey(DEFAULT_SHORTCUT); setKeyMsg(null); }}>既定に戻す</button>
            <button className="btn-line" disabled={busy || key === ""} onClick={() => { setKey(""); setKeyMsg(null); }}>解除</button>
          </div>
          <span id="rs-key-help" className="note">欄を選んで、使いたいキーを押します（例 Ctrl+Alt+R）。Esc で取り消し、Backspace で空にします。押すと、前面のアプリで選んでいる文章を取り込んでパネルを開きます。</span>
          {reg?.registered && !keyDirty ? <span className="note" role="status">登録されています: {reg.registered}</span> : null}
          {reg?.error && !keyDirty ? <span className="note" role="alert" style={{ color: "var(--fail-ink)" }}>登録できていません: {reg.error}</span> : null}
          {keyMsg ? <span className="note" role="status" style={keyMsg.ok ? undefined : { color: "var(--fail-ink)" }}>{keyMsg.text}</span> : null}
        </div>
      </div>
      <p className="small muted">文章ルールは、作成のたびに会話ごとの指示として渡します（Codex の設定は変更しません）。空なら標準の書き方です。「共通」と、使った機能（返信か推敲）のルールを合わせて使います。</p>
      {rules("共通", "commonRules", "例: 敬体（です・ます）で書く。絵文字は使わない。")}
      {rules("返信用", "replyRules", "例: 結論を先に書く。署名は付けない。依頼には期限と担当を明記する。")}
      {rules("推敲用", "polishRules", "例: 意味を変えない。一文は60字以内を目安に。専門用語はそのまま残す。")}
      <div className="field"><label htmlFor="rs-model">モデル</label>
        <div style={{ display: "flex", gap: 6, flexWrap: "wrap", alignItems: "center" }}>
          {models.length === 0 ? <span className="note" role="status">モデル一覧を取得できません（Codex に接続してから開き直してください）。いまの指定: {draft.model ?? "Codex の既定"}</span> : (
            <select id="rs-model" value={draft.model ?? ""} onChange={(e) => setDraft({ ...draft, model: e.target.value || null, effort: e.target.value ? null : draft.effort })}>
              <option value="">指定しない（Codex の既定）</option>
              {draft.model && !models.some((m) => m.id === draft.model) ? <option value={draft.model}>{draft.model}（一覧にありません）</option> : null}
              {models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}
            </select>)}
          <label htmlFor="rs-effort">推論強度</label>
          <select id="rs-effort" value={draft.effort ?? ""} onChange={(e) => setDraft({ ...draft, effort: e.target.value || null })} disabled={efforts.length === 0 && !draft.effort}>
            <option value="">モデルの既定</option>
            {draft.effort && !efforts.includes(draft.effort) ? <option value={draft.effort}>{draft.effort}（このモデルの一覧にありません）</option> : null}
            {efforts.map((e) => <option key={e} value={e}>{e === "low" ? "low（速さ重視・既定）" : e}</option>)}
          </select>
        </div>
        <span className="note">既定は速さ重視の低い強度（low）です。モデルが示さない強度は、指定せずに作成します。</span>
      </div>
      <div className="field"><span />
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <button className="btn primary" disabled={busy || !dirty} onClick={() => void saveRules()}>文章ルール・モデルを保存</button>
          {dirty ? <button className="btn-line" disabled={busy} onClick={() => { setDraft(value); setSaveMsg(null); }}>元に戻す</button> : null}
          {saveMsg ? <span className="note" role="status" style={saveMsg.ok ? undefined : { color: "var(--fail-ink)" }}>{saveMsg.text}</span> : (dirty ? <span className="note" role="status">未保存の変更があります</span> : null)}
        </div>
      </div>
    </>
  );
}

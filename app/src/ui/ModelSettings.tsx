import { useState } from "react";
import type { ModelInfo } from "../ipc/types";
import { savedDefaultModelId } from "./quickNew";

/** 設定「モデル」: 新しいチャットの既定モデル（AppSettings.defaultModel）。選ぶとすぐ保存し、結果を表示する。 */
export interface ModelDefaultProps {
  /** 保存されている既定のモデルID（なし＝null）。 */
  saved: string | null;
  /** 保存。成功したら true（失敗の理由は通知に出る）。null は「指定しない」。 */
  set: (id: string | null) => Promise<boolean>;
}

export function ModelSettingsBody({ models, def }: { models: ModelInfo[]; def: ModelDefaultProps }) {
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const valid = savedDefaultModelId(models.map((m) => ({ id: m.id, isDefault: m.isDefault, defaultEffort: m.defaultEffort })), def.saved ? { model: def.saved } : null);
  const onChange = async (v: string) => {
    setStatus(null);
    const ok = await def.set(v === "" ? null : v);
    setStatus(ok ? { ok: true, text: "保存しました" } : { ok: false, text: "保存できませんでした。理由は画面の通知を確認してください。次回の起動では保存されている値に戻ります" });
  };
  if (models.length === 0) {
    return (
      <div className="field"><span>新しいチャットの既定</span>
        <span className="note" role="status">モデル一覧を取得できません（Codex に接続してから開き直してください）</span></div>);
  }
  return (
    <div className="field"><span>新しいチャットの既定</span>
      <div style={{ display: "flex", gap: 6 }}>
        <select aria-label="新しいチャットの既定のモデル" value={valid ?? ""} onChange={(e) => { void onChange(e.target.value); }}>
          <option value="">指定しない（Codex の既定を使う）</option>
          {models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}
        </select></div>
      {def.saved && !valid && <span className="note">保存されている「{def.saved}」は、現在の一覧にありません。別のモデルを選ぶか、「指定しない」にしてください。</span>}
      <span className="note" role="status">{status ? status.text : "選ぶとすぐ保存します。"}</span>
      <span className="note">既存のチャットの設定は変わりません。選択肢は Codex から取得します。</span></div>);
}

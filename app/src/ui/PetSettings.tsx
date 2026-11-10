// 設定「全般」のペット欄。ペットのデータ（~/.codex/pets）は読むだけで、コピー・変更しない。非対応のペットは理由つきで一覧に残す。
import { useEffect, useState } from "react";
import type { PetEntry, PetSettings } from "../ipc/types";
import * as host from "../ipc/client";

export interface PetProps {
  value: PetSettings;
  /** 保存して窓へ反映する（失敗は呼び出し側が表示）。 */
  set: (p: PetSettings) => Promise<void> | void;
  /** 実接続のときだけ使える。 */
  live: boolean;
}

export function PetSettingsField({ value, set, live }: PetProps) {
  const [list, setList] = useState<PetEntry[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    if (!live) return;
    let alive = true;
    host.petList().then((l) => { if (alive) setList(l); }).catch((e) => { if (alive) setErr(host.asIpcError(e).message); });
    return () => { alive = false; };
  }, [live]);
  if (!live) return <div className="field"><span>ペット</span><span className="small">実接続のときだけ使えます。</span></div>;
  const chosen = list?.find((e) => e.folder === value.selected);
  return (
    <div className="field"><span>ペット</span>
      <div>
        <label><input type="checkbox" checked={value.visible} onChange={(e) => void set({ ...value, visible: e.target.checked })} />画面に表示する</label>
        <div>
          <label>種類　<select value={value.selected ?? ""} onChange={(e) => void set({ ...value, selected: e.target.value || null })} aria-label="ペットの種類">
            <option value="">（選ばない）</option>
            {value.selected && list && !chosen && <option value={value.selected}>{value.selected}（見つかりません）</option>}
            {(list ?? []).map((p) => <option key={p.folder} value={p.folder} disabled={!p.supported}>{p.displayName}{p.supported ? "" : "（非対応）"}</option>)}
          </select></label>
        </div>
        <div>
          <label>大きさ　<input type="range" min={50} max={150} step={10} value={value.scalePercent} onChange={(e) => void set({ ...value, scalePercent: Number(e.target.value) })} aria-label="ペットの大きさ" /> {value.scalePercent}%</label>
        </div>
        {err && <div className="note">一覧を読めませんでした: {err}</div>}
        {list && list.length === 0 && <div className="note">ペットがありません。Codex の hatch-pet で作ると ~/.codex/pets に入り、ここに出ます。</div>}
        {list?.filter((p) => !p.supported).map((p) => <div key={p.folder} className="note">{p.displayName}: {p.unsupportedReason}</div>)}
        <span className="note">完了・失敗・承認待ちを吹き出しで知らせます（表示中は OS 通知の代わり）。承認や停止はペットからはできません。</span>
      </div>
    </div>
  );
}

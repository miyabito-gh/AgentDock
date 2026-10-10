// コンパクト監視窓（別の窓、`?view=monitor`）。右パネルと同じ表示範囲の規則（`DockPane`）を使う。
// 承認・質問はここでは回答せず、通常画面を開いて回答する。この窓を閉じても作業は続く。
import { useCallback, useEffect, useRef, useState } from "react";
import type { HostEvent, HostEventEnvelope, MonitorScope } from "./ipc/types";
import * as host from "./ipc/client";
import { applyHostEvent, createEventPipeline, emptyBundle, replaceSnapshot, seqAction } from "./ipc/live";
import type { Bundle } from "./mock/data";
import { DockPane } from "./ui/Dock";
import { keyStr } from "./ui/format";
import { applyAppearance, watchSystemTheme } from "./ui/appearance";

export default function MonitorApp() {
  const [bundle, setBundle] = useState<Bundle>(() => emptyBundle());
  const [selId, setSelId] = useState<string | null>(null);
  const [showFinished, setShowFinished] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  const say = useCallback((t: string) => {
    setToast(t);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  }, []);
  const sayErr = useCallback((prefix: string, e: unknown) => say(`${prefix}: ${host.asIpcError(e).message}`), [say]);

  // 購読 → スナップショット。seqが飛んだら取り直す（通常画面と同じ規則）。
  useEffect(() => {
    let alive = true;
    const unlisten: Array<() => void> = [];
    let ready = false;
    let last = 0;
    let resyncing = false;
    const pipe = createEventPipeline((events) => setBundle((b) => events.reduce(applyHostEvent, b)));
    const pending: HostEventEnvelope[] = [];
    const resync = async () => {
      if (resyncing) return;
      resyncing = true;
      try {
        const snap = await host.getSnapshot();
        if (!alive) return;
        last = snap.seq;
        pipe.flushAll();
        setBundle((b) => replaceSnapshot(b, snap));
      } catch (e) { sayErr("状態を取り直せませんでした", e); } finally { resyncing = false; }
    };
    const handle = (env: HostEventEnvelope) => {
      const a = seqAction(last, env.seq);
      if (a === "ignore") return;
      if (a === "resync") { void resync(); return; }
      last = env.seq;
      pipe.push(env.event as HostEvent);
    };
    void (async () => {
      try {
        const un = await host.subscribeHostEvents((env) => { if (ready) handle(env); else pending.push(env); });
        if (!alive) { un(); return; }
        unlisten.push(un);
        const unSel = await host.subscribeSelectedChat((id) => setSelId(id));
        if (!alive) { unSel(); return; }
        unlisten.push(unSel);
        void host.requestSelectedChat();
        const snap = await host.getSnapshot();
        if (!alive) return;
        last = snap.seq;
        pipe.flushAll();
        setBundle((b) => replaceSnapshot(b, snap));
        ready = true;
        for (const env of pending.splice(0)) handle(env);
      } catch (e) { ready = true; sayErr("ホストに接続できませんでした", e); }
    })();
    return () => { alive = false; unlisten.forEach((u) => u()); pipe.dispose(); };
  }, [sayErr]);

  const snap = bundle.snapshot;
  // 表示設定（テーマ・文字サイズ）。ホストの設定を受け取るまでは写しを上書きしない。
  const appearance = snap.settings.appearance;
  const settingsReady = snap.seq > 0;
  useEffect(() => {
    if (!settingsReady) return;
    applyAppearance(appearance);
    return watchSystemTheme(appearance);
  }, [settingsReady, appearance.theme, appearance.uiText, appearance.bodyText]); // eslint-disable-line react-hooks/exhaustive-deps
  const kind = snap.settings.monitorScope;
  const scope: MonitorScope = kind === "allChats"
    ? { kind: "allChats", showFinished }
    : { kind: "selectedChat", chat: selId ? { backend: "codex", id: selId } : null };
  const onScope = (s: MonitorScope) => {
    if (s.kind === "allChats") setShowFinished(s.showFinished);
    if (s.kind !== kind) host.setMonitorWindowScope(s.kind).catch((e) => sayErr("表示範囲を保存できませんでした", e));
  };
  const top = snap.settings.monitorWindow.alwaysOnTop;
  const openMain = (id: string | null) => {
    host.showMainWindow(id ? { backend: "codex", id } : null).catch((e) => sayErr("通常画面を開けませんでした", e));
  };

  // 確認済みの失敗（ホストの記録と現在の失敗turnの一致）。
  const confirmed = new Set(snap.agents.filter((a) => {
    if (a.status.state !== "failed") return false;
    const turn = a.status.turn ?? a.agent.latestTurn ?? "";
    const acked = snap.chatLocals.find((l) => l.chat.id === a.agent.chat.id)?.acknowledgedFailures ?? [];
    return acked.some((t) => t.agent.id === a.agent.key.id && t.turnId === turn);
  }).map((a) => keyStr(a.agent.key)));

  const pendingReqs = snap.requests.filter((r) => r.state.kind === "pending" && (kind === "allChats" || r.chat.id === selId));

  return (
    <div className="monitor-win" role="application" aria-label="AgentDock 監視">
      <div className="monitor-head">
        <label className="small"><input type="checkbox" checked={top} onChange={(e) => { host.setAlwaysOnTop("monitor", e.target.checked).catch((er) => sayErr("最前面を切り替えられませんでした", er)); }} />最前面</label>
        <span className="grow" />
        <button className="btn-line" onClick={() => openMain(null)}>通常画面を開く</button>
      </div>
      {pendingReqs.length ? (
        <div className="gbanner warn" role="status">
          <span className="grow">承認・質問の回答待ちが {pendingReqs.length} 件あります。通常画面を開いて回答します（ここでは回答しません）。</span>
          <button className="btn-line" onClick={() => openMain(pendingReqs[0].chat.id)}>通常画面で開く</button>
        </div>
      ) : null}
      <DockPane snap={snap} scope={scope} selId={selId} mini confirmed={confirmed} onScope={onScope}
        onOpen={(id) => openMain(id)} onConfirmFail={() => undefined} onAct={() => undefined} />
      {toast ? <div className="toast" role="status">{toast}</div> : null}
    </div>
  );
}

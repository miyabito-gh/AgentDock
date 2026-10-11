import { useCallback, useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { BackendStatus } from "../ipc/types";
import { LimitsView } from "./PrefsDialogs";
import { fetchedText, looksLoggedOut, unavailableText, usageLine } from "./usage";

/** 利用状況（読取りのみ）。開いたときと更新ボタンで取得し、常時ポーリングしない。
 *  `account/rateLimits/updated` の通知は、開いている間だけ読み直す合図に使う（通知・警告は出さない）。
 *  取れない項目は、取得不能・非対応・未確認を区別して理由つきで出す（0や空にしない）。 */
export function UsagePanel({ live }: { live: boolean }) {
  const [status, setStatus] = useState<BackendStatus | null>(null);
  const [err, setErr] = useState<{ connected: boolean; text: string } | null>(null);
  const [at, setAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const seq = useRef(0);

  const load = useCallback(async () => {
    if (!live) return;
    const mine = ++seq.current;
    setLoading(true);
    try {
      const s = await host.getBackendStatus();
      if (mine !== seq.current) return;
      setStatus(s); setErr(null); setAt(Date.now());
    } catch (e) {
      if (mine !== seq.current) return;
      const ie = host.asIpcError(e);
      setErr({ connected: ie.code !== "notConnected", text: ie.message });
    } finally {
      if (mine === seq.current) setLoading(false);
    }
  }, [live]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    if (!live) return;
    const un = host.subscribeHostEvents((e) => { if (e.event.kind === "rateLimitsChanged") void load(); });
    return () => { void un.then((f) => f()); };
  }, [live, load]);

  if (!live) return <p className="small muted">モックでは動きません（実接続で使えます）。</p>;

  const out = loggedOut(status);
  const limitsWhy = status ? unavailableText(status.rateLimits, status.rateLimitsError, out) : null;
  const usageWhy = status ? unavailableText(status.usage, status.usageError, out) : null;
  return (
    <div className="usage" aria-busy={loading}>
      <div className="usage-head">
        <b>利用状況</b>
        <span className="small muted" role="status">{loading ? "取得しています…" : at !== null ? fetchedText(at) : ""}</span>
        <span className="grow" />
        <button type="button" className="btn-line" disabled={loading} onClick={() => void load()}>更新</button>
      </div>
      {err ? (
        <p className="why err">{err.connected ? `取得できませんでした（${err.text}）` : "接続していません"}</p>
      ) : null}
      {status ? (
        <>
          <div className="usage-sec"><span className="small muted">利用上限</span>
            {limitsWhy ? <div className="why">{limitsWhy}</div> : <LimitsView k={status.rateLimits} />}</div>
          <div className="usage-sec"><span className="small muted">使用量</span>
            {usageWhy ? <div className="why">{usageWhy}</div> : <div>{status.usage.kind === "value" ? usageLine(status.usage.value) : null}</div>}</div>
          <p className="small muted">Codex が示した値のままで、残量・料金・回復には換算しません。{err ? "上は前回取得した内容です。" : ""}</p>
        </>
      ) : !err ? <p className="small muted">{loading ? "" : "未確認"}</p> : null}
    </div>
  );
}

const loggedOut = (s: BackendStatus | null): boolean => (s ? looksLoggedOut(s) : false);

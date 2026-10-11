// 会話内のパスの実在確認（読取りのメタデータだけ）。画面に見えたものを150msためて、1回の問い合わせにまとめる。
import { useCallback, useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { ChatKey, PathInfo } from "../ipc/types";

const BATCH = 50;
const DEBOUNCE_MS = 150;
/** 確認結果を使い回す時間。過ぎたら、見えたときに取り直す。 */
const TTL_MS = 30_000;
const PENDING_STALE_MS = 5_000;
const CACHE_MAX = 3_000;

interface Entry { at: number; info: PathInfo | null; pending: boolean }

export function usePathStats(live: boolean, chat: ChatKey | null): { pathInfo: (path: string) => PathInfo | undefined; requestPath: (path: string) => void } {
  const cache = useRef(new Map<string, Entry>());
  const queue = useRef(new Set<string>());
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [version, setVersion] = useState(0);
  const chatRef = useRef(chat);
  chatRef.current = chat;
  const id = chat?.id ?? null;
  const key = (cid: string, p: string) => `${cid}\0${p}`;

  useEffect(() => {
    queue.current.clear();
    return () => { if (timer.current) { clearTimeout(timer.current); timer.current = null; } };
  }, [id, live]);

  const flush = useCallback(async () => {
    timer.current = null;
    const c = chatRef.current;
    if (!c) return;
    const batch = [...queue.current].slice(0, BATCH);
    batch.forEach((p) => queue.current.delete(p));
    if (!batch.length) return;
    const store = (p: string, info: PathInfo | null) => cache.current.set(key(c.id, p), { at: Date.now(), info, pending: false });
    try {
      const res = await host.statPaths(c, batch);
      res.forEach((r) => store(r.input, r));
      batch.filter((p) => !res.some((r) => r.input === p)).forEach((p) => store(p, null));
    } catch {
      batch.forEach((p) => store(p, null));
    }
    setVersion((v) => v + 1);
    if (queue.current.size && !timer.current) timer.current = setTimeout(() => { void flush(); }, DEBOUNCE_MS);
  }, []);

  const requestPath = useCallback((path: string) => {
    const c = chatRef.current;
    if (!live || !c || /^[\\/]{2}/.test(path.trim())) return; // ネットワークパスは問い合わせない
    const k = key(c.id, path);
    const e = cache.current.get(k);
    const now = Date.now();
    if (e && now - e.at < (e.pending ? PENDING_STALE_MS : TTL_MS)) return;
    if (cache.current.size > CACHE_MAX) cache.current.clear();
    cache.current.set(k, { at: now, info: e?.info ?? null, pending: true });
    queue.current.add(path);
    if (!timer.current) timer.current = setTimeout(() => { void flush(); }, DEBOUNCE_MS);
  }, [live, id, flush]);

  // eslint-disable-next-line react-hooks/exhaustive-deps
  const pathInfo = useCallback((path: string) => (id ? cache.current.get(key(id, path))?.info ?? undefined : undefined), [id, version]);
  return { pathInfo, requestPath };
}

// ペット窓（別の窓、`?view=pet`）。透明・最前面の小窓に、選んだペットの動きと、完了・失敗・承認待ちの吹き出しを出す（DESIGN_PET）。
// 承認・質問・停止・送信はここでは行わない。吹き出しを押すと通常画面を開いて該当チャットを表示するだけ。
// 透明部分のクリックは下の窓へ通す（ホストがカーソル位置を見て切り替える。ここでは「クリックを受ける領域」を知らせる）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ChatKey, HostEvent, HostEventEnvelope } from "./ipc/types";
import * as host from "./ipc/client";
import type { PetBubbleMessage } from "./ipc/client";
import { applyHostEvent, createEventPipeline, emptyBundle, replaceSnapshot, seqAction } from "./ipc/live";
import type { Bundle } from "./mock/data";
import { keyStr } from "./ui/format";
import { applyAppearance, watchSystemTheme } from "./ui/appearance";
import { ChatPane, MenuPane, RequestPane } from "./pet/PetCard";
import { chooseTarget, pendingRequests } from "./pet/cardLogic";
import { cellOf, choosePose, emptyTracker, FRAME_H, FRAME_W, gazeIndex, isActive, isIdlePose, SHEET_H, SHEET_W, trackAgents, type AgentLite, type PetTracker } from "./pet/motion";

// ホスト `win::pet` の窓サイズと同じ数値（吹き出しの領域の大きさ）。
const BUBBLE_ZONE_H = 96;
const BUBBLE_MAX_W = 240;
// カードを開いているときの窓の上側の領域（ホスト `win::pet` の CARD_W / CARD_H と同じ数値）。
const CARD_ZONE_H = 420;
const CARD_MAX_W = 300;
/** 吹き出しを出しておく時間。承認・質問は長めにする。押す・次の吹き出しで入れ替わる。 */
const BUBBLE_MS = 10_000;
const BUBBLE_ATTENTION_MS = 20_000;
const BUBBLE_MAX_KEPT = 5;
const DRAG_PX = 4;
const DRAG_IDLE_MS = 250;
const GAZE_MIN_GAP_MS = 5000;
const GAZE_RANGE_MS = 7000;
const GAZE_HOLD_MS = 2500;
const GAZE_POLL_MS = 120;
const GAZE_DEAD_ZONE_PX = 24;

type BubbleKind = PetBubbleMessage["kind"] | "info";
interface Bubble extends Omit<PetBubbleMessage, "kind"> { kind: BubbleKind; id: number; until: number }
type Panel = null | { kind: "card"; mode: "request" | "chat" } | { kind: "menu" };
type Sprite = { state: "none" } | { state: "loading" } | { state: "error"; text: string } | { state: "ready"; url: string };

const KIND_CLASS: Record<BubbleKind, string> = {
  info: "done", awaitingAnswer: "wait", orgAttention: "wait", completed: "done", orgCompleted: "done", failed: "fail", orgFailed: "fail",
};

export default function PetApp() {
  const [bundle, setBundle] = useState<Bundle>(() => emptyBundle());
  const [now, setNow] = useState(() => Date.now());
  const [reduced, setReduced] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const [sprite, setSprite] = useState<Sprite>({ state: "none" });
  const [bubbles, setBubbles] = useState<Bubble[]>([]);
  const [drag, setDrag] = useState<"left" | "right" | null>(null);
  const [jumpAt, setJumpAt] = useState<number | null>(null);
  const [failedAt, setFailedAt] = useState<number | null>(null);
  const [doneAt, setDoneAt] = useState<number | null>(null);
  const [lost, setLost] = useState(false);
  const [gaze, setGaze] = useState<number | null>(null);
  const [panel, setPanel] = useState<Panel>(null);
  const [notified, setNotified] = useState<string | null>(null);
  const [selectedChat, setSelectedChat] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [pausedUntil, setPausedUntil] = useState<number | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const spriteRef = useRef<HTMLDivElement>(null);
  const bubbleRef = useRef<HTMLButtonElement>(null);
  const trackerRef = useRef<PetTracker>(emptyTracker());
  const bubbleId = useRef(0);
  const idleRef = useRef(false);

  // 購読 → スナップショット。seqが飛んだら取り直す（通常画面・監視窓と同じ規則）。
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
      } catch { /* 取り直せなければ表示は古いまま。次のイベントで再試行する */ } finally { resyncing = false; }
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
        const unBubble = await host.subscribePetBubble((m) => {
          const t = Date.now();
          if (m.chat) setNotified(m.chat.id);
          const b: Bubble = { ...m, id: ++bubbleId.current, until: t + (m.kind === "awaitingAnswer" || m.kind === "orgAttention" ? BUBBLE_ATTENTION_MS : BUBBLE_MS) };
          setBubbles((list) => [...list, b].slice(-BUBBLE_MAX_KEPT));
        });
        if (!alive) { unBubble(); return; }
        unlisten.push(unBubble);
        const unSel = await host.subscribeSelectedChat((id) => setSelectedChat(id));
        if (!alive) { unSel(); return; }
        unlisten.push(unSel);
        void host.requestSelectedChat();
        host.petPauseGet().then(setPausedUntil).catch(() => undefined);
        const snap = await host.getSnapshot();
        if (!alive) return;
        last = snap.seq;
        pipe.flushAll();
        setBundle((b) => replaceSnapshot(b, snap));
        ready = true;
        for (const env of pending.splice(0)) handle(env);
      } catch { ready = true; }
    })();
    return () => { alive = false; unlisten.forEach((u) => u()); pipe.dispose(); };
  }, []);

  const snap = bundle.snapshot;
  const settingsReady = snap.seq > 0;
  const appearance = snap.settings.appearance;
  useEffect(() => {
    if (!settingsReady) return;
    applyAppearance(appearance);
    return watchSystemTheme(appearance);
  }, [settingsReady, appearance.theme, appearance.uiText, appearance.bodyText]); // eslint-disable-line react-hooks/exhaustive-deps

  // 動きを減らす設定（OS）。
  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);

  // 状態の材料: 全チャットのエージェント状態と承認・質問待ちの要求（既存のスナップショットだけ）。
  const agents: AgentLite[] = useMemo(() => snap.agents.map((a) => ({
    key: keyStr(a.agent.key), root: a.agent.parent.kind === "root", state: a.status.state,
    stale: a.freshness === "disconnected" || a.freshness === "needsReconcile",
  })), [snap.agents]);
  const pending = useMemo(() => snap.requests.some((r) => r.state.kind === "pending"), [snap.requests]);
  const running = useMemo(() => agents.some((a) => isActive(a.state)), [agents]);
  useEffect(() => {
    const r = trackAgents(trackerRef.current, agents, settingsReady);
    trackerRef.current = r.tracker;
    const t = Date.now();
    if (r.completed) setDoneAt(t);
    if (r.failed) setFailedAt(t);
    setLost(r.tracker.lost.length > 0);
  }, [agents, settingsReady]);

  // 吹き出しの期限切れ。
  useEffect(() => {
    if (bubbles.length === 0) return;
    const next = Math.min(...bubbles.map((b) => b.until));
    const t = window.setTimeout(() => setBubbles((l) => l.filter((b) => b.until > Date.now())), Math.max(50, next - Date.now()));
    return () => window.clearTimeout(t);
  }, [bubbles]);

  // 選んだペットの画像を読む（読み取り専用。Blob URLにして使い、切替・終了時に解放する）。
  const selected = snap.settings.pet.selected;
  useEffect(() => {
    if (!settingsReady) return;
    if (!selected) { setSprite({ state: "none" }); return; }
    let alive = true;
    let url: string | null = null;
    setSprite({ state: "loading" });
    void (async () => {
      try {
        const entry = (await host.petList()).find((e) => e.folder === selected);
        if (!entry) throw new Error("選んだペットが見つかりません");
        if (!entry.supported) throw new Error(entry.unsupportedReason ?? "非対応のペットです");
        const buf = await host.petSprite(selected);
        if (!alive) return;
        url = URL.createObjectURL(new Blob([buf], { type: entry.mime ?? "image/png" }));
        setSprite({ state: "ready", url });
      } catch (e) {
        if (alive) setSprite({ state: "error", text: host.asIpcError(e).message });
      }
    })();
    return () => { alive = false; if (url) URL.revokeObjectURL(url); };
  }, [selected, settingsReady]);

  // 動きの選択（純粋ロジック `pet/motion.ts`）。
  const inputs = { now, reducedMotion: reduced, drag, jumpAt, pending, failedAt, doneAt, running, gaze: null as number | null };
  const basePose = choosePose(inputs);
  idleRef.current = isIdlePose(basePose);
  const pose = gaze !== null && idleRef.current ? choosePose({ ...inputs, gaze }) : basePose;
  const cell = cellOf(pose, now);

  // コマ送り: 次のコマの境目で描き直す。静止（動きを減らす設定・目線）は間隔を空けて、時間切れの判定だけ行う。
  useEffect(() => {
    const t = window.setTimeout(() => setNow(Date.now()), Math.max(30, Math.min(cell.remaining ?? 250, 1000)));
    return () => window.clearTimeout(t);
  }, [now, cell.remaining]);

  // ときどき目線がカーソルを向く（待機中だけ。カーソル位置が取れなければ向かない）。
  const followGaze = snap.settings.pet.followGaze;
  useEffect(() => {
    if (reduced || !followGaze) { setGaze(null); return; }
    let alive = true;
    let timer: number | undefined;
    const schedule = () => { timer = window.setTimeout(start, GAZE_MIN_GAP_MS + Math.random() * GAZE_RANGE_MS); };
    const start = () => {
      if (!alive) return;
      if (!idleRef.current) { schedule(); return; }
      const end = Date.now() + GAZE_HOLD_MS;
      const step = async () => {
        if (!alive) return;
        if (Date.now() >= end || !idleRef.current) { setGaze(null); schedule(); return; }
        try {
          const c = await host.petCursor();
          const r = spriteRef.current?.getBoundingClientRect();
          if (alive && c && r) {
            const dx = c.x - (r.x + r.width / 2);
            const dy = c.y - (r.y + r.height / 2);
            setGaze(Math.hypot(dx, dy) < GAZE_DEAD_ZONE_PX ? null : gazeIndex(dx, dy));
          }
        } catch { /* 取れなければ正面のまま */ }
        timer = window.setTimeout(step, GAZE_POLL_MS);
      };
      void step();
    };
    schedule();
    return () => { alive = false; window.clearTimeout(timer); setGaze(null); };
  }, [reduced, followGaze]);

  // クリックを受ける領域（スプライトと吹き出し）をホストへ知らせる。それ以外の透明部分はクリックが下の窓へ通る。
  const lastRegions = useRef("");
  useEffect(() => {
    const rs = [spriteRef.current, bubbleRef.current, panelRef.current].flatMap((el) => {
      if (!el) return [];
      const r = el.getBoundingClientRect();
      return [{ x: r.x, y: r.y, width: r.width, height: r.height }];
    });
    const key = JSON.stringify(rs);
    if (key === lastRegions.current) return;
    lastRegions.current = key;
    host.petSetHitRegions(rs).catch(() => { lastRegions.current = ""; });
  });

  // ドラッグ: 動かすと窓ごと移動（OSのドラッグ）。移動の向きで running-left / right。クリック（動かさない）は1回跳ぶ。
  const press = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  const dragSession = useRef(false);
  const dragTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    let un: (() => void) | undefined;
    let alive = true;
    let lastX: number | null = null;
    getCurrentWindow().onMoved(({ payload }) => {
      if (!dragSession.current) { lastX = payload.x; return; }
      if (lastX !== null && payload.x !== lastX) setDrag(payload.x < lastX ? "left" : "right");
      lastX = payload.x;
      window.clearTimeout(dragTimer.current);
      dragTimer.current = window.setTimeout(() => setDrag(null), DRAG_IDLE_MS);
    }).then((u) => { if (alive) un = u; else u(); }).catch(() => undefined);
    return () => { alive = false; un?.(); window.clearTimeout(dragTimer.current); };
  }, []);
  const endDragSession = () => { if (dragSession.current) { dragSession.current = false; window.clearTimeout(dragTimer.current); setDrag(null); } };
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || !spriteRef.current?.contains(e.target as Node)) return;
    endDragSession();
    press.current = { x: e.clientX, y: e.clientY, moved: false };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    // OSのドラッグが終わるとポインター操作が戻ってくる（ボタンは離れている）。
    if (dragSession.current && e.buttons === 0) endDragSession();
    const p = press.current;
    if (!p || p.moved || (e.buttons & 1) === 0) return;
    if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < DRAG_PX) return;
    p.moved = true;
    dragSession.current = true;
    getCurrentWindow().startDragging().catch(() => { dragSession.current = false; });
  };
  const onPointerUp = () => {
    const p = press.current;
    press.current = null;
    if (p && !p.moved) { setJumpAt(Date.now()); toggleCard(); }
    endDragSession();
  };
  // カード（簡易チャット・回答・メニュー）。開くときは先に窓を広げ、閉じるときは先に描画を畳んでから窓を戻す。
  const panelOpen = useRef(false);
  const showPanel = useCallback((p: NonNullable<Panel>) => {
    const wasOpen = panelOpen.current;
    panelOpen.current = true;
    if (wasOpen) { setPanel(p); return; }
    host.petSetCard(true).catch(() => undefined).finally(() => { if (panelOpen.current) setPanel(p); });
  }, []);
  const hidePanel = useCallback(() => {
    if (!panelOpen.current) return;
    panelOpen.current = false;
    setPanel(null);
    window.setTimeout(() => { if (!panelOpen.current) host.petSetCard(false).catch(() => undefined); }, 120);
  }, []);
  const pendingReqs = useMemo(() => pendingRequests(snap.requests), [snap.requests]);
  const pendingRef = useRef(pendingReqs);
  pendingRef.current = pendingReqs;
  const panelRefState = useRef<Panel>(null);
  panelRefState.current = panel;
  const toggleCard = () => {
    if (panelRefState.current?.kind === "card") hidePanel();
    else showPanel({ kind: "card", mode: pendingRef.current.length > 0 ? "request" : "chat" });
  };
  const pushInfo = useCallback((title: string) => {
    setBubbles((l) => [...l, { kind: "info" as const, title, body: "", chat: null, id: ++bubbleId.current, until: Date.now() + 3500 }].slice(-BUBBLE_MAX_KEPT));
  }, []);
  // 回答待ちが無くなったら（回答済み・他の経路で解決済み）、回答のカードをすぐ閉じる。
  useEffect(() => {
    if (panel?.kind === "card" && panel.mode === "request" && pendingReqs.length === 0) hidePanel();
  }, [panel, pendingReqs.length, hidePanel]);
  // Esc でカード・メニューを閉じる。
  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === "Escape") hidePanel(); };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [hidePanel]);
  const openMain = useCallback((chat: ChatKey | null) => { host.showMainWindow(chat).catch(() => undefined); }, []);

  const top = bubbles.length > 0 && !panel ? bubbles[bubbles.length - 1] : null;
  const clickBubble = (b: Bubble) => {
    setBubbles((l) => l.filter((x) => x.id !== b.id));
    if (b.kind === "info") return;
    // 承認・質問の吹き出しは、その場で回答できるカードを開く（回答待ちが見つからなければ通常画面）。
    if ((b.kind === "awaitingAnswer") && pendingRef.current.length > 0) { showPanel({ kind: "card", mode: "request" }); return; }
    openMain(b.chat);
  };
  const knownIds = useMemo(() => new Set(snap.chats.map((c) => c.key.id)), [snap.chats]);
  const target = chooseTarget({ picked, notified, selected: selectedChat }, knownIds);
  const setPet = (p: typeof snap.settings.pet) => { host.petSetSettings(p).catch(() => pushInfo("設定を保存できませんでした")); };
  const pause = (minutes: number | null) => { host.petPause(minutes).then(setPausedUntil).catch(() => pushInfo("通知の設定を変えられませんでした")); };

  const s = snap.settings.pet.scalePercent / 100;
  const w = FRAME_W * s;
  const h = Math.round(FRAME_H * s);
  const sheetStyle = sprite.state === "ready" ? {
    backgroundImage: `url(${sprite.url})`, backgroundSize: `${SHEET_W * s}px ${SHEET_H * s}px`,
    backgroundPosition: `${-cell.col * FRAME_W * s}px ${-cell.row * FRAME_H * s}px`,
  } : undefined;
  const note = sprite.state === "none" ? "ペットが未選択です（設定で選べます）" : sprite.state === "error" ? `ペットを読み込めません: ${sprite.text}` : sprite.state === "loading" ? "読み込み中…" : null;
  const showLost = lost && isIdlePose(pose);

  return (
    <div className="pet-root" aria-label="AgentDock ペット" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onContextMenu={(e) => { e.preventDefault(); if (panelRefState.current?.kind === "menu") hidePanel(); else showPanel({ kind: "menu" }); }}>
      <div className={`pet-bubble-zone${panel ? " open" : ""}`} style={{ height: panel ? CARD_ZONE_H : BUBBLE_ZONE_H }}>
        {panel && (
          <div ref={panelRef} className="pet-panel" style={{ maxWidth: CARD_MAX_W }}>
            <button type="button" className="pet-panel-x" aria-label="閉じる" onClick={hidePanel}>×</button>
            {panel.kind === "menu" ? (
              <MenuPane pet={snap.settings.pet} pausedUntil={pausedUntil} onSet={setPet} onPause={pause}
                onHide={() => setPet({ ...snap.settings.pet, visible: false })} onOpenMain={() => { hidePanel(); openMain(null); }} />
            ) : panel.mode === "request" && pendingReqs.length > 0 ? (
              <RequestPane r={pendingReqs[0]} chats={snap.chats} onOpenChat={(k) => { hidePanel(); openMain(k); }}
                onFinished={(t) => { hidePanel(); pushInfo(t); }} onToChat={() => showPanel({ kind: "card", mode: "chat" })} />
            ) : (
              <ChatPane snap={snap} turns={bundle.turns} targetId={target} onPick={setPicked} onOpenChat={(k) => { hidePanel(); openMain(k); }}
                pendingCount={pendingReqs.length} onToRequest={() => showPanel({ kind: "card", mode: "request" })} />
            )}
          </div>
        )}
        {top && (
          <button ref={bubbleRef} type="button" className={`pet-bubble ${KIND_CLASS[top.kind]}`} style={{ maxWidth: BUBBLE_MAX_W }} onClick={() => clickBubble(top)}
            title={top.chat ? "押すと該当のチャットを開きます" : "押すとAgentDockを開きます"}>
            <span className="pet-bubble-title">{top.title}{bubbles.length > 1 ? `（ほか${bubbles.length - 1}件）` : ""}</span>
            <span className="pet-bubble-body">{top.body}</span>
          </button>
        )}
      </div>
      <div ref={spriteRef} className="pet-sprite" style={{ width: note ? undefined : w, height: h, ...sheetStyle }} onDoubleClick={() => openMain(null)}
        role="img" aria-label={showLost ? "ペット（状態不明のチャットがあります）" : "ペット"}>
        {note && <span className="pet-note">{note}</span>}
        {showLost && <span className="pet-unknown" title="作業中だったチャットの状態が不明です（完了とは限りません）">?</span>}
      </div>
    </div>
  );
}

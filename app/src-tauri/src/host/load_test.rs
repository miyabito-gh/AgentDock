//! 負荷の測定（P3-9、`app/DESIGN_P3.md` §1 #18）。偽のイベント源（`BackendEvent` を直接流す）で、
//! 20ルート・合計100agent・2秒間の状態変化をホストの受信処理（`handle_backend_event`＝受信→`HostEvent`生成）へ流し、
//! キューの滞留と1イベントあたりの処理時間を測る。数値目標は確定させない（正本 §6）。画面側の表示までの時間は実機で見る。
//! 実行: `cargo test --lib host_load -- --ignored --nocapture`

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::*;

const ROOTS: usize = 20;
const PER_ROOT: usize = 5; // ルート1＋子孫4＝合計100agent
const TICKS: usize = 20; // 100ms × 20 = 2秒

fn ak(s: &str) -> AgentKey {
    AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
}
fn ev() -> Evidence {
    Evidence { source: EvidenceSource::LiveEvent, raw_label: Some("load".into()), source_time: None, observed_at: now_ms() }
}
fn status(state: AgentState, turn: &str) -> AgentStatus {
    AgentStatus { state, raw: RawState { label: "load".into() }, scope: StateScope::Turn, turn: Some(ExternalId(turn.into())), wait: None, evidence: ev() }
}
fn agent(root: usize, k: usize) -> Agent {
    let id = format!("r{root}-a{k}");
    Agent {
        key: ak(&id),
        chat: ChatKey { backend: BackendKind::Codex, id: ExternalId(format!("r{root}-a0")) },
        parent: if k == 0 { ParentLink::Root } else { ParentLink::Explicit { parent: ak(&format!("r{root}-a0")) } },
        forked_from: Known::NotFetched,
        display_name: Known::NotFetched,
        role: Known::NotFetched,
        assignment: Known::NotFetched,
        agent_path: Known::NotFetched,
        latest_turn: None,
    }
}
fn item(a: &str, n: usize) -> ItemKey {
    ItemKey { agent: ak(a), turn_id: Some(ExternalId("t1".into())), item_id: ExternalId(format!("i{n}")) }
}

/// 1 tick 分のイベント（tick 0 は発見と開始、最後の tick は終了）。
fn tick_events(tick: usize) -> Vec<BackendEvent> {
    let mut out = Vec::new();
    for r in 0..ROOTS {
        for k in 0..PER_ROOT {
            let id = format!("r{r}-a{k}");
            if tick == 0 {
                out.push(BackendEvent::AgentDiscovered { agent: agent(r, k) });
                out.push(BackendEvent::TurnStarted { turn: TurnKey { agent: ak(&id), turn_id: ExternalId("t1".into()) }, evidence: ev() });
                out.push(BackendEvent::AgentStatus { agent: ak(&id), status: status(AgentState::Running, "t1") });
            }
            out.push(BackendEvent::Activity {
                activity: Activity {
                    key: item(&id, tick),
                    kind: ActivityKind::Command,
                    phase: ActivityPhase::InProgress,
                    summary: Known::direct(format!("step {tick}")),
                    evidence: ev(),
                },
            });
            for _ in 0..3 {
                out.push(BackendEvent::ActivityDelta { item: item(&id, tick), delta: "x".repeat(40) });
            }
            if tick == TICKS - 1 {
                out.push(BackendEvent::AgentStatus { agent: ak(&id), status: status(AgentState::Idle, "t1") });
                out.push(BackendEvent::TurnEnded { turn: TurnKey { agent: ak(&id), turn_id: ExternalId("t1".into()) }, end: TurnEnd::Completed, error: None, evidence: ev() });
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "load measurement; run explicitly with --ignored --nocapture"]
async fn host_load_20_roots_100_agents_2s() {
    let dir = std::env::temp_dir().join(format!("agentdock-load-{}", std::process::id()));
    let host = Arc::new(Host::new(dir));
    let emitted = Arc::new(AtomicUsize::new(0));
    let e2 = emitted.clone();
    host.set_emitter(Arc::new(move |_| {
        e2.fetch_add(1, Ordering::Relaxed);
    }));

    // 実際の受信経路と同じ有界チャネル（アダプターの有界キュー相当）。
    let (tx, mut rx) = mpsc::channel::<(Instant, EventEnvelope)>(1024);
    let producer = tokio::spawn(async move {
        let mut seq = 0u64;
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        let mut sent = 0usize;
        let mut blocked = Duration::ZERO;
        for tick in 0..TICKS {
            interval.tick().await;
            for event in tick_events(tick) {
                seq += 1;
                let env = EventEnvelope { source: SourceId("load".into()), seq, received_at: now_ms(), event };
                let t = Instant::now();
                tx.send((Instant::now(), env)).await.unwrap();
                blocked += t.elapsed();
                sent += 1;
            }
        }
        (sent, blocked)
    });

    let mut handle_times: Vec<Duration> = Vec::new();
    let mut lags: Vec<Duration> = Vec::new();
    let mut max_depth = 0usize;
    let started = Instant::now();
    while let Some((sent_at, env)) = rx.recv().await {
        max_depth = max_depth.max(rx.len());
        lags.push(sent_at.elapsed());
        let t = Instant::now();
        host.handle_backend_event(&env);
        handle_times.push(t.elapsed());
    }
    let total = started.elapsed();
    let (sent, blocked) = producer.await.unwrap();

    handle_times.sort();
    lags.sort();
    let p = |v: &Vec<Duration>, q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)];
    println!(
        "events={sent} handled={} total={total:?} emittedHostEvents={} maxChannelDepth={max_depth} producerBlocked={blocked:?}",
        handle_times.len(),
        emitted.load(Ordering::Relaxed)
    );
    println!("handle time: p50={:?} p99={:?} max={:?}", p(&handle_times, 0.5), p(&handle_times, 0.99), handle_times.last().unwrap());
    println!("queue lag (send->start of handling): p50={:?} p99={:?} max={:?}", p(&lags, 0.5), p(&lags, 0.99), lags.last().unwrap());

    assert_eq!(handle_times.len(), sent, "every event is handled");
    assert_eq!(host.snapshot().agents.len(), ROOTS * PER_ROOT, "all agents are tracked");
}

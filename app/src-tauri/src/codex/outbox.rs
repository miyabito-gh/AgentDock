//! ホスト監視層へ渡すイベントの送出バッファ。
//!
//! - 順序を保つ単一キュー。満杯のとき落としてよいのは活動表示系（[`is_critical`] が偽）だけ。
//!   承認・質問・turn終端・接続状態など重要なイベントは落とさない（必要なら容量を超えて保持する）。
//! - 何かを落としたら、その事実を `Gap` として必ず送る。新しいイベントを待たず、消費側が空いた時点で送る。
//! - `seq` は実際に送り出す順に採番する（欠番を作らない）。

use crate::backend::backend::{BackendEvent, EventEnvelope, GapScope};
use crate::backend::model::{SourceId, UnixMillis};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Notify};

fn now_ms() -> UnixMillis {
    UnixMillis(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
}

/// 落としてはいけないイベント。活動表示（本文・item・警告・reroute）だけが落としてよい。
pub fn is_critical(e: &BackendEvent) -> bool {
    !matches!(
        e,
        BackendEvent::Activity { .. } | BackendEvent::ActivityDelta { .. } | BackendEvent::Unrecognized { .. } | BackendEvent::ModelRerouted { .. }
    )
}

struct Queued {
    source: SourceId,
    received_at: UnixMillis,
    event: BackendEvent,
    critical: bool,
}

#[derive(Default)]
struct State {
    items: VecDeque<Queued>,
    /// 落としたイベントがある（Gapをまだ送っていない）。値は対象の監視元。
    gap_source: Option<SourceId>,
}

pub struct Outbox {
    cap: usize,
    state: Mutex<State>,
    notify: Notify,
    seq: AtomicU64,
}

impl Outbox {
    pub fn new(cap: usize) -> Self {
        Outbox { cap: cap.max(1), state: Mutex::new(State::default()), notify: Notify::new(), seq: AtomicU64::new(0) }
    }

    pub fn push(&self, source: &SourceId, event: BackendEvent) {
        let critical = is_critical(&event);
        {
            let mut st = self.state.lock().unwrap();
            if st.items.len() >= self.cap {
                if critical {
                    // 古い非重要イベントを1件追い出して場所を作る。無ければ容量を超えて保持する。
                    if let Some(pos) = st.items.iter().position(|q| !q.critical) {
                        st.items.remove(pos);
                        st.gap_source = Some(source.clone());
                    }
                } else {
                    st.gap_source = Some(source.clone());
                    drop(st);
                    self.notify.notify_one(); // Gapを送らせる
                    return;
                }
            }
            st.items.push_back(Queued { source: source.clone(), received_at: now_ms(), event, critical });
        }
        self.notify.notify_one();
    }

    fn envelope(&self, source: SourceId, received_at: UnixMillis, event: BackendEvent) -> EventEnvelope {
        EventEnvelope { source, seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1, received_at, event }
    }

    /// 次に送るもの。落とした事実（Gap）が最優先。
    pub fn pop(&self) -> Option<EventEnvelope> {
        let mut st = self.state.lock().unwrap();
        if let Some(src) = st.gap_source.take() {
            drop(st);
            return Some(self.envelope(
                src,
                now_ms(),
                BackendEvent::Gap { scope: GapScope::Source, reason: "event queue was full; non-critical events were dropped".into() },
            ));
        }
        let q = st.items.pop_front()?;
        drop(st);
        Some(self.envelope(q.source, q.received_at, q.event))
    }

    pub fn len(&self) -> usize {
        self.state.lock().unwrap().items.len()
    }

    /// 送出task。消費側が空くまで待ち、Gapを含めて順に送る。受け口が閉じたら終了。
    pub async fn forward(self: std::sync::Arc<Self>, tx: mpsc::Sender<EventEnvelope>) {
        loop {
            while let Some(env) = self.pop() {
                if tx.send(env).await.is_err() {
                    return;
                }
            }
            self.notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{AgentKey, BackendKind, ExternalId, RequestKey};
    use std::sync::Arc;
    use std::time::Duration;

    fn src() -> SourceId {
        SourceId("s".into())
    }

    fn activity(n: &str) -> BackendEvent {
        BackendEvent::Unrecognized { raw_label: n.into(), note: None }
    }

    fn resolved() -> BackendEvent {
        use crate::backend::model::{Evidence, EvidenceSource};
        BackendEvent::RequestResolved {
            request: RequestKey { backend: BackendKind::Codex, source: src(), request_id: ExternalId("n:1".into()) },
            evidence: Evidence { source: EvidenceSource::LiveEvent, raw_label: None, source_time: None, observed_at: UnixMillis(0) },
        }
    }

    fn label(e: &EventEnvelope) -> String {
        match &e.event {
            BackendEvent::Unrecognized { raw_label, .. } => raw_label.clone(),
            BackendEvent::Gap { .. } => "GAP".into(),
            BackendEvent::RequestResolved { .. } => "RESOLVED".into(),
            BackendEvent::Connection { .. } => "CONN".into(),
            _ => "other".into(),
        }
    }

    #[test]
    fn dropped_noncritical_events_are_reported_as_gap_first() {
        let o = Outbox::new(2);
        o.push(&src(), activity("a"));
        o.push(&src(), activity("b"));
        o.push(&src(), activity("c")); // 満杯: 落ちる
        let got: Vec<_> = std::iter::from_fn(|| o.pop()).map(|e| label(&e)).collect();
        assert_eq!(got, ["GAP", "a", "b"]);
        assert!(o.pop().is_none());
    }

    #[test]
    fn critical_events_are_never_dropped_even_when_full() {
        let o = Outbox::new(2);
        o.push(&src(), activity("a"));
        o.push(&src(), activity("b"));
        o.push(&src(), resolved()); // 満杯: 古い非重要(a)を追い出して入る
        o.push(&src(), resolved()); // 非重要(b)を追い出す
        o.push(&src(), resolved()); // 非重要なし: 容量を超えて保持
        assert_eq!(o.len(), 3);
        let got: Vec<_> = std::iter::from_fn(|| o.pop()).map(|e| label(&e)).collect();
        assert_eq!(got, ["GAP", "RESOLVED", "RESOLVED", "RESOLVED"]);
    }

    #[test]
    fn seq_follows_send_order_without_holes_and_connection_is_critical() {
        let o = Outbox::new(1);
        o.push(&src(), activity("a"));
        o.push(&src(), activity("x")); // 落ちる
        o.push(&src(), BackendEvent::Connection { state: crate::backend::model::ConnectionState::Connected });
        let seqs: Vec<_> = std::iter::from_fn(|| o.pop()).map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 2]);
        let _ = AgentKey { backend: BackendKind::Codex, id: ExternalId("x".into()) };
    }

    #[tokio::test]
    async fn gap_is_delivered_when_consumer_frees_up_without_new_events() {
        let o = Arc::new(Outbox::new(1));
        let (tx, mut rx) = mpsc::channel(1);
        tokio::spawn(o.clone().forward(tx));
        // 受け口(容量1)は誰も読まない。forwarderが1件を保持して詰まる間に、さらに積む。
        for i in 0..5 {
            o.push(&src(), activity(&format!("e{i}")));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut seen = Vec::new();
        // 以降は新しいpushなしで読むだけ。Gapが必ず届く。
        while let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
            seen.push(label(&e));
        }
        assert!(seen.iter().any(|l| l == "GAP"), "{seen:?}");
    }
}

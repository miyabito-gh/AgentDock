//! 改行区切りJSON-RPCクライアント。`AsyncRead`/`AsyncWrite`に対して動くのでプロセスなしでテストできる。
//!
//! 規則（DESIGN_T1 §4）:
//! - 切断・timeout・書込み失敗は [`BackendError::OutcomeUnknown`]。成功にも `Rejected` にもしない。
//!   `Rejected` はエラー応答を受けた場合だけ。
//! - 自動再送・自動回答はしない。サーバー要求への応答は呼び出し側が明示的に行う。
//! - 通知とサーバー要求は受信順に `seq` を振り、有界mpscへ転送する（満杯なら読取りを待つ。黙って捨てない）。
//!   解釈できない行は `RpcEvent::Malformed` として渡す。

use super::id::RpcId;
use crate::backend::backend::BackendError;
use crate::backend::model::{BackendKind, ExternalId, RequestKey, SourceId};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};

type Pending = HashMap<i64, oneshot::Sender<Result<Value, BackendError>>>;

/// 受信側へ渡すイベント。`seq` は接続内で通知・サーバー要求・不正行に共通の受信順。
#[derive(Debug, Clone, PartialEq)]
pub enum RpcEvent {
    Notification { seq: u64, method: String, params: Value },
    /// サーバー要求（承認・質問等）。応答は [`RpcClient::respond`] / [`RpcClient::respond_error`]。
    ServerRequest { seq: u64, key: RequestKey, method: String, params: Value },
    /// 対応する保留要求がない応答（timeout後の遅延応答など）。
    UnmatchedResponse { seq: u64, id: Value },
    /// JSONとして/JSON-RPCとして解釈できない行。接続は止めない。
    Malformed { seq: u64, note: String },
    /// 受信側が遅れて内部の待ち行列が上限を超え、通知を落とした（少なくとも1件）。欠落扱い（Gap）にすること。
    /// 応答の照合・サーバー要求・切断は落とさない。
    Overflow { seq: u64 },
    /// 接続終了。以後イベントは来ない。
    Disconnected { reason: String },
}

struct Inner {
    source: SourceId,
    next_id: AtomicI64,
    writer: Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>,
    state: StdMutex<State>,
    write_timeout: Duration,
    /// 書込みtimeout等で接続を打ち切るとき、読取りtaskを起こして終わらせる。
    abort: Notify,
}

const DEFAULT_BACKLOG_LIMIT: usize = 10_000;
const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// `closed` は `state` のロック内でのみ変更・判定する（登録と排出の競合を避ける）。
#[derive(Default)]
struct State {
    closed: bool,
    pending: Pending,
    /// 応答待ちのサーバー要求ID（二重応答・未知ID応答の防止）。
    server_pending: HashSet<ExternalId>,
}

#[derive(Clone)]
pub struct RpcClient {
    inner: Arc<Inner>,
}

impl RpcClient {
    /// 読取りtaskを起動する。tokioランタイム内で呼ぶこと。
    pub fn start<R, W>(reader: R, writer: W, source: SourceId, event_capacity: usize) -> (RpcClient, mpsc::Receiver<RpcEvent>)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::start_with(reader, writer, source, event_capacity, DEFAULT_BACKLOG_LIMIT, DEFAULT_WRITE_TIMEOUT)
    }

    /// `backlog_limit`: 受信側が遅れたとき、読取りtaskと受け口の間に溜める通知の上限（超過分は落として `Overflow`）。
    /// 応答の照合は受け口の混雑に影響されない。`write_timeout` 超過は結果不明として接続を閉じる。
    pub fn start_with<R, W>(
        reader: R,
        writer: W,
        source: SourceId,
        event_capacity: usize,
        backlog_limit: usize,
        write_timeout: Duration,
    ) -> (RpcClient, mpsc::Receiver<RpcEvent>)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (tx, rx) = mpsc::channel(event_capacity.max(1));
        let (utx, urx) = mpsc::unbounded_channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        let inner = Arc::new(Inner {
            source,
            next_id: AtomicI64::new(1),
            writer: Mutex::new(Some(Box::new(writer))),
            state: StdMutex::new(State::default()),
            write_timeout,
            abort: Notify::new(),
        });
        tokio::spawn(forward(urx, tx, backlog.clone()));
        tokio::spawn(read_loop(reader, inner.clone(), utx, backlog, backlog_limit.max(1)));
        (RpcClient { inner }, rx)
    }

    pub fn source(&self) -> &SourceId {
        &self.inner.source
    }

    pub fn is_closed(&self) -> bool {
        self.inner.state.lock().unwrap().closed
    }

    /// 要求を送り応答を待つ。再送はしない。timeout時は `OutcomeUnknown`（相手が実行した可能性あり）。
    pub async fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, BackendError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.inner.state.lock().unwrap();
            if st.closed {
                return Err(BackendError::NotConnected);
            }
            st.pending.insert(id, tx);
        }
        let msg = json!({ "id": id, "method": method, "params": params });
        if let Err(e) = self.write_line(&msg).await {
            self.inner.state.lock().unwrap().pending.remove(&id);
            return Err(match e {
                BackendError::NotConnected => BackendError::NotConnected,
                other => BackendError::OutcomeUnknown { message: format!("write failed: {other}") },
            });
        }
        let wait = async {
            rx.await.unwrap_or_else(|_| Err(BackendError::OutcomeUnknown { message: "response channel dropped".into() }))
        };
        match timeout {
            None => wait.await,
            Some(d) => match tokio::time::timeout(d, wait).await {
                Ok(r) => r,
                Err(_) => {
                    self.inner.state.lock().unwrap().pending.remove(&id);
                    Err(BackendError::OutcomeUnknown { message: format!("timeout after {d:?} waiting for {method}") })
                }
            },
        }
    }

    /// 応答を要求しない通知（`initialized`等）。
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), BackendError> {
        if self.is_closed() {
            return Err(BackendError::NotConnected);
        }
        let mut msg = json!({ "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.write_line(&msg).await
    }

    /// サーバー要求へ成功応答する。同じ要求への二重応答・未知IDは `Protocol` で拒否し送信しない。
    pub async fn respond(&self, request: &RequestKey, result: Value) -> Result<(), BackendError> {
        self.respond_with(request, "result", result).await
    }

    /// サーバー要求へエラー応答する。
    pub async fn respond_error(&self, request: &RequestKey, code: i64, message: &str) -> Result<(), BackendError> {
        self.respond_with(request, "error", json!({ "code": code, "message": message })).await
    }

    async fn respond_with(&self, request: &RequestKey, field: &str, body: Value) -> Result<(), BackendError> {
        if request.source != self.inner.source {
            return Err(BackendError::Protocol { message: "request belongs to another source".into() });
        }
        let rpc_id = RpcId::decode(&request.request_id)
            .ok_or_else(|| BackendError::Protocol { message: format!("bad request id {}", request.request_id.0) })?;
        {
            let mut st = self.inner.state.lock().unwrap();
            if st.closed {
                return Err(BackendError::NotConnected);
            }
            if !st.server_pending.remove(&request.request_id) {
                return Err(BackendError::Protocol { message: "unknown or already answered server request".into() });
            }
        }
        let mut msg = json!({ "id": rpc_id.to_value() });
        msg[field] = body;
        self.write_line(&msg).await.map_err(|e| match e {
            BackendError::NotConnected => BackendError::NotConnected,
            other => BackendError::OutcomeUnknown { message: format!("response write failed: {other}") },
        })
    }

    /// 書込み側を閉じる（stdinのEOF）。相手の終了は読取り側のEOFで検知する。
    pub async fn close_writer(&self) {
        let mut w = self.inner.writer.lock().await;
        if let Some(mut writer) = w.take() {
            let _ = writer.shutdown().await;
        }
    }

    /// 書込み（ロック待ちを含む）にtimeoutを付ける。超過は結果不明で、部分書込みの可能性があるため接続を閉じる。
    async fn write_line(&self, msg: &Value) -> Result<(), BackendError> {
        let mut line = serde_json::to_string(msg).map_err(|e| BackendError::Protocol { message: e.to_string() })?;
        line.push('\n');
        let write = async {
            let mut w = self.inner.writer.lock().await;
            let writer = w.as_mut().ok_or(BackendError::NotConnected)?;
            writer.write_all(line.as_bytes()).await.map_err(|e| BackendError::Io { message: e.to_string() })?;
            writer.flush().await.map_err(|e| BackendError::Io { message: e.to_string() })
        };
        match tokio::time::timeout(self.inner.write_timeout, write).await {
            Ok(r) => r,
            Err(_) => {
                self.abort("write timed out");
                Err(BackendError::OutcomeUnknown { message: format!("write timed out after {:?}; connection closed", self.inner.write_timeout) })
            }
        }
    }

    /// 接続を打ち切る: 保留中の要求を結果不明で完了、書込み側を破棄、読取りtaskを終了させる。
    fn abort(&self, reason: &str) {
        close_all(&self.inner, reason);
        self.inner.abort.notify_one();
        if let Ok(mut w) = self.inner.writer.try_lock() {
            w.take();
        }
    }
}

/// 内部の待ち行列 → 受け口。受け口が詰まっても読取りtaskは止まらない。
async fn forward(mut urx: mpsc::UnboundedReceiver<RpcEvent>, tx: mpsc::Sender<RpcEvent>, backlog: Arc<AtomicUsize>) {
    while let Some(ev) = urx.recv().await {
        backlog.fetch_sub(1, Ordering::SeqCst);
        if tx.send(ev).await.is_err() {
            break;
        }
    }
}

fn enqueue(utx: &mpsc::UnboundedSender<RpcEvent>, backlog: &AtomicUsize, ev: RpcEvent) -> bool {
    backlog.fetch_add(1, Ordering::SeqCst);
    utx.send(ev).is_ok()
}

async fn read_loop<R: AsyncRead + Unpin>(reader: R, inner: Arc<Inner>, utx: mpsc::UnboundedSender<RpcEvent>, backlog: Arc<AtomicUsize>, limit: usize) {
    let mut lines = BufReader::new(reader).lines();
    let mut seq: u64 = 0;
    let mut overflow_open = false;
    let reason = loop {
        let line = tokio::select! {
            r = lines.next_line() => match r {
                Ok(Some(l)) => l,
                Ok(None) => break "stdout closed (EOF)".to_string(),
                Err(e) => break format!("read error: {e}"),
            },
            _ = inner.abort.notified() => break "connection aborted (write failure)".to_string(),
        };
        if line.trim().is_empty() {
            continue;
        }
        let ev = match serde_json::from_str::<Value>(&line) {
            Err(e) => {
                seq += 1;
                Some(RpcEvent::Malformed { seq, note: format!("invalid JSON: {e}") })
            }
            Ok(v) => dispatch(&inner, v, &mut seq),
        };
        let Some(ev) = ev else { continue };
        let queued = backlog.load(Ordering::SeqCst);
        if queued < limit / 2 {
            overflow_open = false;
        }
        // 落としてよいのは通知・不正行・遅延応答だけ。サーバー要求は回答待ちなので必ず渡す。
        let droppable = matches!(ev, RpcEvent::Notification { .. } | RpcEvent::Malformed { .. } | RpcEvent::UnmatchedResponse { .. });
        if droppable && queued >= limit {
            if !overflow_open {
                overflow_open = true;
                seq += 1;
                if !enqueue(&utx, &backlog, RpcEvent::Overflow { seq }) {
                    break "event receiver dropped".to_string();
                }
            }
            continue;
        }
        if !enqueue(&utx, &backlog, ev) {
            break "event receiver dropped".to_string();
        }
    };
    close_all(&inner, &reason);
    if let Some(mut w) = inner.writer.lock().await.take() {
        let _ = w.shutdown().await;
    }
    let _ = enqueue(&utx, &backlog, RpcEvent::Disconnected { reason });
}

/// 切断: 保留中の要求をすべて「結果不明」で完了させる（成功・Rejected扱いにしない）。
fn close_all(inner: &Inner, reason: &str) {
    let drained: Vec<_> = {
        let mut st = inner.state.lock().unwrap();
        st.closed = true;
        st.server_pending.clear();
        st.pending.drain().collect()
    };
    for (_, tx) in drained {
        let _ = tx.send(Err(BackendError::OutcomeUnknown { message: format!("connection lost: {reason}") }));
    }
}

fn dispatch(inner: &Inner, v: Value, seq: &mut u64) -> Option<RpcEvent> {
    let Some(obj) = v.as_object() else {
        *seq += 1;
        return Some(RpcEvent::Malformed { seq: *seq, note: "message is not an object".into() });
    };
    let method = obj.get("method").and_then(Value::as_str).map(str::to_string);
    let id = obj.get("id").filter(|i| !i.is_null());
    match (method, id) {
        (Some(method), Some(id)) => {
            *seq += 1;
            let Some(rpc_id) = RpcId::from_value(id) else {
                return Some(RpcEvent::Malformed { seq: *seq, note: format!("server request {method} has invalid id") });
            };
            let ext = rpc_id.encode();
            inner.state.lock().unwrap().server_pending.insert(ext.clone());
            Some(RpcEvent::ServerRequest {
                seq: *seq,
                key: RequestKey { backend: BackendKind::Codex, source: inner.source.clone(), request_id: ext },
                method,
                params: obj.get("params").cloned().unwrap_or(Value::Null),
            })
        }
        (Some(method), None) => {
            *seq += 1;
            Some(RpcEvent::Notification { seq: *seq, method, params: obj.get("params").cloned().unwrap_or(Value::Null) })
        }
        (None, Some(id)) => {
            let result = if let Some(err) = obj.get("error") {
                Err(BackendError::Rejected {
                    code: err.get("code").and_then(Value::as_i64),
                    message: err.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                })
            } else if let Some(r) = obj.get("result") {
                Ok(r.clone())
            } else {
                Err(BackendError::Protocol { message: "response has neither result nor error".into() })
            };
            let tx = id.as_i64().and_then(|n| inner.state.lock().unwrap().pending.remove(&n));
            match tx {
                Some(tx) => {
                    let _ = tx.send(result);
                    None
                }
                None => {
                    *seq += 1;
                    Some(RpcEvent::UnmatchedResponse { seq: *seq, id: id.clone() })
                }
            }
        }
        (None, None) => {
            *seq += 1;
            Some(RpcEvent::Malformed { seq: *seq, note: "message has neither method nor id".into() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, DuplexStream};

    /// クライアントと、サーバー役の(読取り, 書込み)端を返す。
    fn setup() -> (RpcClient, mpsc::Receiver<RpcEvent>, BufReader<DuplexStream>, DuplexStream) {
        let (c_out, s_in) = duplex(65536);
        let (s_out, c_in) = duplex(65536);
        let (client, rx) = RpcClient::start(c_in, c_out, SourceId("src1".into()), 16);
        (client, rx, BufReader::new(s_in), s_out)
    }

    async fn read_json(r: &mut BufReader<DuplexStream>) -> Value {
        let mut l = String::new();
        r.read_line(&mut l).await.unwrap();
        serde_json::from_str(&l).unwrap()
    }

    async fn send_json(w: &mut DuplexStream, v: Value) {
        w.write_all(format!("{v}\n").as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn correlates_out_of_order_responses() {
        let (client, _rx, mut sr, mut sw) = setup();
        let (c1, c2) = (client.clone(), client.clone());
        let a = tokio::spawn(async move { c1.request("a", json!({}), None).await });
        let m1 = read_json(&mut sr).await;
        let b = tokio::spawn(async move { c2.request("b", json!({}), None).await });
        let m2 = read_json(&mut sr).await;
        assert_eq!(m1["method"], "a");
        assert_eq!(m2["method"], "b");
        send_json(&mut sw, json!({"id": m2["id"], "result": {"v": 2}})).await;
        send_json(&mut sw, json!({"id": m1["id"], "result": {"v": 1}})).await;
        assert_eq!(a.await.unwrap().unwrap(), json!({"v": 1}));
        assert_eq!(b.await.unwrap().unwrap(), json!({"v": 2}));
    }

    #[tokio::test]
    async fn error_response_is_rejected_not_unknown() {
        let (client, _rx, mut sr, mut sw) = setup();
        let c = client.clone();
        let t = tokio::spawn(async move { c.request("x", json!({}), None).await });
        let m = read_json(&mut sr).await;
        send_json(&mut sw, json!({"id": m["id"], "error": {"code": -32601, "message": "nope"}})).await;
        match t.await.unwrap() {
            Err(BackendError::Rejected { code, message }) => {
                assert_eq!(code, Some(-32601));
                assert_eq!(message, "nope");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn notifications_in_order_and_malformed_lines_surface() {
        let (_client, mut rx, _sr, mut sw) = setup();
        send_json(&mut sw, json!({"method": "n1", "params": {"a": 1}})).await;
        sw.write_all(b"not json\n").await.unwrap();
        send_json(&mut sw, json!({"method": "n2"})).await;
        assert_eq!(rx.recv().await.unwrap(), RpcEvent::Notification { seq: 1, method: "n1".into(), params: json!({"a": 1}) });
        assert!(matches!(rx.recv().await.unwrap(), RpcEvent::Malformed { seq: 2, .. }));
        assert_eq!(rx.recv().await.unwrap(), RpcEvent::Notification { seq: 3, method: "n2".into(), params: Value::Null });
    }

    #[tokio::test]
    async fn server_request_roundtrip_preserves_id_type_and_blocks_double_answer() {
        let (client, mut rx, mut sr, mut sw) = setup();
        send_json(&mut sw, json!({"id": 7, "method": "item/commandExecution/requestApproval", "params": {"k": 1}})).await;
        send_json(&mut sw, json!({"id": "7", "method": "tool/requestUserInput", "params": {}})).await;
        let (k1, k2) = match (rx.recv().await.unwrap(), rx.recv().await.unwrap()) {
            (RpcEvent::ServerRequest { key: a, .. }, RpcEvent::ServerRequest { key: b, .. }) => (a, b),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(k1.source, SourceId("src1".into()));
        assert_ne!(k1.request_id, k2.request_id);
        client.respond(&k1, json!({"decision": "accept"})).await.unwrap();
        client.respond_error(&k2, -1, "no").await.unwrap();
        assert_eq!(read_json(&mut sr).await, json!({"id": 7, "result": {"decision": "accept"}}));
        assert_eq!(read_json(&mut sr).await, json!({"id": "7", "error": {"code": -1, "message": "no"}}));
        assert!(matches!(client.respond(&k1, json!({})).await, Err(BackendError::Protocol { .. })));
        let mut other = k2.clone();
        other.source = SourceId("src2".into());
        assert!(matches!(client.respond(&other, json!({})).await, Err(BackendError::Protocol { .. })));
    }

    #[tokio::test]
    async fn disconnect_resolves_pending_as_outcome_unknown() {
        let (client, mut rx, mut sr, sw) = setup();
        let c = client.clone();
        let t = tokio::spawn(async move { c.request("slow", json!({}), None).await });
        let _ = read_json(&mut sr).await;
        drop(sw);
        assert!(matches!(t.await.unwrap(), Err(BackendError::OutcomeUnknown { .. })));
        assert!(matches!(rx.recv().await.unwrap(), RpcEvent::Disconnected { .. }));
        assert!(client.is_closed());
        assert!(matches!(client.request("x", json!({}), None).await, Err(BackendError::NotConnected)));
    }

    #[tokio::test]
    async fn timeout_is_outcome_unknown_not_resent_and_late_response_is_unmatched() {
        let (client, mut rx, mut sr, mut sw) = setup();
        let r = client.request("slow", json!({}), Some(Duration::from_millis(50))).await;
        assert!(matches!(r, Err(BackendError::OutcomeUnknown { .. })));
        let m = read_json(&mut sr).await;
        let mut extra = String::new();
        assert!(tokio::time::timeout(Duration::from_millis(50), sr.read_line(&mut extra)).await.is_err());
        send_json(&mut sw, json!({"id": m["id"], "result": {}})).await;
        assert!(matches!(rx.recv().await.unwrap(), RpcEvent::UnmatchedResponse { .. }));
    }

    #[tokio::test]
    async fn slow_event_consumer_does_not_stall_response_correlation() {
        // 受け口(容量1)を誰も読まない状態で通知を溜めても、応答は照合される。
        let (c_out, s_in) = duplex(65536);
        let (mut s_out, c_in) = duplex(65536);
        let (client, rx) = RpcClient::start_with(c_in, c_out, SourceId("s".into()), 1, 5, Duration::from_secs(5));
        let mut sr = BufReader::new(s_in);
        for i in 0..50 {
            send_json(&mut s_out, json!({"method": "n", "params": {"i": i}})).await;
        }
        let c = client.clone();
        let t = tokio::spawn(async move { c.request("q", json!({}), Some(Duration::from_secs(5))).await });
        let m = read_json(&mut sr).await;
        send_json(&mut s_out, json!({"id": m["id"], "result": {"ok": true}})).await;
        assert_eq!(t.await.unwrap().unwrap(), json!({"ok": true}));
        // 溢れた通知は黙って捨てず Overflow が届く。サーバー要求は落とさない。
        send_json(&mut s_out, json!({"id": 9, "method": "item/tool/requestUserInput", "params": {}})).await;
        let mut rx = rx;
        let (mut overflow, mut got_request) = (false, false);
        while !(overflow && got_request) {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap() {
                RpcEvent::Overflow { .. } => overflow = true,
                RpcEvent::ServerRequest { .. } => got_request = true,
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn stuck_write_times_out_as_outcome_unknown_and_closes_connection() {
        // 読み手のいない極小バッファへ大きな書込み → 詰まる。
        let (c_out, _s_in) = duplex(8);
        let (_s_out, c_in) = duplex(64);
        let (client, mut rx) = RpcClient::start_with(c_in, c_out, SourceId("s".into()), 4, 100, Duration::from_millis(100));
        let r = client.request("x", json!({"big": "y".repeat(256)}), None).await;
        assert!(matches!(r, Err(BackendError::OutcomeUnknown { .. })), "{r:?}");
        assert!(client.is_closed());
        assert!(matches!(client.request("again", json!({}), None).await, Err(BackendError::NotConnected)));
        assert!(matches!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap(), RpcEvent::Disconnected { .. }));
    }

    #[tokio::test]
    async fn pending_server_requests_are_cleared_on_disconnect() {
        let (client, mut rx, _sr, mut sw) = setup();
        send_json(&mut sw, json!({"id": 1, "method": "m", "params": {}})).await;
        let key = match rx.recv().await.unwrap() {
            RpcEvent::ServerRequest { key, .. } => key,
            o => panic!("{o:?}"),
        };
        drop(sw);
        assert!(matches!(rx.recv().await.unwrap(), RpcEvent::Disconnected { .. }));
        assert!(matches!(client.respond(&key, json!({})).await, Err(BackendError::NotConnected)));
    }
}

//! OS通知の配信（P5、DESIGN_P2 §4）。判断は純粋ロジック `rules::notify`、ここは時刻・タイマー・設定・フォーカスをつなぐ。
//!
//! - 入力は `HostData.notify_inbox`（終端遷移・要求到着の検出点が積む）。`Host::mutate` の直後に `Host::notify_submit` へ渡る。
//! - 承認・質問は即時、終端は2秒の固定窓の満了で配信する。タイマーは1本だけ（`next_deadline`）。
//! - 実際のOS表示は `OsSink`（`lib.rs` が `win::toast` を渡す）。未設定（テスト・開発の一部）なら何も表示しない。
//! - 通知を開く操作は該当チャットの表示だけ（`navigate_to_chat`）。回答・再実行・キュー再開はしない。

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use super::{now_ms, Host};
use crate::backend::ipc::*;
use crate::backend::local::AcknowledgeFailureArgs;
use crate::backend::model::*;
use crate::rules::notify::{render, Aggregator, FocusContext, NotifyInput, OsNotification, Pending};

pub type OsSink = Arc<dyn Fn(OsNotification) + Send + Sync>;

#[derive(Default)]
struct Inner {
    agg: Aggregator,
    main_focused: bool,
    selected: Option<ChatKey>,
    timer_armed: bool,
}

#[derive(Default)]
pub struct Notifier {
    inner: Mutex<Inner>,
    sink: OnceLock<OsSink>,
    host: OnceLock<Weak<Host>>,
}

impl Host {
    /// OS通知の出力先を設定する（起動時に1回）。
    pub fn attach_notifier(self: &Arc<Self>, sink: OsSink) {
        let _ = self.notifier.host.set(Arc::downgrade(self));
        let _ = self.notifier.sink.set(sink);
    }

    /// 通常画面のフォーカス状態（窓のFocusedイベント）。
    pub fn set_main_focused(&self, focused: bool) {
        self.notifier.inner.lock().unwrap().main_focused = focused;
    }

    /// 選択中のチャット（None＝なし）。抑制の判定に使うだけで、状態は変えない。
    pub fn set_selected_chat(&self, chat: Option<ChatKey>) {
        self.notifier.inner.lock().unwrap().selected = chat;
    }

    /// 通知（クリック）から該当チャットを開く。表示するだけで、回答・再実行はしない。
    pub fn navigate_to_chat(&self, chat: ChatKey) {
        self.mutate(|_| ((), vec![HostEvent::NavigateToChat { chat }]));
    }

    /// 「確認済み」にする。印を外すだけで、再実行・成功化・キュー再開はしない（M42）。
    pub fn acknowledge_failure(self: &Arc<Self>, args: AcknowledgeFailureArgs) -> Result<(), IpcError> {
        let targets = self.read(|d| d.unacknowledged_failures(&args.chat, args.agent.as_ref()));
        if targets.is_empty() {
            return Ok(());
        }
        self.precheck_space()?;
        self.update_local(&args.chat, true, Duration::ZERO, |f| {
            for t in targets {
                if !f.acknowledged_failures.contains(&t) {
                    f.acknowledged_failures.push(t);
                }
            }
        });
        Ok(())
    }

    /// `mutate` の直後に呼ばれる。印を出すための補足情報を用意し、入力を集約器へ渡す。
    pub(super) fn notify_submit(&self, inputs: Vec<NotifyInput>) {
        let Some(host) = self.notifier.host.get().and_then(Weak::upgrade) else { return };
        // 一覧の印は補足情報（ChatLocalView）で運ぶ。まだ無いチャットは、印が付く時点で作る（保存は通常の経路）。
        for input in &inputs {
            let chat = match input {
                NotifyInput::AwaitingAnswer { chat, .. } => chat,
                NotifyInput::Ended { chat, end: TurnEnd::Failed, .. } => chat,
                _ => continue,
            };
            if host.read(|d| !d.locals.contains_key(chat)) {
                host.update_local(chat, true, Duration::ZERO, |_| {});
            }
        }
        let now = now_ms();
        let immediate: Vec<Pending> = {
            let mut inner = self.notifier.inner.lock().unwrap();
            inputs.into_iter().flat_map(|i| inner.agg.push(i, now)).collect()
        };
        for p in immediate {
            host.deliver(&p);
        }
        host.arm_timer();
    }

    /// 窓の満了を待つタイマー（1本だけ）。ランタイム内から呼ぶ。
    fn arm_timer(self: &Arc<Self>) {
        let mut deadline = {
            let mut inner = self.notifier.inner.lock().unwrap();
            if inner.timer_armed {
                return;
            }
            let Some(d) = inner.agg.next_deadline() else { return };
            inner.timer_armed = true;
            d
        };
        // ランタイム外から呼ばれた場合は張らない（次の入力で再度試す）。
        if tokio::runtime::Handle::try_current().is_err() {
            self.notifier.inner.lock().unwrap().timer_armed = false;
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                let wait = (deadline.0 - now_ms().0).max(1) as u64;
                tokio::time::sleep(Duration::from_millis(wait)).await;
                let (due, next) = {
                    let mut inner = host.notifier.inner.lock().unwrap();
                    let due = inner.agg.due(now_ms());
                    let next = inner.agg.next_deadline();
                    if next.is_none() {
                        inner.timer_armed = false;
                    }
                    (due, next)
                };
                for p in due {
                    host.deliver(&p);
                }
                match next {
                    Some(d) => deadline = d,
                    None => break,
                }
            }
        });
    }

    /// 設定・抑制を当てて、出すものだけOSへ渡す。
    fn deliver(&self, p: &Pending) {
        let Some(sink) = self.notifier.sink.get() else { return };
        let (settings, name) = self.read(|d| {
            let chat = match p {
                Pending::AwaitingAnswer { chat, .. } | Pending::Ended { chat, .. } => chat,
            };
            (d.settings.notifications.clone(), d.chat(chat).and_then(|c| c.name.value().cloned()))
        });
        let focus = {
            let inner = self.notifier.inner.lock().unwrap();
            FocusContext { main_window_focused: inner.main_focused, selected_chat: inner.selected.clone() }
        };
        if let Some(n) = render(p, &settings, &focus, name.as_deref()) {
            sink(n);
        }
    }
}

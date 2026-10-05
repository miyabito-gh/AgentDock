//! OS通知の判断（P5、要件§3.11・D03）。純粋ロジック。時刻は引数で注入する。
//!
//! - 承認・質問待ちは即時（集約しない）。
//! - 親・子孫の終端（完了・失敗・中断）は、同一チャット内で最初のイベントから [`AGGREGATE_WINDOW_MS`] の固定窓で集約する
//!   （窓は延長しない）。失敗を完了で隠さない（件数を種類別に出す）。
//! - 通知の起点は「このセッションで非終端を観測していたエージェントが終端になった」ときだけ。
//!   初観測のエージェント・起動時の読込み・再接続後の履歴補完では通知しない（起動時の一斉通知を防ぐ）。
//! - 同じ (agent, turn, 終端) と同じ要求キーは一度だけ（重複通知を防ぐ）。
//! - 抑制: 通常画面がフォーカスを持ち、かつ選択中のチャットのときだけOS通知を出さない。監視窓だけ・背後なら出す。
//! - 本文はチャット名と状態だけ。会話本文・コード・ファイル内容・ツール引数・エラー詳細を入れない。

use std::collections::{HashMap, HashSet, VecDeque};

use crate::backend::local::NotificationSettings;
use crate::backend::model::*;

pub const AGGREGATE_WINDOW_MS: i64 = 2_000;
/// 重複判定に覚えておく件数（古いものから捨てる）。
pub const DEDUP_CAPACITY: usize = 4_096;

#[derive(Debug, Clone, PartialEq)]
pub enum NotifyInput {
    /// 承認待ち・質問待ちの到着（`RequestOpened`）。
    AwaitingAnswer { chat: ChatKey, request: RequestKey, kind: RequestKind },
    /// 終端の観測。`was_nonterminal` は直前にこのセッションで非終端（running/waiting/initializing）を観測していたか。
    Ended { chat: ChatKey, agent: AgentKey, is_root: bool, turn: ExternalId, end: TurnEnd, was_nonterminal: bool },
}

/// 集約後の出力（まだ設定・抑制を当てていない）。
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    AwaitingAnswer { chat: ChatKey, kind: RequestKind },
    Ended { chat: ChatKey, completed: u32, failed: u32, interrupted: u32, root_included: bool },
}

#[derive(Debug, Default)]
#[allow(dead_code)] // P5で使う
struct Window {
    opened_at: i64,
    completed: u32,
    failed: u32,
    interrupted: u32,
    root_included: bool,
}

#[derive(Debug, Default)]
pub struct Aggregator {
    windows: HashMap<ChatKey, Window>,
    seen_keys: HashSet<String>,
    seen_order: VecDeque<String>,
}

impl Aggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入力を受ける。即時に出すもの（承認・質問）を返す。終端は窓に入れて `due` で出す。
    pub fn push(&mut self, input: NotifyInput, now: UnixMillis) -> Vec<Pending> {
        let _ = (input, now, &self.windows, &self.seen_keys, &self.seen_order);
        todo!("P5")
    }

    /// 窓が閉じた（`opened_at + 2000 <= now`）チャットの集約結果を返して窓を消す。
    pub fn due(&mut self, now: UnixMillis) -> Vec<Pending> {
        let _ = now;
        todo!("P5")
    }

    /// 次に `due` を呼ぶべき時刻（タイマーの設定用）。
    pub fn next_deadline(&self) -> Option<UnixMillis> {
        todo!("P5")
    }
}

/// 抑制判定の材料（配信時点の値）。
#[derive(Debug, Clone, PartialEq)]
pub struct FocusContext {
    pub main_window_focused: bool,
    pub selected_chat: Option<ChatKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    AwaitingAnswer,
    Completed,
    Failed,
}

/// OSへ渡す内容。
#[derive(Debug, Clone, PartialEq)]
pub struct OsNotification {
    pub chat: ChatKey,
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub sound: bool,
}

/// 設定・抑制を当てて通知文を作る。出さないなら None。
/// - 失敗が1件以上あれば種類は Failed（題名に「失敗」を含める）。失敗通知がオフでも、完了通知の本文で「完了」とだけ言わない。
/// - 中断は失敗とは別に件数を出し、種類の判定では Failed 側の設定に従う（正常完了ではないため）。
/// - `show_chat_name=false` ならチャット名を出さない。
pub fn render(p: &Pending, settings: &NotificationSettings, focus: &FocusContext, chat_name: Option<&str>) -> Option<OsNotification> {
    let _ = (p, settings, focus, chat_name);
    todo!("P5")
}

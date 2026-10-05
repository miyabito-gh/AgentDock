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
/// 通知本文に出すチャット名の上限文字数。
const NAME_MAX_CHARS: usize = 60;

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

    /// 初めて見るキーなら覚えて true。覚えた件数が上限を超えたら古いものから捨てる。
    fn first_time(&mut self, key: String) -> bool {
        if !self.seen_keys.insert(key.clone()) {
            return false;
        }
        self.seen_order.push_back(key);
        while self.seen_order.len() > DEDUP_CAPACITY {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen_keys.remove(&old);
            }
        }
        true
    }

    /// 入力を受ける。即時に出すもの（承認・質問）を返す。終端は窓に入れて `due` で出す。
    pub fn push(&mut self, input: NotifyInput, now: UnixMillis) -> Vec<Pending> {
        match input {
            NotifyInput::AwaitingAnswer { chat, request, kind } => {
                let key = format!("req|{}|{}", request.source.0, request.request_id.0);
                if self.first_time(key) {
                    vec![Pending::AwaitingAnswer { chat, kind }]
                } else {
                    Vec::new()
                }
            }
            NotifyInput::Ended { chat, agent, is_root, turn, end, was_nonterminal } => {
                if !was_nonterminal {
                    return Vec::new();
                }
                if !self.first_time(format!("end|{}|{}|{:?}", agent.id.0, turn.0, end)) {
                    return Vec::new();
                }
                // 窓は最初のイベントで開き、以後は延長しない（開いている間は件数を足すだけ）。
                let w = self.windows.entry(chat).or_insert_with(|| Window { opened_at: now.0, ..Window::default() });
                match end {
                    TurnEnd::Completed => w.completed += 1,
                    TurnEnd::Failed => w.failed += 1,
                    TurnEnd::Interrupted => w.interrupted += 1,
                }
                w.root_included |= is_root;
                Vec::new()
            }
        }
    }

    /// 窓が閉じた（`opened_at + 2000 <= now`）チャットの集約結果を返して窓を消す。
    pub fn due(&mut self, now: UnixMillis) -> Vec<Pending> {
        let mut ready: Vec<ChatKey> = self.windows.iter().filter(|(_, w)| w.opened_at + AGGREGATE_WINDOW_MS <= now.0).map(|(c, _)| c.clone()).collect();
        ready.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        ready
            .into_iter()
            .filter_map(|chat| {
                let w = self.windows.remove(&chat)?;
                Some(Pending::Ended { chat, completed: w.completed, failed: w.failed, interrupted: w.interrupted, root_included: w.root_included })
            })
            .collect()
    }

    /// 次に `due` を呼ぶべき時刻（タイマーの設定用）。
    pub fn next_deadline(&self) -> Option<UnixMillis> {
        self.windows.values().map(|w| w.opened_at + AGGREGATE_WINDOW_MS).min().map(UnixMillis)
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

fn chat_prefix(show: bool, chat_name: Option<&str>) -> String {
    match (show, chat_name.map(str::trim).filter(|n| !n.is_empty())) {
        (true, Some(n)) => {
            let mut s: String = n.chars().take(NAME_MAX_CHARS).collect();
            if n.chars().count() > NAME_MAX_CHARS {
                s.push('…');
            }
            format!("{s}: ")
        }
        _ => String::new(),
    }
}

/// 設定・抑制を当てて通知文を作る。出さないなら None。
/// - 失敗が1件以上あれば種類は Failed（題名に「失敗」を含める）。失敗通知がオフでも、完了通知の本文で「完了」とだけ言わない。
/// - 中断は失敗とは別に件数を出し、種類の判定では Failed 側の設定に従う（正常完了ではないため）。
/// - `show_chat_name=false` ならチャット名を出さない。
pub fn render(p: &Pending, settings: &NotificationSettings, focus: &FocusContext, chat_name: Option<&str>) -> Option<OsNotification> {
    let (chat, kind, title, detail) = match p {
        Pending::AwaitingAnswer { chat, kind } => {
            let (title, detail) = match kind {
                RequestKind::UserInput | RequestKind::ToolElicitation => ("質問待ち", "回答を待っています"),
                RequestKind::Other { .. } => ("操作待ち", "対応を待っています"),
                _ => ("承認待ち", "承認を待っています"),
            };
            (chat, NotificationKind::AwaitingAnswer, title, detail.to_string())
        }
        Pending::Ended { chat, completed, failed, interrupted, .. } => {
            if completed + failed + interrupted == 0 {
                return None;
            }
            let mut parts = Vec::new();
            if *failed > 0 {
                parts.push(format!("失敗 {failed}件"));
            }
            if *interrupted > 0 {
                parts.push(format!("中断 {interrupted}件"));
            }
            if *completed > 0 {
                parts.push(format!("完了 {completed}件"));
            }
            let detail = parts.join("・");
            if *failed > 0 || *interrupted > 0 {
                (chat, NotificationKind::Failed, if *failed > 0 { "失敗あり" } else { "中断あり" }, detail)
            } else {
                (chat, NotificationKind::Completed, "完了", detail)
            }
        }
    };
    if !settings.enabled {
        return None;
    }
    let allowed = match kind {
        NotificationKind::AwaitingAnswer => settings.approval_and_question,
        NotificationKind::Completed => settings.completed,
        NotificationKind::Failed => settings.failed,
    };
    if !allowed {
        return None;
    }
    // 通常画面が前面で、そのチャットを見ている間だけ抑える。別チャット・監視窓だけ・背後なら出す。
    if focus.main_window_focused && focus.selected_chat.as_ref() == Some(chat) {
        return None;
    }
    Some(OsNotification {
        chat: chat.clone(),
        kind,
        title: format!("AgentDock: {title}"),
        body: format!("{}{}", chat_prefix(settings.show_chat_name, chat_name), detail),
        sound: settings.sound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn agent(s: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn ended(c: &str, a: &str, t: &str, end: TurnEnd, was: bool) -> NotifyInput {
        NotifyInput::Ended { chat: chat(c), agent: agent(a), is_root: a == c, turn: ExternalId(t.into()), end, was_nonterminal: was }
    }
    fn req(id: &str) -> NotifyInput {
        NotifyInput::AwaitingAnswer {
            chat: chat("c1"),
            request: RequestKey { backend: BackendKind::Codex, source: SourceId("s".into()), request_id: ExternalId(id.into()) },
            kind: RequestKind::CommandApproval,
        }
    }
    fn at(ms: i64) -> UnixMillis {
        UnixMillis(ms)
    }
    fn unfocused() -> FocusContext {
        FocusContext { main_window_focused: false, selected_chat: None }
    }

    #[test]
    fn approval_and_question_are_immediate_and_not_duplicated() {
        let mut a = Aggregator::new();
        assert_eq!(a.push(req("r1"), at(0)).len(), 1);
        assert!(a.push(req("r1"), at(10)).is_empty());
        assert_eq!(a.push(req("r2"), at(20)).len(), 1);
        assert_eq!(a.next_deadline(), None);
    }

    #[test]
    fn first_observation_does_not_notify() {
        let mut a = Aggregator::new();
        assert!(a.push(ended("c1", "c1", "t1", TurnEnd::Completed, false), at(0)).is_empty());
        assert_eq!(a.next_deadline(), None);
        assert!(a.due(at(10_000)).is_empty());
    }

    #[test]
    fn fixed_window_is_not_extended() {
        let mut a = Aggregator::new();
        a.push(ended("c1", "k1", "t1", TurnEnd::Completed, true), at(1_000));
        a.push(ended("c1", "k2", "t1", TurnEnd::Completed, true), at(2_900));
        assert_eq!(a.next_deadline(), Some(at(3_000)));
        assert!(a.due(at(2_999)).is_empty());
        let out = a.due(at(3_000));
        assert_eq!(out, vec![Pending::Ended { chat: chat("c1"), completed: 2, failed: 0, interrupted: 0, root_included: false }]);
        // 窓を閉じたあとのイベントは新しい窓。
        a.push(ended("c1", "k3", "t1", TurnEnd::Completed, true), at(3_500));
        assert_eq!(a.next_deadline(), Some(at(5_500)));
    }

    #[test]
    fn failure_is_not_hidden_by_completion_and_chats_are_separate() {
        let mut a = Aggregator::new();
        a.push(ended("c1", "c1", "t1", TurnEnd::Completed, true), at(0));
        a.push(ended("c1", "k1", "t1", TurnEnd::Failed, true), at(100));
        a.push(ended("c1", "k2", "t1", TurnEnd::Interrupted, true), at(200));
        a.push(ended("c2", "k9", "t1", TurnEnd::Completed, true), at(500));
        let out = a.due(at(2_000));
        assert_eq!(out, vec![Pending::Ended { chat: chat("c1"), completed: 1, failed: 1, interrupted: 1, root_included: true }]);
        let out = a.due(at(2_500));
        assert_eq!(out, vec![Pending::Ended { chat: chat("c2"), completed: 1, failed: 0, interrupted: 0, root_included: false }]);
    }

    #[test]
    fn same_agent_turn_end_is_counted_once() {
        let mut a = Aggregator::new();
        a.push(ended("c1", "k1", "t1", TurnEnd::Completed, true), at(0));
        a.push(ended("c1", "k1", "t1", TurnEnd::Completed, true), at(50));
        let out = a.due(at(2_000));
        assert_eq!(out, vec![Pending::Ended { chat: chat("c1"), completed: 1, failed: 0, interrupted: 0, root_included: false }]);
        // 別のturnは別の通知。
        a.push(ended("c1", "k1", "t2", TurnEnd::Completed, true), at(3_000));
        assert_eq!(a.due(at(5_000)).len(), 1);
    }

    #[test]
    fn dedup_memory_is_bounded() {
        let mut a = Aggregator::new();
        for i in 0..(DEDUP_CAPACITY + 10) {
            a.push(req(&format!("r{i}")), at(0));
        }
        assert!(a.seen_keys.len() <= DEDUP_CAPACITY);
        // 最も古いものは忘れている（上限内なので許容）。直近のものは重複として扱う。
        assert!(a.push(req(&format!("r{}", DEDUP_CAPACITY + 9)), at(0)).is_empty());
    }

    fn settings() -> NotificationSettings {
        NotificationSettings::default()
    }
    fn done(c: &str, completed: u32, failed: u32, interrupted: u32) -> Pending {
        Pending::Ended { chat: chat(c), completed, failed, interrupted, root_included: false }
    }

    #[test]
    fn focused_selected_chat_is_suppressed_but_others_are_not() {
        let p = done("c1", 1, 0, 0);
        let focus = FocusContext { main_window_focused: true, selected_chat: Some(chat("c1")) };
        assert!(render(&p, &settings(), &focus, Some("A")).is_none());
        assert!(render(&done("c2", 1, 0, 0), &settings(), &focus, Some("B")).is_some());
        // 背後（フォーカスなし）や監視窓だけのときは選択中でも出す。
        let back = FocusContext { main_window_focused: false, selected_chat: Some(chat("c1")) };
        assert!(render(&p, &settings(), &back, Some("A")).is_some());
        // 承認待ちも同じ規則。
        let ap = Pending::AwaitingAnswer { chat: chat("c1"), kind: RequestKind::CommandApproval };
        assert!(render(&ap, &settings(), &focus, Some("A")).is_none());
        assert!(render(&ap, &settings(), &back, Some("A")).is_some());
    }

    #[test]
    fn chat_name_can_be_hidden() {
        let p = done("c1", 2, 0, 0);
        let n = render(&p, &settings(), &unfocused(), Some("秘密の名前")).unwrap();
        assert!(n.body.contains("秘密の名前"));
        let mut s = settings();
        s.show_chat_name = false;
        let n = render(&p, &s, &unfocused(), Some("秘密の名前")).unwrap();
        assert!(!n.title.contains("秘密の名前") && !n.body.contains("秘密の名前"));
        assert_eq!(n.body, "完了 2件");
    }

    #[test]
    fn failure_wins_the_kind_and_text_only_has_counts() {
        let n = render(&done("c1", 3, 1, 1), &settings(), &unfocused(), Some("A")).unwrap();
        assert_eq!(n.kind, NotificationKind::Failed);
        assert!(n.title.contains("失敗"));
        assert_eq!(n.body, "A: 失敗 1件・中断 1件・完了 3件");
        let n = render(&done("c1", 0, 0, 2), &settings(), &unfocused(), None).unwrap();
        assert_eq!(n.kind, NotificationKind::Failed);
        assert!(!n.body.contains("完了"));
    }

    #[test]
    fn per_kind_and_global_switches() {
        let mut s = settings();
        s.failed = false;
        assert!(render(&done("c1", 3, 1, 0), &s, &unfocused(), None).is_none()); // 失敗を含むので、完了設定では出さない
        assert!(render(&done("c1", 3, 0, 0), &s, &unfocused(), None).is_some());
        let mut s = settings();
        s.completed = false;
        assert!(render(&done("c1", 3, 0, 0), &s, &unfocused(), None).is_none());
        assert!(render(&done("c1", 3, 1, 0), &s, &unfocused(), None).is_some());
        let mut s = settings();
        s.approval_and_question = false;
        let ap = Pending::AwaitingAnswer { chat: chat("c1"), kind: RequestKind::UserInput };
        assert!(render(&ap, &s, &unfocused(), None).is_none());
        let mut s = settings();
        s.enabled = false;
        assert!(render(&ap, &s, &unfocused(), None).is_none());
        assert!(render(&done("c1", 0, 1, 0), &s, &unfocused(), None).is_none());
        s.enabled = true;
        s.sound = false;
        assert!(!render(&ap, &s, &unfocused(), None).unwrap().sound);
    }

    #[test]
    fn body_never_carries_request_details() {
        let ap = Pending::AwaitingAnswer { chat: chat("c1"), kind: RequestKind::Other { raw: "rm -rf / secret".into() } };
        let n = render(&ap, &settings(), &unfocused(), Some("A")).unwrap();
        assert!(!n.body.contains("secret") && !n.title.contains("secret"));
    }
}

//! Markdownエクスポート（P7、M41）。純粋ロジック。
//!
//! - 会話本文（Codexの `read` で取得したturn）と、添付・成果物のファイル名を出す。ファイルの中身は含めない。
//! - 「監視活動履歴も含める」なら、保存済みの監視活動（子・孫を含む）を末尾に追加する。
//! - 取得できなかった部分は「未取得」と書き、空で代用しない。

use crate::backend::local::{ArtifactEntry, AttachmentEntry, AttachmentState};
use crate::backend::model::*;
use crate::store::records::ActivityLine;

pub struct ExportInput<'a> {
    pub chat_name: Known<String>,
    pub turns: &'a [TurnRecord],
    /// 本文が完全に取れたか（`TurnRecord.complete` の集約）。false なら冒頭に注記する。
    pub transcript_complete: bool,
    pub attachments: &'a [AttachmentEntry],
    pub artifacts: &'a [ArtifactEntry],
    /// None＝監視活動を含めない。
    pub monitor_activity: Option<&'a [ActivityLine]>,
    pub exported_at_local: String,
}

const NOT_FETCHED: &str = "（未取得）";

/// 本文中のバッククォートの最長連続より長いフェンスを選ぶ（コードを壊さない）。最短は3つ。
pub fn fence_for(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn fenced(text: &str) -> String {
    let f = fence_for(text);
    format!("{f}\n{text}\n{f}\n")
}

/// 取得できた文字列。取得できなかった理由は区別せず「未取得」と書く（空文字で代用しない）。
fn known_text(k: &Known<String>) -> String {
    match k {
        Known::Value { value, .. } => value.clone(),
        _ => NOT_FETCHED.to_string(),
    }
}

fn kind_label(k: &ActivityKind) -> String {
    match k {
        ActivityKind::UserMessage => "ユーザー".into(),
        ActivityKind::AgentMessage => "エージェント".into(),
        ActivityKind::Reasoning => "推論".into(),
        ActivityKind::Plan => "計画".into(),
        ActivityKind::Command => "コマンド".into(),
        ActivityKind::FileChange => "ファイル変更".into(),
        ActivityKind::ToolCall => "ツール呼び出し".into(),
        ActivityKind::WebSearch => "Web検索".into(),
        ActivityKind::SubAgent => "子エージェント".into(),
        ActivityKind::ReviewStarted => "レビュー開始".into(),
        ActivityKind::ReviewResult => "レビュー結果".into(),
        ActivityKind::ContextCompaction => "文脈の圧縮".into(),
        ActivityKind::Other { raw } => format!("その他（{raw}）"),
    }
}

fn end_label(end: Option<TurnEnd>) -> &'static str {
    match end {
        Some(TurnEnd::Completed) => "完了",
        Some(TurnEnd::Failed) => "失敗",
        Some(TurnEnd::Interrupted) => "中断",
        None => "終端を確認できていません",
    }
}

fn state_label(s: AgentState) -> &'static str {
    match s {
        AgentState::Initializing => "起動中",
        AgentState::Running => "実行中",
        AgentState::Waiting => "待機中",
        AgentState::Idle => "待機（アイドル）",
        AgentState::Done => "完了",
        AgentState::Failed => "失敗",
        AgentState::Interrupted => "中断",
        AgentState::Closed => "終了",
        AgentState::Unknown => "不明",
    }
}

fn freshness_label(f: Freshness) -> &'static str {
    match f {
        Freshness::Live => "live",
        Freshness::HistoryOnly => "履歴のみ",
        Freshness::NeedsReconcile => "要照合",
        Freshness::Disconnected => "切断",
        Freshness::Unsupported => "非対応",
    }
}

/// パス末尾のファイル名（`\` と `/` の両方を区切りとみなす）。中身・フォルダ構成は出さない。
fn file_name_of(path: &str) -> String {
    let t = path.trim_end_matches(['\\', '/']);
    t.rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).unwrap_or(t).to_string()
}

/// 1970-01-01 からの日数を（年, 月, 日）にする（UTC。Howard Hinnantのcivil_from_days）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD HH:MM:SS UTC`。監視活動の時刻表示用（ローカル時刻への変換は行わないので UTC と明記する）。
fn utc_text(at: UnixMillis) -> String {
    let secs = at.0.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn attachment_line(a: &AttachmentEntry) -> String {
    let note = match &a.state {
        AttachmentState::Ready => "",
        AttachmentState::Copying => "（コピー中）",
        AttachmentState::CopyFailed { .. } => "（コピーに失敗）",
        AttachmentState::Missing => "（コピーの実体を確認できません）",
    };
    format!("- {}{note}\n", a.display_name)
}

fn artifact_line(a: &ArtifactEntry) -> String {
    let note = match &a.exists {
        Known::Value { value: true, .. } => "",
        Known::Value { value: false, .. } => "（実在を確認できません）",
        _ => "（実在は未確認）",
    };
    format!("- {}{note}\n", file_name_of(&a.path))
}

fn activity_line(l: &ActivityLine) -> String {
    match l {
        ActivityLine::AgentSeen { at, agent } => {
            let kind = match agent.parent {
                ParentLink::Root => "親",
                ParentLink::Explicit { .. } => "子・孫",
                ParentLink::Unknown => "子・孫（親を確認できません）",
            };
            format!("- {} 発見: {}（{kind}、ID {}）", utc_text(*at), known_text(&agent.display_name), agent.key.id.0)
        }
        ActivityLine::StatusChanged { at, agent, state, raw, .. } => {
            format!("- {} 状態: {}（{}、原状態 {raw}）", utc_text(*at), agent.id.0, state_label(*state))
        }
        ActivityLine::TurnEnded { at, turn, end } => {
            format!("- {} turn終端: {}（{}）", utc_text(*at), turn.agent.id.0, end_label(Some(*end)))
        }
        ActivityLine::Activity { at, activity } => {
            format!("- {} {}: {}（エージェント {}）", utc_text(*at), kind_label(&activity.kind), known_text(&activity.summary), activity.key.agent.id.0)
        }
        ActivityLine::Freshness { at, agent, freshness } => {
            let who = agent.as_ref().map(|a| a.id.0.as_str()).unwrap_or("全体");
            format!("- {} 鮮度: {who}（{}）", utc_text(*at), freshness_label(*freshness))
        }
    }
}

pub fn render_markdown(input: &ExportInput<'_>) -> String {
    let mut out = String::new();
    let title = match &input.chat_name {
        Known::Value { value, .. } if !value.trim().is_empty() => value.trim().to_string(),
        _ => "（名前を確認できないチャット）".to_string(),
    };
    out.push_str(&format!("# {title}\n\n"));
    out.push_str(&format!("エクスポート日時: {}\n\n", input.exported_at_local));
    if !input.transcript_complete {
        out.push_str("> 注意: 会話本文の一部を取得できていません。取得できなかった部分は「未取得」と記しています。\n\n");
    }

    out.push_str("## 会話\n\n");
    if input.turns.is_empty() {
        out.push_str(&format!("{NOT_FETCHED}\n\n"));
    }
    for (i, turn) in input.turns.iter().enumerate() {
        out.push_str(&format!("### turn {}\n\n", i + 1));
        if !turn.complete {
            out.push_str("（このturnの本文は一部しか取得できていません）\n\n");
        }
        for e in &turn.entries {
            out.push_str(&format!("**{}**\n\n", kind_label(&e.kind)));
            match &e.text {
                Known::Value { value, .. } => match e.kind {
                    ActivityKind::UserMessage | ActivityKind::AgentMessage => {
                        out.push_str(value);
                        out.push_str("\n\n");
                    }
                    _ => {
                        out.push_str(&fenced(value));
                        out.push('\n');
                    }
                },
                _ => out.push_str(&format!("{NOT_FETCHED}\n\n")),
            }
        }
        out.push_str(&format!("_turn終端: {}_\n\n", end_label(turn.end)));
    }

    out.push_str("## 添付ファイル\n\n");
    out.push_str("ファイル名のみ。ファイルの中身は含めていません。\n\n");
    if input.attachments.is_empty() {
        out.push_str("なし\n\n");
    } else {
        for a in input.attachments {
            out.push_str(&attachment_line(a));
        }
        out.push('\n');
    }

    out.push_str("## 成果物\n\n");
    out.push_str("ファイル名のみ。ファイルの中身は含めていません。\n\n");
    if input.artifacts.is_empty() {
        out.push_str("なし\n\n");
    } else {
        for a in input.artifacts {
            out.push_str(&artifact_line(a));
        }
        out.push('\n');
    }

    if let Some(lines) = input.monitor_activity {
        out.push_str("## 監視活動履歴\n\n");
        out.push_str("このアプリが取得した範囲の記録です（子・孫を含む。取得できなかった期間の活動は含まれません）。\n\n");
        if lines.is_empty() {
            out.push_str("取得済みの監視活動はありません\n");
        } else {
            for l in lines {
                out.push_str(&activity_line(l));
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::local::AttachmentSource;

    fn ak(s: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn ck(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn ikey(i: &str) -> ItemKey {
        ItemKey { agent: ak("root"), turn_id: Some(ExternalId("t1".into())), item_id: ExternalId(i.into()) }
    }
    fn entry(kind: ActivityKind, text: Known<String>) -> TranscriptEntry {
        TranscriptEntry { key: ikey("i"), kind, text, phase: ActivityPhase::Completed }
    }
    fn turn(entries: Vec<TranscriptEntry>, end: Option<TurnEnd>, complete: bool) -> TurnRecord {
        TurnRecord { key: TurnKey { agent: ak("root"), turn_id: ExternalId("t1".into()) }, end, started_at: Known::NotFetched, completed_at: Known::NotFetched, entries, complete }
    }
    fn att(name: &str, original: &str) -> AttachmentEntry {
        AttachmentEntry {
            id: LocalId("att-1".into()),
            chat: ck("c"),
            kind: AttachmentKind::File,
            display_name: name.into(),
            source: AttachmentSource::File { original_path: original.into() },
            copy_path: Some(r"C:\data\chats\dir-1\attachments\att-1\secret-copy.txt".into()),
            size: Known::NotFetched,
            attached_at: UnixMillis(1),
            state: AttachmentState::Ready,
            used_by: vec![],
        }
    }
    fn art(path: &str, exists: Known<bool>) -> ArtifactEntry {
        ArtifactEntry { id: LocalId("art-1".into()), chat: ck("c"), agent: ak("root"), item: None, path: path.into(), observed_at: UnixMillis(1), exists, checked_at: None, in_chat_area: true }
    }
    fn input<'a>(turns: &'a [TurnRecord], atts: &'a [AttachmentEntry], arts: &'a [ArtifactEntry], act: Option<&'a [ActivityLine]>) -> ExportInput<'a> {
        ExportInput {
            chat_name: Known::direct("調査メモ".into()),
            turns,
            transcript_complete: turns.iter().all(|t| t.complete),
            attachments: atts,
            artifacts: arts,
            monitor_activity: act,
            exported_at_local: "2026-10-06 12:00".into(),
        }
    }

    #[test]
    fn fence_is_longer_than_any_backtick_run_in_the_text() {
        assert_eq!(fence_for(""), "```");
        assert_eq!(fence_for("plain `code` here"), "```");
        assert_eq!(fence_for("```rust\nfn x() {}\n```"), "````");
        assert_eq!(fence_for("a ```` b `` c"), "`````");
        // 離れた連続は足し合わせない。
        assert_eq!(fence_for("`` x ``"), "```");
        assert_eq!(fence_for("``````"), "```````");
    }

    #[test]
    fn code_blocks_are_not_broken_by_backticks_in_the_body() {
        let body = "```\ninner\n```";
        let t = [turn(vec![entry(ActivityKind::Command, Known::direct(body.into()))], Some(TurnEnd::Completed), true)];
        let md = render_markdown(&input(&t, &[], &[], None));
        assert!(md.contains(&format!("````\n{body}\n````\n")), "{md}");
    }

    #[test]
    fn attachments_and_artifacts_are_names_only() {
        let a = [att("report.xlsx", r"D:\private\report.xlsx")];
        let r = [art(r"C:\data\chats\dir-1\workspace\out\result.csv", Known::direct(true)), art(r"C:\x\gone.txt", Known::direct(false)), art(r"C:\x\unchecked.txt", Known::NotFetched)];
        let md = render_markdown(&input(&[], &a, &r, None));
        assert!(md.contains("- report.xlsx\n"));
        assert!(md.contains("- result.csv\n"));
        assert!(md.contains("- gone.txt（実在を確認できません）"));
        assert!(md.contains("- unchecked.txt（実在は未確認）"));
        // パス・コピー先・元パスは出さない。
        assert!(!md.contains("private") && !md.contains("secret-copy") && !md.contains("dir-1") && !md.contains(r"C:\x"));
    }

    #[test]
    fn monitor_activity_is_added_only_when_requested() {
        let lines = [
            ActivityLine::TurnEnded { at: UnixMillis(0), turn: TurnKey { agent: ak("child-1"), turn_id: ExternalId("t9".into()) }, end: TurnEnd::Failed },
            ActivityLine::Freshness { at: UnixMillis(86_400_000), agent: None, freshness: Freshness::Disconnected },
        ];
        let without = render_markdown(&input(&[], &[], &[], None));
        assert!(!without.contains("監視活動履歴"));
        let with = render_markdown(&input(&[], &[], &[], Some(&lines)));
        assert!(with.contains("## 監視活動履歴"));
        assert!(with.contains("1970-01-01 00:00:00 UTC turn終端: child-1（失敗）"));
        assert!(with.contains("1970-01-02 00:00:00 UTC 鮮度: 全体（切断）"));
        let empty = render_markdown(&input(&[], &[], &[], Some(&[])));
        assert!(empty.contains("取得済みの監視活動はありません"));
    }

    #[test]
    fn missing_parts_are_marked_not_fetched_and_never_blank() {
        let t = [turn(
            vec![entry(ActivityKind::UserMessage, Known::direct("こんにちは".into())), entry(ActivityKind::AgentMessage, Known::NotFetched)],
            None,
            false,
        )];
        let mut i = input(&t, &[], &[], None);
        i.chat_name = Known::NotFetched;
        let md = render_markdown(&i);
        assert!(md.starts_with("# （名前を確認できないチャット）"));
        assert!(md.contains("会話本文の一部を取得できていません"));
        assert!(md.contains("こんにちは"));
        assert!(md.contains("**エージェント**\n\n（未取得）"));
        assert!(md.contains("turn終端: 終端を確認できていません"));
        // 会話が1件も取れていないときも、空ではなく未取得と書く。
        assert!(render_markdown(&input(&[], &[], &[], None)).contains("## 会話\n\n（未取得）"));
    }

    #[test]
    fn utc_text_handles_epoch_and_leap_years() {
        assert_eq!(utc_text(UnixMillis(0)), "1970-01-01 00:00:00 UTC");
        // 2024-02-29 12:34:56 UTC
        assert_eq!(utc_text(UnixMillis(1_709_210_096_000)), "2024-02-29 12:34:56 UTC");
    }
}

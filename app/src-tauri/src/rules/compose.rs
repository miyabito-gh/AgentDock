//! 入力欄へ挿入する文章の組立て（段階③ P3-4、`app/DESIGN_P3.md` §1 #8・#11）。I/Oをしない。
//!
//! - 過去会話の参考指定: 履歴の本文を機械的に抜き出して引用ブロックにする。要約・加工はしない。取得できなかったturnは「未取得」と明示する。
//! - 本文の中にコードフェンス（```）があっても枠が壊れないよう、枠の長さは本文中の最長のバッククォート列より長くする。
//! - 会話は変更しない（読むだけ）。入力欄へ入れるのはUIで、ここは文字列を作るだけ。

use crate::backend::ipc::{ReferenceBlock, ReferenceTurn};
use crate::backend::model::*;

/// 参考ブロックの警告の目安（超えても止めない。文字数をUIに示す）。
pub const REFERENCE_WARN_CHARS: usize = 20_000;
/// 一覧に出すユーザー依頼の先頭の長さ（選ぶための目印。要約ではない）。
const PREVIEW_CHARS: usize = 60;

const NO_TEXT: &str = "（本文を取得できませんでした）";

/// 作業フォルダに作るAGENTS.mdの最小の雛形。既存のファイルは上書きしない（作成は新規作成のみ）。
pub const AGENTS_TEMPLATE: &str = "# AGENTS.md\n\nこのフォルダでCodexに守ってほしいことを書きます（不要な項目は消してください）。\n\n## プロジェクトの概要\n\n- \n\n## 作業のルール\n\n- \n\n## 確認の方法（ビルド・テストなど）\n\n- \n";

fn message_role(kind: &ActivityKind) -> Option<&'static str> {
    match kind {
        ActivityKind::UserMessage => Some("ユーザー"),
        ActivityKind::AgentMessage => Some("エージェント"),
        _ => None,
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 参考にできるturnの一覧（履歴の順）。会話本文（ユーザー依頼・エージェント応答）のないturnは候補にしない。
pub fn reference_turns(turns: &[TurnRecord]) -> Vec<ReferenceTurn> {
    turns
        .iter()
        .filter(|t| t.entries.iter().any(|e| message_role(&e.kind).is_some()))
        .map(|t| {
            let first_user = t.entries.iter().find(|e| e.kind == ActivityKind::UserMessage).and_then(|e| e.text.value());
            let preview: String = first_user.map(|s| one_line(s)).unwrap_or_default().chars().take(PREVIEW_CHARS).collect();
            ReferenceTurn {
                turn: t.key.turn_id.clone(),
                user_preview: preview,
                has_agent_text: t.entries.iter().any(|e| e.kind == ActivityKind::AgentMessage && e.text.value().is_some()),
                complete: t.complete,
            }
        })
        .collect()
}

/// 本文を囲む枠。本文中の最長のバッククォート列より長い（最低3）。
pub fn fence_for(text: &str) -> String {
    let (mut longest, mut run) = (0usize, 0usize);
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

/// 本文を枠で囲んだブロック（枠は本文より内側の衝突を避ける長さ）。
pub fn fenced(text: &str) -> String {
    let f = fence_for(text);
    format!("{f}text\n{text}\n{f}")
}

/// 1turn分の抜粋。会話本文だけ（ユーザー依頼とエージェント応答）を、履歴の順に並べる。
fn turn_body(t: &TurnRecord) -> String {
    let mut parts: Vec<String> = Vec::new();
    for e in &t.entries {
        let Some(role) = message_role(&e.kind) else { continue };
        let text = e.text.value().map(String::as_str).filter(|s| !s.is_empty()).unwrap_or(NO_TEXT);
        parts.push(format!("{role}:\n{text}"));
    }
    if parts.is_empty() {
        parts.push("（会話本文はありません）".into());
    }
    if !t.complete {
        parts.push("（このturnは一部しか取得できていません。本文が欠けている可能性があります）".into());
    }
    parts.join("\n\n")
}

/// 参考ブロックを組み立てる。`wanted` は抜粋するturn（渡された順）。履歴にないturnは `missing` に列挙し、本文にも「未取得」と書く。
/// 要約・省略・言い換えはしない。`title` は出所の会話名、`at_text` は参照した日時（呼び出し側が用意した表示用の文字列）。
pub fn build_reference(title: &str, at_text: &str, turns: &[TurnRecord], wanted: &[ExternalId]) -> ReferenceBlock {
    if wanted.is_empty() {
        return ReferenceBlock { text: String::new(), chars: 0, missing: Vec::new(), over_limit: false };
    }
    let mut missing: Vec<ExternalId> = Vec::new();
    let mut sections: Vec<String> = Vec::new();
    for (i, id) in wanted.iter().enumerate() {
        let n = i + 1;
        match turns.iter().find(|t| &t.key.turn_id == id) {
            Some(t) => sections.push(format!("--- turn {n} ---\n{}", turn_body(t))),
            None => {
                missing.push(id.clone());
                sections.push(format!("--- turn {n} ---\n（未取得: このturnを履歴から取得できませんでした）"));
            }
        }
    }
    let header = format!(
        "> 参考: 過去の会話「{}」の抜粋（参照日時 {at_text}、{}件のturn）。以下は原文のままの抜粋で、要約・加工はしていません。",
        one_line(title),
        wanted.len()
    );
    let text = format!("{header}\n{}\n", fenced(&sections.join("\n\n")));
    let chars = text.chars().count();
    ReferenceBlock { text, chars: chars as u32, missing, over_limit: chars > REFERENCE_WARN_CHARS }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ActivityKind, text: Option<&str>) -> TranscriptEntry {
        TranscriptEntry {
            key: ItemKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("th".into()) }, turn_id: None, item_id: ExternalId("i".into()) },
            kind,
            text: match text {
                Some(t) => Known::direct(t.to_string()),
                None => Known::Missing,
            },
            phase: ActivityPhase::Completed,
        }
    }

    fn turn(id: &str, complete: bool, entries: Vec<TranscriptEntry>) -> TurnRecord {
        TurnRecord {
            key: TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("th".into()) }, turn_id: ExternalId(id.into()) },
            end: Some(TurnEnd::Completed),
            started_at: Known::NotFetched,
            completed_at: Known::NotFetched,
            entries,
            complete,
        }
    }

    fn id(s: &str) -> ExternalId {
        ExternalId(s.into())
    }

    #[test]
    fn fence_is_longer_than_any_backtick_run_in_the_text() {
        assert_eq!(fence_for("plain"), "```");
        assert_eq!(fence_for("a ``` b"), "````");
        assert_eq!(fence_for("````x`"), "`````");
        // 離れたバッククォートは連続とみなさない。
        assert_eq!(fence_for("` ` ``"), "```");
    }

    #[test]
    fn reference_keeps_text_verbatim_and_survives_code_fences_inside() {
        let body = "見て:\n```rust\nfn main() {}\n```\n以上";
        let turns = vec![turn("t1", true, vec![entry(ActivityKind::UserMessage, Some(body)), entry(ActivityKind::AgentMessage, Some("了解"))])];
        let b = build_reference("元の会話", "2026-10-07 10:00:00", &turns, &[id("t1")]);
        assert!(b.missing.is_empty());
        assert!(b.text.contains(body), "the original text is not summarized or altered");
        assert!(b.text.contains("エージェント:\n了解"));
        // 本文に3連の枠があるので、外側の枠は4連になり、閉じ枠も同じ長さ。
        let lines: Vec<&str> = b.text.lines().collect();
        assert_eq!(lines.iter().filter(|l| **l == "````").count(), 1, "the closing fence matches the longer opening fence");
        assert!(lines.contains(&"````text"));
        assert_eq!(b.chars as usize, b.text.chars().count());
        assert!(!b.over_limit);
    }

    #[test]
    fn unfetched_turns_are_listed_and_stated_in_the_text() {
        let turns = vec![turn("t1", true, vec![entry(ActivityKind::UserMessage, Some("a"))])];
        let b = build_reference("c", "now", &turns, &[id("t1"), id("gone")]);
        assert_eq!(b.missing, vec![id("gone")]);
        assert!(b.text.contains("（未取得: このturnを履歴から取得できませんでした）"));
        // 取得できたturnは普通に入る。
        assert!(b.text.contains("--- turn 1 ---\nユーザー:\na"));
        assert!(b.text.contains("--- turn 2 ---"));
    }

    #[test]
    fn incomplete_turns_and_missing_texts_are_flagged_not_filled_in() {
        let turns = vec![turn("t1", false, vec![entry(ActivityKind::UserMessage, Some("q")), entry(ActivityKind::AgentMessage, None)])];
        let b = build_reference("c", "now", &turns, &[id("t1")]);
        assert!(b.text.contains("エージェント:\n（本文を取得できませんでした）"));
        assert!(b.text.contains("一部しか取得できていません"));
        assert!(b.missing.is_empty(), "found-but-partial turns are flagged in the text, not in the not-found list");
    }

    #[test]
    fn only_conversation_text_is_excerpted_and_empty_selection_makes_nothing() {
        let turns = vec![turn("t1", true, vec![entry(ActivityKind::Command, Some("rm -rf x")), entry(ActivityKind::UserMessage, Some("hi"))])];
        let b = build_reference("c", "now", &turns, &[id("t1")]);
        assert!(!b.text.contains("rm -rf"), "command output and tool calls are not part of the excerpt");
        let none = build_reference("c", "now", &turns, &[]);
        assert_eq!((none.text.as_str(), none.chars), ("", 0));
    }

    #[test]
    fn long_blocks_are_flagged_over_the_limit_but_still_built() {
        let big = "あ".repeat(REFERENCE_WARN_CHARS + 1);
        let turns = vec![turn("t1", true, vec![entry(ActivityKind::UserMessage, Some(&big))])];
        let b = build_reference("c", "now", &turns, &[id("t1")]);
        assert!(b.over_limit);
        assert!(b.text.contains(&big));
    }

    #[test]
    fn turn_list_skips_turns_without_conversation_text_and_previews_the_first_line() {
        let turns = vec![
            turn("t0", true, vec![entry(ActivityKind::Command, Some("ls"))]),
            turn("t1", false, vec![entry(ActivityKind::UserMessage, Some("  最初の\n  依頼です  ")), entry(ActivityKind::AgentMessage, Some("ok"))]),
            turn("t2", true, vec![entry(ActivityKind::UserMessage, Some(&"長".repeat(100)))]),
        ];
        let l = reference_turns(&turns);
        assert_eq!(l.iter().map(|t| t.turn.0.as_str()).collect::<Vec<_>>(), vec!["t1", "t2"]);
        assert_eq!(l[0].user_preview, "最初の 依頼です");
        assert!(l[0].has_agent_text && !l[0].complete);
        assert_eq!(l[1].user_preview.chars().count(), PREVIEW_CHARS);
        assert!(!l[1].has_agent_text);
    }
}

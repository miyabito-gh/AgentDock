//! 組織実行の依頼整理の純粋ロジック（DESIGN_ORG_INTAKE §1）。I/Oをしない。
//!
//! - 要約の turn の本文・出力形式、最終回答の解析と検査（長さ上限）
//! - 開始できない理由、Planner へ送る依頼文の組立て（決定的・16KB上限）
//! - 入力ファイルの分類（作業フォルダ内か、コミット済みか）
//! - `IntakeState` の遷移（切断・失敗・再要約・開始後の編集）と、起動時の `Started` の照合

use serde_json::{json, Value};

use super::compose::fenced;
use super::work_folders::{is_same_or_under, normalize_path};
use crate::backend::intake::*;
use crate::backend::model::{ActivityKind, MessagePhase, TurnEnd, TurnRecord, UnixMillis};

/// 文字列1つの上限（文字数）と、配列の上限（件数）。
pub const MAX_TEXT_CHARS: usize = 2_000;
pub const MAX_ITEMS: usize = 30;
/// Planner へ送る依頼文の上限（バイト）。
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// 要約の turn で送る固定の本文（§1.6）。履歴から該当 turn を探す目印にも使う。
pub const SUMMARY_REQUEST_TEXT: &str = "（AgentDock: 依頼要約の作成）ここまでの会話から、組織実行に渡す依頼を決められた形で出してください。分からない点は推測で埋めず openQuestions に入れてください。ファイルは変更しないでください。";

/// 要約の turn に添える但し書き。目印（`SUMMARY_REQUEST_TEXT`）の後ろに付けるので、履歴の照合は従来どおり目印で行える。
pub const SUMMARY_REQUEST_EXTRA: &str = "整理段階の制約（ファイルを変更しない・組織実行を開始しない・この段階では実装しない など、この会話の進め方に関する決まり）は、確定依頼の内容（goal・constraints など）に含めないでください。確定依頼は、後で実装を行う担当に渡す依頼です。";

/// 要約の turn で実際に送る本文。
pub fn summary_request_message() -> String {
    format!("{SUMMARY_REQUEST_TEXT}{SUMMARY_REQUEST_EXTRA}")
}

/// 依頼整理チャットの目的メモに入れる「進め方」（固定文。既存の資料枠で届く）。
pub const INTAKE_PURPOSE_TEXT: &str = "この会話は、組織実行（複数のエージェントに仕事を分担させる実行）に渡す依頼を整理するためのものです。まだ組織実行は始まっていません。\nこの会話では、ファイルの変更・コマンドの実行・実装を行わず、質問と確認と要約だけを行ってください。依頼を受けても実行せず、目的・成果物の形・入力ファイル・制約・完了条件を利用者に確認してください。足りない点は質問し、推測で決めつけないでください。ファイルは読むだけにしてください。内容が固まったら、利用者が要約を作ります。";

/// 依頼整理の間、手動送信のたびに本文の先頭（資料ブロック）へ添える固定の決まり。継続チャットの設定・目的メモの有無に関わらない。
pub const INTAKE_SEND_NOTICE: &str = "これは依頼整理の会話です。ファイルの変更・コマンド実行・実装は行わず、質問と確認と要約だけを行ってください。依頼を受けても実行せず、目的・成果物の形・入力ファイル・制約・完了条件を確認してください。";

/// 依頼整理チャットの権限。承認を出さず、書込みは sandbox（read-only）で拒否される。
pub const INTAKE_PERMISSION: crate::backend::backend::PermissionPreset = crate::backend::backend::PermissionPreset::ReadOnlyNoPrompt;

/// 依頼整理の「間」か（会話・要約中・要約あり）。やめた・開始済みは含めない。この間は権限を変えられず、送信のたびに進め方を添える。
pub fn intake_guard_active(state: Option<&IntakeState>) -> bool {
    matches!(state, Some(IntakeState::Drafting | IntakeState::Summarizing { .. } | IntakeState::Ready))
}

/// 要約の turn に付ける出力形式（`IntakeSummary` と同形。すべて必須・追加項目なし）。
pub fn summary_output_schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["goal", "deliverable", "inputs", "constraints", "doneCriteria", "openQuestions"],
        "properties": {
            "goal": { "type": "string" },
            "deliverable": { "type": "string" },
            "inputs": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["path", "note"],
                    "properties": { "path": { "type": "string" }, "note": { "type": "string" } }
                }
            },
            "constraints": strings.clone(),
            "doneCriteria": strings.clone(),
            "openQuestions": strings
        }
    })
}

// ───────────────────────────── 要約の解析と検査 ─────────────────────────────

fn str_field(v: &Value, key: &str) -> Result<String, String> {
    let s = v.get(key).ok_or_else(|| format!("{key} がありません"))?.as_str().ok_or_else(|| format!("{key} が文字列ではありません"))?;
    check_text(key, s)?;
    Ok(s.trim().to_string())
}

fn check_text(what: &str, s: &str) -> Result<(), String> {
    if s.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("{what} が{MAX_TEXT_CHARS}文字を超えています"));
    }
    Ok(())
}

fn list_field(v: &Value, key: &str) -> Result<Vec<String>, String> {
    let arr = v.get(key).ok_or_else(|| format!("{key} がありません"))?.as_array().ok_or_else(|| format!("{key} が配列ではありません"))?;
    if arr.len() > MAX_ITEMS {
        return Err(format!("{key} が{MAX_ITEMS}件を超えています"));
    }
    let mut out = Vec::new();
    for item in arr {
        let s = item.as_str().ok_or_else(|| format!("{key} に文字列でない項目があります"))?;
        check_text(key, s)?;
        let s = s.trim();
        if !s.is_empty() {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

/// コードフェンスで囲まれていたら中身だけを取り出す（出力形式を付けても、まれに囲まれて返るため）。
pub(crate) fn strip_fence(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else { return t };
    let Some(body) = rest.strip_suffix("```") else { return t };
    // 先頭の言語名の行（`json` など）を捨てる。
    match body.split_once('\n') {
        Some((first, tail)) if !first.contains('{') => tail.trim(),
        _ => body.trim(),
    }
}

/// turn の最終回答を要約として読む。型と長さ上限を検査し、満たさなければ理由を返す。
pub fn parse_summary_output(text: &str) -> Result<IntakeSummary, String> {
    let v: Value = serde_json::from_str(strip_fence(text)).map_err(|_| "JSONとして読めませんでした".to_string())?;
    if !v.is_object() {
        return Err("JSONのオブジェクトではありません".to_string());
    }
    let mut inputs = Vec::new();
    let arr = v.get("inputs").ok_or_else(|| "inputs がありません".to_string())?.as_array().ok_or_else(|| "inputs が配列ではありません".to_string())?;
    if arr.len() > MAX_ITEMS {
        return Err(format!("inputs が{MAX_ITEMS}件を超えています"));
    }
    for item in arr {
        let path = str_field(item, "path").map_err(|m| format!("inputs: {m}"))?;
        let note = str_field(item, "note").map_err(|m| format!("inputs: {m}"))?;
        if !path.is_empty() {
            inputs.push(IntakeInput { path, note });
        }
    }
    Ok(IntakeSummary {
        goal: str_field(&v, "goal")?,
        deliverable: str_field(&v, "deliverable")?,
        inputs,
        constraints: list_field(&v, "constraints")?,
        done_criteria: list_field(&v, "doneCriteria")?,
        open_questions: list_field(&v, "openQuestions")?,
    })
}

/// 利用者が編集した要約の型・長さの検査（保存の前に通す）。
pub fn check_summary_limits(s: &IntakeSummary) -> Result<(), String> {
    check_text("目的", &s.goal)?;
    check_text("成果物の形", &s.deliverable)?;
    for (name, list) in [("制約", &s.constraints), ("完了条件", &s.done_criteria), ("未解決の点", &s.open_questions)] {
        if list.len() > MAX_ITEMS {
            return Err(format!("{name}は{MAX_ITEMS}件までにしてください"));
        }
        for t in list {
            check_text(name, t)?;
        }
    }
    if s.inputs.len() > MAX_ITEMS {
        return Err(format!("入力ファイルは{MAX_ITEMS}件までにしてください"));
    }
    for i in &s.inputs {
        check_text("入力ファイルのパス", &i.path)?;
        check_text("入力ファイルのメモ", &i.note)?;
    }
    Ok(())
}

/// 開始を止める理由（目的が空・完了条件が0件）。警告だけのもの（入力の検査結果）は含めない。
pub fn summary_blockers(s: &IntakeSummary) -> Vec<String> {
    let mut out = Vec::new();
    if s.goal.trim().is_empty() {
        out.push("目的が空です".to_string());
    }
    if !s.done_criteria.iter().any(|c| !c.trim().is_empty()) {
        out.push("完了条件が1件もありません".to_string());
    }
    out
}

// ───────────────────────────── 依頼文の組立て ─────────────────────────────

/// 入力の検査結果の表示文。
pub fn input_label(c: InputCheck) -> &'static str {
    match c {
        InputCheck::Committed => "コミット済み",
        InputCheck::Uncommitted => "未コミットまたは変更あり（Workerの作業場所には最新が入りません）",
        InputCheck::Outside => "作業フォルダの外です（Workerからは見えません）",
        InputCheck::Missing => "見つかりません",
        InputCheck::Unchecked => "検査できていません",
    }
}

fn section_text(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        "（なし）".to_string()
    } else if t.contains("```") {
        fenced(t)
    } else {
        t.to_string()
    }
}

fn bullets(items: &[String]) -> String {
    let items: Vec<&String> = items.iter().filter(|s| !s.trim().is_empty()).collect();
    if items.is_empty() {
        return "（なし）".to_string();
    }
    items
        .iter()
        .map(|s| {
            let t = s.trim();
            if t.contains("```") {
                format!("-\n{}", fenced(t))
            } else {
                format!("- {}", t.split_whitespace().collect::<Vec<_>>().join(" "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 確定依頼の先頭に付ける注記（整理段階の制約を Planner・Worker へ持ち込まない）。
pub const PLANNER_REQUEST_NOTE: &str = "以下は依頼整理を終えた確定依頼である。依頼整理の段階の制約（ファイルを変更しない・組織実行を開始しない 等）はこの依頼には適用しない。";

/// Planner への依頼文。同じ入力から必ず同じ文を作る。`checks` は入力パスごとの検査結果（なければ「検査できていません」）。
/// 16KB を超えるときは作らず理由を返す。
pub fn compose_planner_request(rev: u32, s: &IntakeSummary, checks: &[(String, InputCheck)]) -> Result<String, String> {
    let inputs = if s.inputs.iter().all(|i| i.path.trim().is_empty()) {
        "（なし）".to_string()
    } else {
        s.inputs
            .iter()
            .filter(|i| !i.path.trim().is_empty())
            .map(|i| {
                let check = checks.iter().find(|(p, _)| p == &i.path).map(|(_, c)| *c).unwrap_or(InputCheck::Unchecked);
                let note = i.note.trim();
                let note = if note.is_empty() { String::new() } else { format!(" — {}", note.split_whitespace().collect::<Vec<_>>().join(" ")) };
                format!("- {}{}（検査: {}）", i.path.trim(), note, input_label(check))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let text = format!(
        "# 依頼（依頼整理で確定・第{rev}版）\n{PLANNER_REQUEST_NOTE}\n\n## 目的\n{}\n\n## 成果物の形\n{}\n\n## 入力（作業フォルダからの相対パス。各行に検査結果）\n{}\n\n## 制約\n{}\n\n## 完了条件\n{}\n\n## 未解決の点（あれば。Planner は推測で埋めず計画に明記すること）\n{}\n",
        section_text(&s.goal),
        section_text(&s.deliverable),
        inputs,
        bullets(&s.constraints),
        bullets(&s.done_criteria),
        bullets(&s.open_questions),
    );
    if text.len() > MAX_REQUEST_BYTES {
        return Err(format!("依頼文が{}KBを超えています（{}バイト）。内容を短くしてください", MAX_REQUEST_BYTES / 1024, text.len()));
    }
    Ok(text)
}

// ───────────────────────────── 入力ファイルの分類 ─────────────────────────────

/// 入力パスを作業フォルダに対して解く。作業フォルダの中なら、そこからの相対パス（`/` 区切り）と絶対パス。外・解けないときは None。
/// 相対パスは作業フォルダ基準、ドライブ付きの絶対パスはそのまま扱う。
pub fn resolve_input(folder: &str, path: &str) -> Option<(String, String)> {
    let base = normalize_path(folder)?;
    let raw = path.trim();
    let abs = if raw.len() >= 2 && raw.as_bytes()[1] == b':' { raw.to_string() } else { format!("{}\\{}", base.trim_end_matches('\\'), raw) };
    let n = normalize_path(&abs)?;
    if !is_same_or_under(&n, &base) || n.eq_ignore_ascii_case(base.trim_end_matches('\\')) {
        return None;
    }
    let rel = n.get(base.trim_end_matches('\\').len()..)?.trim_start_matches('\\').replace('\\', "/");
    Some((rel, n))
}

/// 入力ファイルについて集めた事実（ホストが読取りで集める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFacts {
    pub exists: bool,
    /// 作業フォルダの中か。
    pub inside: bool,
    /// Git で調べられたか（調べられなければ `tracked`・`changed` は無意味）。
    pub git_checked: bool,
    pub tracked: bool,
    /// 未コミットの変更（または未追跡）がある。
    pub changed: bool,
}

pub fn classify_input(f: &InputFacts) -> InputCheck {
    if !f.exists {
        return InputCheck::Missing;
    }
    if !f.inside {
        return InputCheck::Outside;
    }
    if !f.git_checked {
        return InputCheck::Unchecked;
    }
    if f.tracked && !f.changed {
        InputCheck::Committed
    } else {
        InputCheck::Uncommitted
    }
}

// ───────────────────────────── 要約の turn の特定 ─────────────────────────────

/// 履歴の1 turn から、要約の特定に使う事実だけを取り出したもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFacts {
    pub user_texts: Vec<String>,
    pub started_at_ms: Option<i64>,
    pub end: Option<TurnEnd>,
    /// 最終回答の本文（最終の位置づけの発言を優先し、なければ最後の発言）。
    pub final_text: Option<String>,
}

pub fn facts_of(t: &TurnRecord) -> TurnFacts {
    let agent_texts = |only_final: bool| -> Option<String> {
        t.entries
            .iter()
            .filter(|e| e.kind == ActivityKind::AgentMessage && (!only_final || e.message_phase == MessagePhase::Final))
            .filter_map(|e| e.text.value().cloned())
            .filter(|s| !s.trim().is_empty())
            .next_back()
    };
    TurnFacts {
        user_texts: t.entries.iter().filter(|e| e.kind == ActivityKind::UserMessage).filter_map(|e| e.text.value().cloned()).collect(),
        started_at_ms: t.started_at.value().map(|u| u.0),
        end: t.end,
        final_text: agent_texts(true).or_else(|| agent_texts(false)),
    }
}

/// 履歴から見つけた要約の turn の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryTurn {
    /// 要約の turn が見つからない（送信が届いていない・まだ履歴にない・時刻を確認できない）。
    NotFound,
    /// まだ終わっていない。
    Running,
    Failed(String),
    Done(String),
}

/// 時計のずれ・送信直前の記録の余裕（ミリ秒）。
const SENT_AT_SLACK_MS: i64 = 5_000;

/// 一番新しい要約の turn を選ぶ。押した時刻より前の turn は、前の要約なので取り込まない（時刻を確認できない turn も取り込まない）。
pub fn pick_summary_turn(turns: &[TurnFacts], sent_at_ms: i64) -> SummaryTurn {
    let Some(t) = turns.iter().rev().find(|t| t.user_texts.iter().any(|u| u.contains(SUMMARY_REQUEST_TEXT))) else {
        return SummaryTurn::NotFound;
    };
    match t.started_at_ms {
        Some(at) if at + SENT_AT_SLACK_MS >= sent_at_ms => {}
        _ => return SummaryTurn::NotFound,
    }
    match t.end {
        None => SummaryTurn::Running,
        Some(TurnEnd::Failed) => SummaryTurn::Failed("要約の turn が失敗しました".to_string()),
        Some(TurnEnd::Interrupted) => SummaryTurn::Failed("要約の turn が中断されました".to_string()),
        Some(TurnEnd::Completed) => match &t.final_text {
            Some(text) => SummaryTurn::Done(text.clone()),
            None => SummaryTurn::Failed("要約の最終回答が見つかりませんでした".to_string()),
        },
    }
}

// ───────────────────────────── 状態の遷移 ─────────────────────────────

/// 状態を動かす出来事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeEvent {
    /// 利用者が「要約を作る」を押し、送信した。
    SummaryRequested { client_message_id: String, at: UnixMillis },
    /// 要約の turn の結果を読めた。
    SummaryParsed(IntakeSummary),
    /// turn が失敗・中断した、または結果を読み取れなかった。
    SummaryFailed(String),
    /// 要約の送信が拒否された（送っていない）。
    SummaryRejected(String),
    /// 利用者が要約・設定を編集した（手で書く含む）。
    Edited { summary: IntakeSummary, prefs: IntakeStartPrefs },
    /// 開始した。
    StartRecorded { workflow_id: String, at: UnixMillis },
    Abandon,
}

/// 遷移。許されない出来事は理由を返し、記録は変えない。
pub fn apply_event(rec: &IntakeRecord, ev: IntakeEvent) -> Result<IntakeRecord, String> {
    let mut r = rec.clone();
    match (&rec.state, ev) {
        (IntakeState::Abandoned, IntakeEvent::Abandon) => {}
        (IntakeState::Abandoned, _) => return Err("この依頼整理はやめています".to_string()),
        (_, IntakeEvent::Abandon) => r.state = IntakeState::Abandoned,
        (IntakeState::Started { .. }, IntakeEvent::SummaryRequested { .. }) => return Err("開始済みです。要約を直してから、もう一度作ってください".to_string()),
        (_, IntakeEvent::SummaryRequested { client_message_id, at }) => {
            r.state = IntakeState::Summarizing { client_message_id, sent_at: at };
            r.last_error = None;
        }
        (IntakeState::Summarizing { .. }, IntakeEvent::SummaryParsed(summary)) => {
            r.summary = Some(summary);
            r.rev += 1;
            r.state = IntakeState::Ready;
            r.last_error = None;
        }
        (IntakeState::Summarizing { .. }, IntakeEvent::SummaryFailed(reason) | IntakeEvent::SummaryRejected(reason)) => {
            r.state = IntakeState::Drafting;
            r.last_error = Some(reason);
        }
        (_, IntakeEvent::SummaryParsed(_) | IntakeEvent::SummaryFailed(_) | IntakeEvent::SummaryRejected(_)) => return Err("要約を作っている途中ではありません".to_string()),
        (IntakeState::Summarizing { .. }, IntakeEvent::Edited { .. }) => return Err("要約を作っている間は編集できません。結果を確認してください".to_string()),
        (_, IntakeEvent::Edited { summary, prefs }) => {
            r.summary = Some(summary);
            r.prefs = prefs;
            r.rev += 1;
            r.state = IntakeState::Ready;
            r.last_error = None;
        }
        (IntakeState::Ready, IntakeEvent::StartRecorded { workflow_id, at }) => {
            r.started.push(IntakeStarted { workflow_id: workflow_id.clone(), rev: rec.rev, at });
            r.state = IntakeState::Started { workflow_id, rev: rec.rev, at };
        }
        (_, IntakeEvent::StartRecorded { .. }) => return Err("開始できる状態ではありません".to_string()),
    }
    Ok(r)
}

/// 開始時に書いた `intake.json` の写しから読んだ印。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedMark {
    pub intake_id: String,
    pub rev: u32,
    pub workflow_id: String,
    pub at: UnixMillis,
}

/// 起動時の照合。ワークフローは動いたのに chat.json の `Started` 書込みが失敗していたとき、記録を直す。直すものがなければ None。
/// 要約の版が開始版のままなら `Started` にし、編集が進んでいれば履歴（`started`）だけに足す。
pub fn reconcile_intake_started(rec: &IntakeRecord, mark: &StartedMark) -> Option<IntakeRecord> {
    if rec.id.0 != mark.intake_id || rec.started.iter().any(|s| s.workflow_id == mark.workflow_id) {
        return None;
    }
    let mut r = rec.clone();
    r.started.push(IntakeStarted { workflow_id: mark.workflow_id.clone(), rev: mark.rev, at: mark.at });
    if matches!(rec.state, IntakeState::Ready) && rec.rev == mark.rev {
        r.state = IntakeState::Started { workflow_id: mark.workflow_id.clone(), rev: mark.rev, at: mark.at };
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::LocalId;

    fn good_json() -> String {
        json!({
            "goal": "議事録を整える",
            "deliverable": "docx",
            "inputs": [{ "path": "notes/a.md", "note": "元" }],
            "constraints": ["敬語"],
            "doneCriteria": ["誤字がない"],
            "openQuestions": []
        })
        .to_string()
    }

    fn sum() -> IntakeSummary {
        parse_summary_output(&good_json()).unwrap()
    }

    fn rec(state: IntakeState) -> IntakeRecord {
        IntakeRecord { id: LocalId("i1".into()), state, summary: None, rev: 0, prefs: IntakeStartPrefs::default(), last_error: None, started: vec![], write_only: false }
    }

    #[test]
    fn intake_guard_and_permission() {
        use crate::backend::backend::PermissionPreset;
        assert_eq!(INTAKE_PERMISSION, PermissionPreset::ReadOnlyNoPrompt);
        assert!(intake_guard_active(Some(&IntakeState::Drafting)));
        assert!(intake_guard_active(Some(&IntakeState::Ready)));
        assert!(intake_guard_active(Some(&IntakeState::Summarizing { client_message_id: "m".into(), sent_at: UnixMillis(1) })));
        assert!(!intake_guard_active(Some(&IntakeState::Abandoned)));
        assert!(!intake_guard_active(Some(&IntakeState::Started { workflow_id: "w".into(), rev: 1, at: UnixMillis(1) })));
        assert!(!intake_guard_active(None));
        assert!(INTAKE_SEND_NOTICE.contains("実行") && INTAKE_PURPOSE_TEXT.contains("質問と確認と要約だけ"));
    }

    #[test]
    fn parse_reads_a_valid_summary_and_a_fenced_one() {
        let s = sum();
        assert_eq!(s.goal, "議事録を整える");
        assert_eq!(s.inputs, vec![IntakeInput { path: "notes/a.md".into(), note: "元".into() }]);
        assert!(s.open_questions.is_empty());
        let fenced_json = format!("```json\n{}\n```", good_json());
        assert_eq!(parse_summary_output(&fenced_json).unwrap(), s);
    }

    #[test]
    fn parse_rejects_missing_wrong_typed_and_oversized() {
        assert!(parse_summary_output("not json").is_err());
        assert!(parse_summary_output("[]").is_err());
        let mut v: Value = serde_json::from_str(&good_json()).unwrap();
        v.as_object_mut().unwrap().remove("doneCriteria");
        assert!(parse_summary_output(&v.to_string()).unwrap_err().contains("doneCriteria"));
        let mut v: Value = serde_json::from_str(&good_json()).unwrap();
        v["goal"] = json!(3);
        assert!(parse_summary_output(&v.to_string()).unwrap_err().contains("goal"));
        let mut v: Value = serde_json::from_str(&good_json()).unwrap();
        v["constraints"] = json!([1]);
        assert!(parse_summary_output(&v.to_string()).is_err());
        let mut v: Value = serde_json::from_str(&good_json()).unwrap();
        v["goal"] = json!("あ".repeat(MAX_TEXT_CHARS + 1));
        assert!(parse_summary_output(&v.to_string()).unwrap_err().contains("文字を超えて"));
        let mut v: Value = serde_json::from_str(&good_json()).unwrap();
        v["openQuestions"] = json!(vec!["q"; MAX_ITEMS + 1]);
        assert!(parse_summary_output(&v.to_string()).unwrap_err().contains("件を超えて"));
        let mut v: Value = serde_json::from_str(&good_json()).unwrap();
        v["inputs"] = json!([{ "path": "a" }]);
        assert!(parse_summary_output(&v.to_string()).unwrap_err().contains("note"));
    }

    #[test]
    fn blockers_are_empty_goal_and_no_done_criteria_only() {
        assert!(summary_blockers(&sum()).is_empty());
        let mut s = sum();
        s.goal = "  ".into();
        s.done_criteria = vec!["".into()];
        assert_eq!(summary_blockers(&s).len(), 2);
    }

    #[test]
    fn compose_is_deterministic_marks_empty_sections_and_includes_checks() {
        let mut s = sum();
        s.open_questions.clear();
        s.constraints.clear();
        let checks = vec![("notes/a.md".to_string(), InputCheck::Uncommitted)];
        let a = compose_planner_request(2, &s, &checks).unwrap();
        assert_eq!(a, compose_planner_request(2, &s, &checks).unwrap());
        assert!(a.starts_with("# 依頼（依頼整理で確定・第2版）"));
        assert!(a.contains("## 制約\n（なし）"));
        assert!(a.contains("- notes/a.md — 元（検査: 未コミットまたは変更あり"));
        assert!(a.contains("## 完了条件\n- 誤字がない"));
        // a missing check is shown as unchecked, never as fine
        assert!(compose_planner_request(1, &s, &[]).unwrap().contains("検査: 検査できていません"));
    }

    #[test]
    fn compose_uses_a_longer_fence_for_text_that_contains_a_fence_and_refuses_over_16kb() {
        let mut s = sum();
        s.goal = "例:\n```\ncode\n```".into();
        let t = compose_planner_request(1, &s, &[]).unwrap();
        assert!(t.contains("````text\n例:\n```\ncode\n```\n````"), "{t}");
        s.goal = "あ".repeat(MAX_TEXT_CHARS);
        s.deliverable = "い".repeat(MAX_TEXT_CHARS);
        s.constraints = vec!["う".repeat(MAX_TEXT_CHARS)];
        s.open_questions = vec!["え".repeat(MAX_TEXT_CHARS)];
        assert!(compose_planner_request(1, &s, &[]).unwrap_err().contains("16KB"));
    }

    #[test]
    fn the_confirmed_request_says_intake_constraints_do_not_apply_and_the_summary_ask_keeps_its_marker() {
        let t = compose_planner_request(1, &sum(), &[]).unwrap();
        assert!(t.contains(PLANNER_REQUEST_NOTE), "{t}");
        assert!(t.starts_with("# 依頼（依頼整理で確定・第1版）"), "{t}");
        let m = summary_request_message();
        assert!(m.starts_with(SUMMARY_REQUEST_TEXT) && m.contains("含めないでください"));
    }

    #[test]
    fn resolve_input_keeps_paths_inside_the_folder_only() {
        assert_eq!(resolve_input("C:\\work\\p", "notes/a.md"), Some(("notes/a.md".into(), "C:\\work\\p\\notes\\a.md".into())));
        assert_eq!(resolve_input("C:\\work\\p", "C:\\work\\p\\b.txt").map(|r| r.0), Some("b.txt".into()));
        assert_eq!(resolve_input("C:\\work\\p", "..\\other\\a.md"), None);
        assert_eq!(resolve_input("C:\\work\\p", "D:\\x.md"), None);
        assert_eq!(resolve_input("C:\\work\\p", "."), None);
    }

    #[test]
    fn classify_input_orders_missing_outside_unchecked_committed() {
        let f = |exists, inside, git_checked, tracked, changed| classify_input(&InputFacts { exists, inside, git_checked, tracked, changed });
        assert_eq!(f(false, true, true, true, false), InputCheck::Missing);
        assert_eq!(f(true, false, true, true, false), InputCheck::Outside);
        assert_eq!(f(true, true, false, false, false), InputCheck::Unchecked);
        assert_eq!(f(true, true, true, true, false), InputCheck::Committed);
        assert_eq!(f(true, true, true, true, true), InputCheck::Uncommitted);
        assert_eq!(f(true, true, true, false, true), InputCheck::Uncommitted);
    }

    fn at(n: i64) -> UnixMillis {
        UnixMillis(n)
    }

    fn tf(user: &str, started: Option<i64>, end: Option<TurnEnd>, text: Option<&str>) -> TurnFacts {
        TurnFacts { user_texts: vec![user.to_string()], started_at_ms: started, end, final_text: text.map(str::to_string) }
    }

    #[test]
    fn the_newest_summary_turn_is_picked_and_old_or_untimed_ones_are_not() {
        let req = format!("【資料】
{SUMMARY_REQUEST_TEXT}");
        let done = tf(&req, Some(10_000), Some(TurnEnd::Completed), Some("{}"));
        // none at all, or no matching request
        assert_eq!(pick_summary_turn(&[], 0), SummaryTurn::NotFound);
        assert_eq!(pick_summary_turn(&[tf("こんにちは", Some(1), Some(TurnEnd::Completed), Some("x"))], 0), SummaryTurn::NotFound);
        // a summary made before this request was sent is a previous one
        assert_eq!(pick_summary_turn(std::slice::from_ref(&done), 20_000), SummaryTurn::NotFound);
        // within the slack of the send time
        assert_eq!(pick_summary_turn(std::slice::from_ref(&done), 14_000), SummaryTurn::Done("{}".into()));
        // an unknown start time is not adopted
        assert_eq!(pick_summary_turn(&[tf(&req, None, Some(TurnEnd::Completed), Some("{}"))], 0), SummaryTurn::NotFound);
        // running / failed / interrupted / completed without an answer
        assert_eq!(pick_summary_turn(&[tf(&req, Some(10_000), None, None)], 9_000), SummaryTurn::Running);
        assert!(matches!(pick_summary_turn(&[tf(&req, Some(10_000), Some(TurnEnd::Failed), None)], 9_000), SummaryTurn::Failed(_)));
        assert!(matches!(pick_summary_turn(&[tf(&req, Some(10_000), Some(TurnEnd::Interrupted), Some("x"))], 9_000), SummaryTurn::Failed(m) if m.contains("中断")));
        assert!(matches!(pick_summary_turn(&[tf(&req, Some(10_000), Some(TurnEnd::Completed), None)], 9_000), SummaryTurn::Failed(_)));
        // the newest matching turn wins over an older one
        let older = tf(&req, Some(10_000), Some(TurnEnd::Completed), Some("old"));
        let newer = tf(&req, Some(30_000), Some(TurnEnd::Completed), Some("new"));
        assert_eq!(pick_summary_turn(&[older, tf("途中", Some(20_000), Some(TurnEnd::Completed), Some("x")), newer], 29_000), SummaryTurn::Done("new".into()));
    }

    #[test]
    fn happy_path_summarize_edit_start() {
        let r = rec(IntakeState::Drafting);
        let r = apply_event(&r, IntakeEvent::SummaryRequested { client_message_id: "cm1".into(), at: at(1) }).unwrap();
        assert!(matches!(r.state, IntakeState::Summarizing { .. }));
        let r = apply_event(&r, IntakeEvent::SummaryParsed(sum())).unwrap();
        assert_eq!((r.state.clone(), r.rev), (IntakeState::Ready, 1));
        let r = apply_event(&r, IntakeEvent::StartRecorded { workflow_id: "wf-1".into(), at: at(2) }).unwrap();
        assert_eq!(r.state, IntakeState::Started { workflow_id: "wf-1".into(), rev: 1, at: at(2) });
        assert_eq!(r.started.len(), 1);
        // editing after start goes back to Ready with rev+1 and keeps the history
        let r = apply_event(&r, IntakeEvent::Edited { summary: sum(), prefs: IntakeStartPrefs::default() }).unwrap();
        assert_eq!((r.state.clone(), r.rev, r.started.len()), (IntakeState::Ready, 2, 1));
    }

    #[test]
    fn failure_goes_back_to_drafting_and_a_second_summary_is_an_explicit_new_request() {
        let r = apply_event(&rec(IntakeState::Drafting), IntakeEvent::SummaryRequested { client_message_id: "cm1".into(), at: at(1) }).unwrap();
        let failed = apply_event(&r, IntakeEvent::SummaryFailed("中断されました".into())).unwrap();
        assert_eq!(failed.state, IntakeState::Drafting);
        assert_eq!(failed.last_error.as_deref(), Some("中断されました"));
        // while the result is unconfirmed, "もう一度要約を作る" replaces the pending request with a new one
        let again = apply_event(&r, IntakeEvent::SummaryRequested { client_message_id: "cm2".into(), at: at(5) }).unwrap();
        assert!(matches!(&again.state, IntakeState::Summarizing { client_message_id, .. } if client_message_id == "cm2"));
        // a result with nothing pending is refused
        assert!(apply_event(&rec(IntakeState::Ready), IntakeEvent::SummaryParsed(sum())).is_err());
        assert!(apply_event(&rec(IntakeState::Ready), IntakeEvent::SummaryFailed("x".into())).is_err());
    }

    #[test]
    fn editing_while_summarizing_start_from_non_ready_and_abandoned_are_refused() {
        let s = rec(IntakeState::Summarizing { client_message_id: "c".into(), sent_at: at(1) });
        assert!(apply_event(&s, IntakeEvent::Edited { summary: sum(), prefs: IntakeStartPrefs::default() }).is_err());
        assert!(apply_event(&rec(IntakeState::Drafting), IntakeEvent::StartRecorded { workflow_id: "w".into(), at: at(1) }).is_err());
        let abandoned = apply_event(&rec(IntakeState::Drafting), IntakeEvent::Abandon).unwrap();
        assert!(apply_event(&abandoned, IntakeEvent::Edited { summary: sum(), prefs: IntakeStartPrefs::default() }).is_err());
        assert!(apply_event(&abandoned, IntakeEvent::Abandon).is_ok());
        assert!(apply_event(&rec(IntakeState::Started { workflow_id: "w".into(), rev: 1, at: at(1) }), IntakeEvent::SummaryRequested { client_message_id: "c".into(), at: at(2) }).is_err());
    }

    #[test]
    fn reconcile_fills_started_only_when_missing_and_keeps_edited_state() {
        let mark = StartedMark { intake_id: "i1".into(), rev: 1, workflow_id: "wf-9".into(), at: at(9) };
        let mut ready = rec(IntakeState::Ready);
        ready.rev = 1;
        let fixed = reconcile_intake_started(&ready, &mark).unwrap();
        assert_eq!(fixed.state, IntakeState::Started { workflow_id: "wf-9".into(), rev: 1, at: at(9) });
        assert_eq!(fixed.started.len(), 1);
        // already recorded: nothing to fix
        assert!(reconcile_intake_started(&fixed, &mark).is_none());
        // edited since the start: only the history is added
        let mut edited = rec(IntakeState::Ready);
        edited.rev = 2;
        let r = reconcile_intake_started(&edited, &mark).unwrap();
        assert_eq!((r.state.clone(), r.started.len()), (IntakeState::Ready, 1));
        // another intake: untouched
        assert!(reconcile_intake_started(&ready, &StartedMark { intake_id: "other".into(), ..mark }).is_none());
    }

    #[test]
    fn output_schema_requires_every_summary_key() {
        let s = summary_output_schema();
        assert_eq!(s["required"].as_array().unwrap().len(), 6);
        assert_eq!(s["additionalProperties"], json!(false));
    }
}

//! レビュー・分岐・圧縮の純粋な判定（段階③ P3-2、`app/DESIGN_P3.md` §1 #3・#4・#6・#7）。I/Oをしない。
//!
//! - レビュー対象の可否（作業フォルダがGitリポジトリでないときは指示だけ）。
//! - 分岐の受理不明の照合: 候補が「ちょうど1件」のときだけ採用する（0件・複数件・時刻不明は採用しない）。
//! - 受理不明の操作（レビュー・圧縮）の照合: 開始後のturnに痕跡があれば観測、痕跡なしは十分な時間が経ち履歴が完全なときだけ。
//!   どの結果でも再送の根拠にはしない（再実行はユーザーの新しい操作）。

use crate::backend::model::*;
use crate::backend::parity::ReviewTarget;

/// 候補の時刻照合の許容幅（ホストとバックエンドの時計・丸めのずれ）。
const SINCE_SLACK_MS: i64 = 2_000;
/// 開始から、この時間が経つまでは「痕跡なし」と確定しない（遅れて処理される要求を二重に実行しないため）。
pub const NOT_OBSERVED_MIN_AGE_MS: i64 = 120_000;

// ───────────────────────────── レビュー対象 ─────────────────────────────

/// 作業フォルダの状態から見た、レビュー対象の可否。`Ok` 以外は理由（利用者に出す文）を返す。
/// `repo_ready`: 作業フォルダがGitリポジトリと確認できたか。`branches`: 確認できたローカルブランチ。
pub fn review_target_check(target: &ReviewTarget, repo_ready: bool, branches: &[String]) -> Result<(), String> {
    const NEEDS_GIT: &str = "作業フォルダがGitのリポジトリと確認できないため、このレビュー対象は選べません（指示を書いて依頼する方式だけ使えます）";
    match target {
        ReviewTarget::Custom { instructions } => {
            if instructions.trim().is_empty() {
                Err("レビューの指示が空です".into())
            } else {
                Ok(())
            }
        }
        ReviewTarget::UncommittedChanges => {
            if repo_ready {
                Ok(())
            } else {
                Err(NEEDS_GIT.into())
            }
        }
        ReviewTarget::BaseBranch { branch } => {
            if !repo_ready {
                Err(NEEDS_GIT.into())
            } else if !branches.iter().any(|b| b == branch) {
                Err(format!("ブランチ「{branch}」が見つかりません"))
            } else {
                Ok(())
            }
        }
        ReviewTarget::Commit { sha, .. } => {
            if !repo_ready {
                Err(NEEDS_GIT.into())
            } else if !is_commit_sha(sha) {
                Err("コミットのIDは、7〜40桁の16進数で指定してください".into())
            } else {
                Ok(())
            }
        }
    }
}

/// コミットIDの形（短縮形を含む16進数7〜40桁）。
pub fn is_commit_sha(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `git for-each-ref` の出力（1行1ブランチ）。空行は除く。
pub fn parse_branch_lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// `git log --format=%H<TAB>%s` の出力。IDの形でない行は読み飛ばす（形の違う出力を候補にしない）。
pub fn parse_commit_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (sha, title) = l.split_once('\t')?;
            let sha = sha.trim();
            is_commit_sha(sha).then(|| (sha.to_string(), title.trim().to_string()))
        })
        .collect()
}

// ───────────────────────────── 分岐の照合 ─────────────────────────────

/// 一覧から読んだ会話1件の、照合に使う項目（取れない項目は None）。
#[derive(Debug, Clone, PartialEq)]
pub struct ForkCandidate {
    pub chat: ChatKey,
    /// 分岐元として示されたID。
    pub forked_from: Option<String>,
    pub created_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForkMatch {
    /// 分岐元が一致し、開始以降に作られた候補がちょうど1件。
    One(ChatKey),
    /// 該当する候補がない（まだ見えていない可能性もあるので「分岐なし」とは言わない）。
    None,
    /// 複数件、または時刻を確認できない候補があり、1件に絞れない。
    Ambiguous(usize),
}

/// 受理不明の分岐の照合。`since` 以降（許容幅つき）に作られ、`source_id` から分岐した会話がちょうど1件のときだけ採用する。
/// 作成時刻が不明の候補は「開始以降」と言えないため絞り込めず、採用せず曖昧として数える。
pub fn match_fork(candidates: &[ForkCandidate], source_id: &str, since: UnixMillis) -> ForkMatch {
    let from_source: Vec<&ForkCandidate> = candidates.iter().filter(|c| c.forked_from.as_deref() == Some(source_id)).collect();
    let after: Vec<&ForkCandidate> = from_source.iter().copied().filter(|c| c.created_at.is_some_and(|t| t.0 >= since.0 - SINCE_SLACK_MS)).collect();
    let unknown_time = from_source.iter().filter(|c| c.created_at.is_none()).count();
    match (after.len(), unknown_time) {
        (1, 0) => ForkMatch::One(after[0].chat.clone()),
        (0, 0) => ForkMatch::None,
        (n, u) => ForkMatch::Ambiguous(n + u),
    }
}

// ───────────────────────────── 編集して再送・再生成（M50、DESIGN_P5 §5） ─────────────────────────────

/// 編集して再送・再生成の可否の判断材料。ホストが履歴とチャットの状態から作る。
#[derive(Debug, Clone, PartialEq)]
pub struct ResendCtx {
    /// 分岐の能力が使えないときの理由（使えるなら None）。
    pub capability_problem: Option<String>,
    pub connected: bool,
    pub delete_pending: bool,
    /// 外部で実行中の可能性（外部作成で live でない）。
    pub running_elsewhere: bool,
    pub stop_unconfirmed: bool,
    /// メイン・子孫に作業中・対応待ちがある、または送信の受理不明・結果未確認の操作がある。
    pub busy: bool,
    /// メインの状態が不明で、履歴で終端を確認できていない。
    pub root_state_unknown: bool,
    /// 履歴で、対象turnが `through_turn` の直後であることを確認できたか。
    pub target_follows_through: bool,
    /// 対象turnの依頼（ユーザー発話）の件数。2件以上は実行中の追加指示を含む。
    pub target_request_count: usize,
    /// `through_turn` の終端を履歴で確認できたか（`end === null` や turn ID なしは false）。
    pub through_end_known: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResendBlockKind {
    Capability,
    NotConnected,
    DeletePending,
    RunningElsewhere,
    StopUnconfirmed,
    Busy,
    StateUnknown,
    /// 表示が古い（対象turnが終点の直後でない）。
    Stale,
    MultipleRequests,
    PreviousNotEnded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResendBlock {
    pub kind: ResendBlockKind,
    pub message: String,
}

/// 無効の理由を、DESIGN_P5 §5 の順（1〜9）で最初に当たったものだけ返す。None なら実行してよい。
/// 表示が古い場合の照合（`target_follows_through`）は、履歴の内容に依る条件（追加指示・終点の終端）の前に調べる。
pub fn resend_blocker(c: &ResendCtx) -> Option<ResendBlock> {
    let b = |kind, m: &str| Some(ResendBlock { kind, message: m.to_string() });
    if let Some(reason) = &c.capability_problem {
        return Some(ResendBlock { kind: ResendBlockKind::Capability, message: reason.clone() });
    }
    if !c.connected {
        return b(ResendBlockKind::NotConnected, "Codex に接続していません");
    }
    if c.delete_pending {
        return b(ResendBlockKind::DeletePending, "削除保留中のチャットです");
    }
    if c.running_elsewhere {
        return b(ResendBlockKind::RunningElsewhere, "外部で実行中か確認できないため使えません（再開の案内から確認してください）");
    }
    if c.stop_unconfirmed {
        return b(ResendBlockKind::StopUnconfirmed, "停止を確認できていないため使えません");
    }
    if c.busy {
        return b(ResendBlockKind::Busy, "実行中は使えません。完了するか、中断と停止の確認の後に使えます");
    }
    if c.root_state_unknown {
        return b(ResendBlockKind::StateUnknown, "状態を確認できないため使えません（「状態を再照合」で確認できます）");
    }
    if !c.target_follows_through {
        return b(ResendBlockKind::Stale, "表示が古いため、会話を読み直してからやり直してください");
    }
    if c.target_request_count >= 2 {
        return b(ResendBlockKind::MultipleRequests, "追加指示を含むturnは、編集・再生成できません");
    }
    if !c.through_end_known {
        return b(ResendBlockKind::PreviousNotEnded, "直前のturnの終了を確認できていません");
    }
    None
}

/// 分岐を受け付けた後の次の一歩。分岐が受理不明・拒否ならここで止め、下書きも送信もしない（照合は読取りだけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResendNext {
    /// 分岐が受理されなかった。下書きも送信もしない。
    StopAfterFork,
    /// 下書きを保存できなかった。送らない。
    StopAfterDraft,
    /// 下書きを保存できたが、送信は求められていない（入力欄に入れるだけ）。
    DraftOnly,
    /// 分岐も下書きも済み、送信を1回だけ行う。
    SendOnce,
}

pub fn resend_next(fork_accepted: bool, draft_saved: bool, send_requested: bool) -> ResendNext {
    if !fork_accepted {
        ResendNext::StopAfterFork
    } else if !draft_saved {
        ResendNext::StopAfterDraft
    } else if send_requested {
        ResendNext::SendOnce
    } else {
        ResendNext::DraftOnly
    }
}

/// 送信後に下書きを消してよいか。受理が確認できたときだけ（拒否・受理不明は下書きを残す）。
pub fn clear_draft_after(state: &SendState) -> bool {
    matches!(state, SendState::Accepted { .. })
}

// ───────────────────────────── 受理不明の操作の照合 ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum OpObservation {
    /// 開始後のturnに、その操作の痕跡（レビュー開始・圧縮の記録）を観測した。
    Observed,
    /// 痕跡がない。履歴が完全で、十分な時間が経っているときだけ。再送の根拠にはしない。
    NotObserved,
    /// まだ判断できない（理由を表示する）。
    Undetermined { reason: String },
}

fn marker_of(op: ParityOp) -> Option<fn(&ActivityKind) -> bool> {
    match op {
        ParityOp::CodeReview | ParityOp::ReviewToNewChat => Some(|k| matches!(k, ActivityKind::ReviewStarted)),
        ParityOp::Compact => Some(|k| matches!(k, ActivityKind::ContextCompaction)),
        _ => None,
    }
}

/// 読んだ履歴から、受理不明の操作の痕跡を判定する。読取り専用の照合で、操作の再実行・再送には使わない。
/// `since` は操作を送る前に保存した時刻、`now` は現在時刻。
pub fn judge_op_observation(op: ParityOp, turns: &[TurnRecord], since: UnixMillis, now: UnixMillis) -> OpObservation {
    let Some(is_marker) = marker_of(op) else { return OpObservation::Undetermined { reason: "この操作は履歴から照合できません".into() } };
    let mut unattributable = false;
    for t in turns {
        if !t.entries.iter().any(|e| is_marker(&e.kind)) {
            continue;
        }
        match &t.started_at {
            Known::Value { value, .. } if value.0 >= since.0 - SINCE_SLACK_MS => return OpObservation::Observed,
            Known::Value { .. } => {}
            // 開始時刻が読めない turn にある痕跡は、今回の操作のものか分からない。
            _ => unattributable = true,
        }
    }
    if unattributable {
        return OpObservation::Undetermined { reason: "開始時刻を読めない履歴があり、今回の操作の痕跡か判断できません".into() };
    }
    let all_settled = !turns.is_empty() && turns.iter().all(|t| t.complete && t.end.is_some());
    let age = now.0 - since.0;
    if !all_settled {
        return OpObservation::Undetermined { reason: "履歴が完全に読めない、または実行中のturnがあり、痕跡がないとは言えません".into() };
    }
    if age < NOT_OBSERVED_MIN_AGE_MS {
        return OpObservation::Undetermined { reason: "送ってからの時間が短く、遅れて処理される可能性があります。しばらくしてから確認してください".into() };
    }
    OpObservation::NotObserved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ck(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }

    fn cand(id: &str, from: Option<&str>, at: Option<i64>) -> ForkCandidate {
        ForkCandidate { chat: ck(id), forked_from: from.map(str::to_string), created_at: at.map(UnixMillis) }
    }

    fn ok_ctx() -> ResendCtx {
        ResendCtx {
            capability_problem: None,
            connected: true,
            delete_pending: false,
            running_elsewhere: false,
            stop_unconfirmed: false,
            busy: false,
            root_state_unknown: false,
            target_follows_through: true,
            target_request_count: 1,
            through_end_known: true,
        }
    }

    #[test]
    fn resend_blocker_reports_each_reason_in_the_documented_order() {
        assert_eq!(resend_blocker(&ok_ctx()), None);
        let kind = |c: &ResendCtx| resend_blocker(c).map(|b| b.kind);
        let mut c = ok_ctx();
        c.capability_problem = Some("非対応".into());
        assert_eq!(kind(&c), Some(ResendBlockKind::Capability));
        c = ok_ctx();
        c.connected = false;
        assert_eq!(kind(&c), Some(ResendBlockKind::NotConnected));
        c = ok_ctx();
        c.delete_pending = true;
        assert_eq!(kind(&c), Some(ResendBlockKind::DeletePending));
        c = ok_ctx();
        c.running_elsewhere = true;
        assert_eq!(kind(&c), Some(ResendBlockKind::RunningElsewhere));
        c = ok_ctx();
        c.stop_unconfirmed = true;
        assert_eq!(kind(&c), Some(ResendBlockKind::StopUnconfirmed));
        c = ok_ctx();
        c.busy = true;
        assert_eq!(kind(&c), Some(ResendBlockKind::Busy));
        c = ok_ctx();
        c.root_state_unknown = true;
        assert_eq!(kind(&c), Some(ResendBlockKind::StateUnknown));
        c = ok_ctx();
        c.target_request_count = 2;
        assert_eq!(kind(&c), Some(ResendBlockKind::MultipleRequests));
        c = ok_ctx();
        c.through_end_known = false;
        assert_eq!(kind(&c), Some(ResendBlockKind::PreviousNotEnded));
        c = ok_ctx();
        c.target_follows_through = false;
        assert_eq!(kind(&c), Some(ResendBlockKind::Stale));
    }

    #[test]
    fn resend_blocker_priority_earlier_reasons_win() {
        let mut c = ok_ctx();
        c.connected = false;
        c.busy = true;
        c.target_request_count = 3;
        c.through_end_known = false;
        assert_eq!(resend_blocker(&c).unwrap().kind, ResendBlockKind::NotConnected);
        // 表示が古いときは、履歴の内容に依る理由（追加指示・終点未確認）より先に出す。
        let mut c = ok_ctx();
        c.target_follows_through = false;
        c.target_request_count = 2;
        c.through_end_known = false;
        assert_eq!(resend_blocker(&c).unwrap().kind, ResendBlockKind::Stale);
        // 追加指示は終点の未確認より先。
        let mut c = ok_ctx();
        c.target_request_count = 2;
        c.through_end_known = false;
        assert_eq!(resend_blocker(&c).unwrap().kind, ResendBlockKind::MultipleRequests);
    }

    #[test]
    fn resend_never_sends_after_an_unconfirmed_fork_or_a_failed_draft_save() {
        // 分岐が受理されていない（拒否・受理不明）: 下書きにも送信にも進まない。
        assert_eq!(resend_next(false, true, true), ResendNext::StopAfterFork);
        assert_eq!(resend_next(false, false, true), ResendNext::StopAfterFork);
        // 下書きを保存できなかったら送らない。
        assert_eq!(resend_next(true, false, true), ResendNext::StopAfterDraft);
        assert_eq!(resend_next(true, false, false), ResendNext::StopAfterDraft);
        // 送らない指定では入力欄に入れるだけ。
        assert_eq!(resend_next(true, true, false), ResendNext::DraftOnly);
        assert_eq!(resend_next(true, true, true), ResendNext::SendOnce);
    }

    #[test]
    fn draft_is_cleared_only_when_the_send_is_confirmed_accepted() {
        let turn = TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("c".into()) }, turn_id: ExternalId("t".into()) };
        assert!(clear_draft_after(&SendState::Accepted { turn }));
        assert!(!clear_draft_after(&SendState::AcceptanceUnknown { since: UnixMillis(1) }));
        assert!(!clear_draft_after(&SendState::Rejected { message: "x".into() }));
        assert!(!clear_draft_after(&SendState::Sending));
        assert!(!clear_draft_after(&SendState::NotFoundAfterReconcile));
    }

    #[test]
    fn fork_is_adopted_only_when_exactly_one_candidate_matches() {
        let since = UnixMillis(10_000);
        // 0件（別の会話の分岐・開始前に作られた分岐しかない）。
        let none = [cand("x", Some("other"), Some(20_000)), cand("old", Some("src"), Some(1_000))];
        assert_eq!(match_fork(&none, "src", since), ForkMatch::None);
        assert_eq!(match_fork(&[], "src", since), ForkMatch::None);
        // 1件。
        let one = [cand("x", Some("other"), Some(20_000)), cand("new", Some("src"), Some(10_500)), cand("old", Some("src"), Some(1_000))];
        assert_eq!(match_fork(&one, "src", since), ForkMatch::One(ck("new")));
        // 複数件は採用しない。
        let many = [cand("a", Some("src"), Some(10_500)), cand("b", Some("src"), Some(11_000))];
        assert_eq!(match_fork(&many, "src", since), ForkMatch::Ambiguous(2));
    }

    #[test]
    fn fork_with_unknown_creation_time_is_never_adopted() {
        let since = UnixMillis(10_000);
        // 時刻不明が1件だけでも採用しない（開始以降と言えない）。
        assert_eq!(match_fork(&[cand("a", Some("src"), None)], "src", since), ForkMatch::Ambiguous(1));
        // 確認できた1件があっても、時刻不明の候補が残るなら1件に絞れない。
        assert_eq!(match_fork(&[cand("a", Some("src"), Some(10_200)), cand("b", Some("src"), None)], "src", since), ForkMatch::Ambiguous(2));
        // 分岐元が示されていない候補は無関係。
        assert_eq!(match_fork(&[cand("a", None, Some(10_200))], "src", since), ForkMatch::None);
    }

    #[test]
    fn fork_allows_a_small_clock_slack_but_not_more() {
        let since = UnixMillis(10_000);
        assert_eq!(match_fork(&[cand("a", Some("src"), Some(8_500))], "src", since), ForkMatch::One(ck("a")));
        assert_eq!(match_fork(&[cand("a", Some("src"), Some(7_000))], "src", since), ForkMatch::None);
    }

    #[test]
    fn review_targets_other_than_custom_need_a_git_repository() {
        let branches = vec!["main".to_string(), "dev".to_string()];
        let base = |b: &str| ReviewTarget::BaseBranch { branch: b.into() };
        let commit = |s: &str| ReviewTarget::Commit { sha: s.into(), title: None };
        let custom = |i: &str| ReviewTarget::Custom { instructions: i.into() };
        // Gitなし: 指示だけ可。
        assert!(review_target_check(&ReviewTarget::UncommittedChanges, false, &[]).is_err());
        assert!(review_target_check(&base("main"), false, &[]).is_err());
        assert!(review_target_check(&commit("abcdef1"), false, &[]).is_err());
        assert!(review_target_check(&custom("見て"), false, &[]).is_ok());
        assert!(review_target_check(&custom("  "), true, &branches).is_err());
        // Gitあり。
        assert!(review_target_check(&ReviewTarget::UncommittedChanges, true, &branches).is_ok());
        assert!(review_target_check(&base("dev"), true, &branches).is_ok());
        assert!(review_target_check(&base("nope"), true, &branches).is_err());
        assert!(review_target_check(&commit("abcdef1234"), true, &branches).is_ok());
        assert!(review_target_check(&commit("--oops"), true, &branches).is_err());
        assert!(review_target_check(&commit("abc"), true, &branches).is_err());
    }

    #[test]
    fn git_listing_parsers_keep_only_well_formed_lines() {
        assert_eq!(parse_branch_lines("main

 dev 
"), vec!["main", "dev"]);
        let log = "0123456789abcdef0123456789abcdef01234567\tfix: a b
not-a-sha\tx
abcdef1\t
broken line without tab
";
        assert_eq!(parse_commit_lines(log), vec![("0123456789abcdef0123456789abcdef01234567".to_string(), "fix: a b".to_string()), ("abcdef1".to_string(), String::new())]);
    }

    fn turn(id: &str, start: Known<UnixMillis>, kinds: Vec<ActivityKind>, settled: bool) -> TurnRecord {
        let key = TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("c".into()) }, turn_id: ExternalId(id.into()) };
        TurnRecord {
            entries: kinds
                .into_iter()
                .enumerate()
                .map(|(i, kind)| TranscriptEntry {
                    key: ItemKey { agent: key.agent.clone(), turn_id: Some(key.turn_id.clone()), item_id: ExternalId(format!("i{i}")) },
                    kind,
                    text: Known::Missing,
                    phase: ActivityPhase::Completed,
                })
                .collect(),
            key,
            end: settled.then_some(TurnEnd::Completed),
            started_at: start,
            completed_at: Known::Missing,
            complete: settled,
        }
    }

    fn at(ms: i64) -> Known<UnixMillis> {
        Known::direct(UnixMillis(ms))
    }

    #[test]
    fn pending_op_is_observed_only_by_a_trace_in_a_turn_started_after_it() {
        let since = UnixMillis(100_000);
        let now = UnixMillis(100_000 + NOT_OBSERVED_MIN_AGE_MS + 1);
        let review = |s| turn("t2", s, vec![ActivityKind::UserMessage, ActivityKind::ReviewStarted], true);
        assert_eq!(judge_op_observation(ParityOp::CodeReview, &[review(at(100_500))], since, now), OpObservation::Observed);
        // 開始前のturnにある痕跡は今回のものではない。
        assert_eq!(judge_op_observation(ParityOp::CodeReview, &[review(at(5_000))], since, now), OpObservation::NotObserved);
        // 種類が違う痕跡は数えない。
        let compacted = turn("t3", at(100_500), vec![ActivityKind::ContextCompaction], true);
        assert_eq!(judge_op_observation(ParityOp::CodeReview, std::slice::from_ref(&compacted), since, now), OpObservation::NotObserved);
        assert_eq!(judge_op_observation(ParityOp::Compact, &[compacted], since, now), OpObservation::Observed);
    }

    #[test]
    fn pending_op_is_not_declared_absent_without_complete_history_and_enough_time() {
        let since = UnixMillis(100_000);
        let late = UnixMillis(100_000 + NOT_OBSERVED_MIN_AGE_MS + 1);
        let early = UnixMillis(100_000 + 5_000);
        let plain = |settled| turn("t1", at(5_000), vec![ActivityKind::UserMessage], settled);
        // 早すぎる。
        assert!(matches!(judge_op_observation(ParityOp::Compact, &[plain(true)], since, early), OpObservation::Undetermined { .. }));
        // 履歴が未完了・実行中のturnがある。
        assert!(matches!(judge_op_observation(ParityOp::Compact, &[plain(false)], since, late), OpObservation::Undetermined { .. }));
        // turnが1件も読めていない。
        assert!(matches!(judge_op_observation(ParityOp::Compact, &[], since, late), OpObservation::Undetermined { .. }));
        // 開始時刻が読めないturnに痕跡がある。
        let unknown = turn("t4", Known::Missing, vec![ActivityKind::ContextCompaction], true);
        assert!(matches!(judge_op_observation(ParityOp::Compact, &[unknown], since, late), OpObservation::Undetermined { .. }));
        // 照合できない操作。
        assert!(matches!(judge_op_observation(ParityOp::Goal, &[plain(true)], since, late), OpObservation::Undetermined { .. }));
    }
}

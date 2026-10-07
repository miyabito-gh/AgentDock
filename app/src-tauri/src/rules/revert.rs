//! 「変更を戻す」の計画（P3-1、`app/DESIGN_P3.md` §1 #2）。純粋ロジック。
//!
//! 戻せるのは、AgentDockが観測した変更の差分を逆適用でき、現在の内容が観測直後と一致するファイルだけ。
//! 次のどれかに当たれば止めて理由を返す（推定で戻さない・部分的な逆適用で済ませない）:
//! - 観測した記録がない／別の会話の、より新しい記録がある
//! - 現在の内容のハッシュが、最新の記録の観測直後と違う（別の変更の可能性）
//! - 逆適用の途中の内容が、一つ前の記録の観測直後と合わない（観測していない変更が挟まっている）
//! - 差分の文脈が現在の内容と完全に一致しない／テキストとして読めない／戻し先に別のファイルがある
//!
//! 現在のファイルの読取りは呼び出し側の関数で渡す（ここでI/Oをしない）。

use sha2::{Digest, Sha256};

use super::diff::{apply_reverse, deleted_content, looks_like_unified, parse_hunks};
use crate::backend::changes::*;
use crate::backend::model::*;

/// バイト列のSHA-256（16進小文字）。観測直後のハッシュと、戻す前の照合で同じ関数を使う。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 現在のファイルの読取り結果。`Ok(None)` はファイルが無い。
pub type ReadResult = Result<Option<Vec<u8>>, String>;

/// 戻すときに行う書込み。`content` が `None` ならそのファイルを削除する（無ければ何もしない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub path: String,
    pub content: Option<Vec<u8>>,
    /// 計画時の現在の内容のSHA-256（`None` は存在しない）。書換え直前に照合する。
    pub expect: Option<String>,
}

/// 書換え直前の内容が、計画時の内容と同じか。
pub fn matches_expect(expect: &Option<String>, current: &Option<Vec<u8>>) -> bool {
    match (expect, current) {
        (None, None) => true,
        (Some(h), Some(b)) => &sha256_hex(b) == h,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub code: RevertBlockCode,
    pub message: String,
}

fn block<T>(code: RevertBlockCode, message: impl Into<String>) -> Result<T, Block> {
    Err(Block { code, message: message.into() })
}

/// 1ファイル分の計画。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// 現在のファイルの場所（変更後のパス）。
    pub path: String,
    pub kind: Option<ChangeKind>,
    pub includes_turns: Vec<ExternalId>,
    /// 戻すと消化される記録（計画の連鎖に入ったもの）。
    pub records: Vec<RecordId>,
    pub outcome: Result<Vec<Write>, Block>,
}

/// 戻す対象の選び方。
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// そのturnの変更から戻す（None＝このチャットの観測した変更すべて）。
    pub turn: Option<ExternalId>,
    /// 対象のファイル（None＝該当するすべて）。
    pub paths: Option<Vec<String>>,
}

/// 「戻し」の記録（`ChangeLine::Reverted`）。
#[derive(Debug, Clone)]
pub struct RevertMark {
    pub at: UnixMillis,
    pub paths: Vec<String>,
    /// 識別子つきの記録。`None`（旧形式）は時刻＋パスで判定する。
    pub records: Option<Vec<RecordId>>,
}

/// パスの比較用の形（Windows: 区切りと大文字小文字を区別しない）。
pub fn norm_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// 書き換える場所として安全か。`..` を含むパスは字句で止め、その上で解決後の位置が作業フォルダの内側かを確かめる。
pub fn check_inside(p: &str, confine: Confine) -> Result<(), Block> {
    if std::path::Path::new(p).components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return block(RevertBlockCode::PathOutside, format!("{p}: パスに `..` を含むため、戻せません"));
    }
    confine(p).or_else(|m| block(RevertBlockCode::PathOutside, format!("{p}: 作業フォルダの内側であることを確認できないため、戻せません（{m}）")))
}

fn is_absolute(p: &str) -> bool {
    std::path::Path::new(p).is_absolute()
}

enum State {
    Absent,
    Content(Vec<u8>),
}

fn state_of(r: ReadResult) -> Result<State, Block> {
    match r {
        Ok(None) => Ok(State::Absent),
        Ok(Some(b)) => Ok(State::Content(b)),
        Err(m) => block(RevertBlockCode::ReadFailed, format!("ファイルを読めません: {m}")),
    }
}

/// 状態が記録の観測直後と一致するか確かめる。
fn verify_post(state: &State, post: &PostState, path: &str) -> Result<(), Block> {
    match (post, state) {
        (PostState::Unknown, _) => block(RevertBlockCode::HashUnknown, format!("{path}: 観測直後の内容を確認できていないため、戻せません")),
        (PostState::Absent, State::Absent) => Ok(()),
        (PostState::Absent, State::Content(_)) => block(RevertBlockCode::HashMismatch, format!("{path}: 削除されたはずのファイルが現在あります（別の変更の可能性）")),
        (PostState::Hash { .. }, State::Absent) => block(RevertBlockCode::MissingNow, format!("{path}: 記録ではファイルがあるはずですが、現在は存在しません")),
        (PostState::Hash { sha256 }, State::Content(b)) => {
            if &sha256_hex(b) == sha256 {
                Ok(())
            } else {
                block(RevertBlockCode::HashMismatch, format!("{path}: 現在の内容が観測直後と一致しません（別の会話・エディタ・コマンドで変更された可能性）"))
            }
        }
    }
}

fn text_of(bytes: &[u8], path: &str) -> Result<String, Block> {
    String::from_utf8(bytes.to_vec()).or_else(|_| block(RevertBlockCode::NotText, format!("{path}: テキストとして読めないファイルは、差分で戻せません")))
}

/// 記録1件を逆向きに適用した後の状態。
fn reverse_one(rec: &ChangeRecord, state: &State) -> Result<State, Block> {
    let path = rec.post_path();
    match rec.kind {
        ChangeKind::Added => Ok(State::Absent),
        ChangeKind::Deleted => match deleted_content(&rec.diff) {
            Ok(c) => Ok(State::Content(c.into_bytes())),
            Err(m) => block(RevertBlockCode::DiffUnreadable, format!("{path}: 削除前の内容を復元できません（{m}）")),
        },
        ChangeKind::Modified => {
            let State::Content(bytes) = state else {
                return block(RevertBlockCode::MissingNow, format!("{path}: 現在のファイルがありません"));
            };
            // 内容の変更がない移動（差分が空）。
            if rec.move_to.is_some() && rec.diff.trim().is_empty() {
                return Ok(State::Content(bytes.clone()));
            }
            let text = text_of(bytes, path)?;
            if !looks_like_unified(&rec.diff) {
                return block(RevertBlockCode::DiffUnreadable, format!("{path}: 記録した差分が統一diffではないため、逆適用できません"));
            }
            let hunks = parse_hunks(&rec.diff).or_else(|m| block(RevertBlockCode::DiffUnreadable, format!("{path}: {m}")))?;
            match apply_reverse(&text, &hunks) {
                Ok(t) => Ok(State::Content(t.into_bytes())),
                Err(m) => block(RevertBlockCode::ContextMismatch, format!("{path}: {m}")),
            }
        }
    }
}

/// 記録の識別子。
pub fn record_id(rec: &ChangeRecord) -> RecordId {
    RecordId { chat: rec.chat.clone(), item: rec.item.clone(), path: rec.path.clone() }
}

/// 「戻し」で消化済みの記録か。識別子つきの戻しは、その記録だけを消化する。
/// 旧形式（識別子なし）は、戻した時刻以前に観測され、そのパスを戻している記録を消化済みとみなす（従来の判定）。
pub fn is_consumed(rec: &ChangeRecord, marks: &[RevertMark]) -> bool {
    let (a, b) = (norm_path(&rec.path), norm_path(rec.post_path()));
    marks.iter().any(|m| match &m.records {
        Some(ids) => ids.iter().any(|r| r.chat == rec.chat && r.item == rec.item && norm_path(&r.path) == a),
        None => m.at >= rec.observed_at && m.paths.iter().any(|p| {
            let n = norm_path(p);
            n == a || n == b
        }),
    })
}

/// 戻す計画を作る。`records` は全チャットの観測記録（観測の順）、`marks` は「戻し」の記録、`read` は現在のファイルの読取り。
/// 戻り値は対象ファイルごとの結果。対象がなければ空（`paths` で指定したのに記録がないものは `NotObserved` で返す）。
///
/// 作業フォルダの内側かどうかの検査は行わない（テスト・内側が自明な呼び出し用）。ホストは [`plan_confined`] を使う。
pub fn plan(chat: &ChatKey, records: &[ChangeRecord], marks: &[RevertMark], sel: &Selection, read: &dyn Fn(&str) -> ReadResult) -> Vec<Planned> {
    plan_confined(chat, records, marks, sel, read, &|_| Ok(()))
}

/// パスが作業フォルダの内側か。解決（親のcanonicalize・リンク越え）した結果で判定し、確認できなければ `Err(理由)`。
pub type Confine<'a> = &'a dyn Fn(&str) -> Result<(), String>;

/// [`plan`] に、書き換える全パス（移動元を含む）の作業フォルダ内検査を加えたもの。
pub fn plan_confined(chat: &ChatKey, records: &[ChangeRecord], marks: &[RevertMark], sel: &Selection, read: &dyn Fn(&str) -> ReadResult, confine: Confine) -> Vec<Planned> {
    let live: Vec<&ChangeRecord> = records.iter().filter(|r| !is_consumed(r, marks)).collect();
    let want = |p: &str| sel.paths.as_ref().is_none_or(|ps| ps.iter().any(|x| norm_path(x) == norm_path(p)));
    let selected = |r: &ChangeRecord| &r.chat == chat && sel.turn.as_ref().is_none_or(|t| r.turn.as_ref() == Some(t));

    // 候補: このチャットの（選択に合う）記録の変更後のパス。重複は1つにまとめる。
    let mut candidates: Vec<String> = Vec::new();
    for r in live.iter().filter(|r| selected(r) && want(r.post_path())) {
        if !candidates.iter().any(|c| norm_path(c) == norm_path(r.post_path())) {
            candidates.push(r.post_path().to_string());
        }
    }
    let mut out: Vec<Planned> = candidates.iter().map(|p| plan_path(chat, &live, sel, p, read, confine)).collect();
    // 指定されたが記録のないファイル。
    if let Some(ps) = &sel.paths {
        for p in ps {
            if !out.iter().any(|o| norm_path(&o.path) == norm_path(p)) {
                out.push(Planned {
                    path: p.clone(),
                    kind: None,
                    includes_turns: vec![],
                    records: vec![],
                    outcome: block(RevertBlockCode::NotObserved, "このチャットの変更として、AgentDockが観測した記録がありません（再起動中・外部・コマンドによる変更は戻せません）"),
                });
            }
        }
    }
    out
}

fn plan_path(chat: &ChatKey, live: &[&ChangeRecord], sel: &Selection, p: &str, read: &dyn Fn(&str) -> ReadResult, confine: Confine) -> Planned {
    let np = norm_path(p);
    let touching: Vec<&ChangeRecord> = live.iter().copied().filter(|r| norm_path(r.post_path()) == np || norm_path(&r.path) == np).collect();
    let start = touching
        .iter()
        .position(|r| &r.chat == chat && sel.turn.as_ref().is_none_or(|t| r.turn.as_ref() == Some(t)))
        .unwrap_or(0);
    let chain = &touching[start..];
    let kind = chain.first().map(|r| r.kind);
    let mut includes_turns: Vec<ExternalId> = Vec::new();
    for r in chain {
        if let Some(t) = &r.turn {
            if sel.turn.as_ref() != Some(t) && !includes_turns.contains(t) {
                includes_turns.push(t.clone());
            }
        }
    }
    let outcome = plan_chain(chat, chain, p, read, confine);
    let records = chain.iter().map(|r| record_id(r)).collect();
    Planned { path: p.to_string(), kind, includes_turns, records, outcome }
}

fn plan_chain(chat: &ChatKey, chain: &[&ChangeRecord], p: &str, read: &dyn Fn(&str) -> ReadResult, confine: Confine) -> Result<Vec<Write>, Block> {
    let np = norm_path(p);
    if chain.iter().any(|r| &r.chat != chat) {
        return block(RevertBlockCode::OtherChatLater, format!("{p}: 別の会話が、このファイルをより後で変更しています。その変更を先に戻すか、手動で確認してください"));
    }
    if !is_absolute(p) {
        return block(RevertBlockCode::PathUnresolved, format!("{p}: 作業フォルダが分からず、場所を特定できません"));
    }
    check_inside(p, confine)?;
    for (i, r) in chain.iter().enumerate() {
        // 移動して去った記録、または2件目以降の移動は、この版では扱わない。
        let moved_away = norm_path(&r.path) == np && norm_path(r.post_path()) != np;
        let moved_in = r.move_to.is_some() && norm_path(&r.path) != np;
        if moved_away || (moved_in && i != 0) {
            return block(RevertBlockCode::Unsupported, format!("{p}: 移動を含む複数回の変更は、まとめて戻せません"));
        }
        if moved_in && !is_absolute(&r.path) {
            return block(RevertBlockCode::PathUnresolved, format!("{}: 移動元の場所を特定できません", r.path));
        }
        if moved_in {
            check_inside(&r.path, confine)?;
        }
    }
    let mut state = state_of(read(p))?;
    let before = match &state {
        State::Absent => None,
        State::Content(b) => Some(sha256_hex(b)),
    };
    for r in chain.iter().rev() {
        verify_post(&state, &r.post, p)?;
        state = reverse_one(r, &state)?;
    }
    let first = chain.first().expect("candidate has at least one record");
    let content = match state {
        State::Absent => None,
        State::Content(b) => Some(b),
    };
    if first.move_to.is_some() && norm_path(&first.path) != np {
        // 移動して来たファイル: 元の場所へ内容を戻し、現在の場所を消す。元の場所に別のファイルがあれば上書きしない。
        if state_of(read(&first.path))?.is_present() {
            return block(RevertBlockCode::TargetExists, format!("{}: 戻し先に別のファイルがあるため、上書きしません", first.path));
        }
        let Some(c) = content else { return block(RevertBlockCode::Unsupported, format!("{p}: 移動元の内容を復元できません")) };
        // 元の場所は不在を確認済み（上の `is_present`）。
        return Ok(vec![Write { path: first.path.clone(), content: Some(c), expect: None }, Write { path: p.to_string(), content: None, expect: before }]);
    }
    Ok(vec![Write { path: p.to_string(), content, expect: before }])
}

impl State {
    fn is_present(&self) -> bool {
        matches!(self, State::Content(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn chat(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }

    #[allow(clippy::too_many_arguments)]
    fn rec(c: &str, turn: &str, at: i64, path: &str, kind: ChangeKind, move_to: Option<&str>, diff: &str, post: PostState) -> ChangeRecord {
        ChangeRecord {
            schema_version: 1,
            chat: chat(c),
            agent: AgentKey { backend: BackendKind::Codex, id: ExternalId(c.into()) },
            turn: Some(ExternalId(turn.into())),
            item: ExternalId(format!("i{at}")),
            path: path.into(),
            kind,
            move_to: move_to.map(str::to_string),
            diff: diff.into(),
            post,
            observed_at: UnixMillis(at),
        }
    }

    fn h(s: &str) -> PostState {
        PostState::Hash { sha256: sha256_hex(s.as_bytes()) }
    }

    fn fs(files: &[(&str, &str)]) -> impl Fn(&str) -> ReadResult {
        let m: HashMap<String, Vec<u8>> = files.iter().map(|(p, c)| (norm_path(p), c.as_bytes().to_vec())).collect();
        move |p: &str| Ok(m.get(&norm_path(p)).cloned())
    }

    const A: &str = r"C:\w\a.txt";
    const D1: &str = "@@ -1,2 +1,2 @@\n keep\n-one\n+two\n";
    const D2: &str = "@@ -1,2 +1,2 @@\n keep\n-two\n+three\n";

    fn only(mut v: Vec<Planned>) -> Planned {
        assert_eq!(v.len(), 1, "{v:?}");
        v.remove(0)
    }

    fn write_of(p: &Planned) -> Vec<(String, Option<String>)> {
        p.outcome.as_ref().unwrap().iter().map(|w| (w.path.clone(), w.content.as_ref().map(|c| String::from_utf8(c.clone()).unwrap()))).collect()
    }

    #[test]
    fn single_update_is_reverted_when_hash_matches() {
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n"))];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\ntwo\n")])));
        assert_eq!(write_of(&p), vec![(A.to_string(), Some("keep\none\n".to_string()))]);
        assert!(p.includes_turns.iter().all(|t| t.0 == "t1"));
    }

    #[test]
    fn hash_mismatch_blocks() {
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n"))];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\ntwo EDITED\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::HashMismatch);
    }

    #[test]
    fn unknown_post_hash_blocks_and_missing_file_is_reported() {
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, PostState::Unknown)];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\ntwo\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::HashUnknown);
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n"))];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::MissingNow);
    }

    #[test]
    fn context_mismatch_blocks_even_when_hash_is_trusted() {
        // 記録の差分は現在の内容に当たらない（記録と実ファイルのずれ）。
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, "@@ -1,2 +1,2 @@\n other\n-one\n+two\n", h("keep\ntwo\n"))];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\ntwo\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::ContextMismatch);
    }

    #[test]
    fn later_record_from_another_chat_blocks() {
        let r = vec![
            rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n")),
            rec("c2", "t9", 20, A, ChangeKind::Modified, None, D2, h("keep\nthree\n")),
        ];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\nthree\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::OtherChatLater);
        // 別チャットの記録が先（古い）なら、このチャットの変更は戻せる。
        let r2 = vec![
            rec("c2", "t9", 5, A, ChangeKind::Modified, None, "@@ -1,2 +1,2 @@\n keep\n-zero\n+one\n", h("keep\none\n")),
            rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n")),
        ];
        let p = only(plan(&chat("c1"), &r2, &[], &Selection::default(), &fs(&[(A, "keep\ntwo\n")])));
        assert_eq!(write_of(&p), vec![(A.to_string(), Some("keep\none\n".to_string()))]);
    }

    #[test]
    fn later_turns_of_the_same_chat_are_reverted_together_newest_first_and_listed() {
        let r = vec![
            rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n")),
            rec("c1", "t2", 20, A, ChangeKind::Modified, None, D2, h("keep\nthree\n")),
        ];
        let sel = Selection { turn: Some(ExternalId("t1".into())), paths: None };
        let p = only(plan(&chat("c1"), &r, &[], &sel, &fs(&[(A, "keep\nthree\n")])));
        assert_eq!(write_of(&p), vec![(A.to_string(), Some("keep\none\n".to_string()))]);
        assert_eq!(p.includes_turns, vec![ExternalId("t2".into())]);
        // t2 だけを選べば、t2 の変更だけ戻る。
        let sel2 = Selection { turn: Some(ExternalId("t2".into())), paths: None };
        let p2 = only(plan(&chat("c1"), &r, &[], &sel2, &fs(&[(A, "keep\nthree\n")])));
        assert_eq!(write_of(&p2), vec![(A.to_string(), Some("keep\ntwo\n".to_string()))]);
        assert!(p2.includes_turns.is_empty());
    }

    #[test]
    fn unobserved_change_between_records_blocks() {
        // 2件目を戻した結果が、1件目の観測直後と一致しない（間に観測していない変更がある）。
        let r = vec![
            rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\nSOMETHING ELSE\n")),
            rec("c1", "t2", 20, A, ChangeKind::Modified, None, D2, h("keep\nthree\n")),
        ];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\nthree\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::HashMismatch);
    }

    #[test]
    fn added_file_is_deleted_and_deleted_file_is_recreated() {
        let add = vec![rec("c1", "t1", 10, A, ChangeKind::Added, None, "hello\n", h("hello\n"))];
        let p = only(plan(&chat("c1"), &add, &[], &Selection::default(), &fs(&[(A, "hello\n")])));
        assert_eq!(write_of(&p), vec![(A.to_string(), None)]);
        let del = vec![rec("c1", "t1", 10, A, ChangeKind::Deleted, None, "bye\nbye2\n", PostState::Absent)];
        let p = only(plan(&chat("c1"), &del, &[], &Selection::default(), &fs(&[])));
        assert_eq!(write_of(&p), vec![(A.to_string(), Some("bye\nbye2\n".to_string()))]);
        // 削除したはずのファイルが戻ってきている → 止める
        let p = only(plan(&chat("c1"), &del, &[], &Selection::default(), &fs(&[(A, "came back\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::HashMismatch);
    }

    #[test]
    fn moved_file_is_moved_back_and_never_overwrites_the_source() {
        let src = r"C:\w\old.txt";
        let dst = r"C:\w\new.txt";
        let r = vec![rec("c1", "t1", 10, src, ChangeKind::Modified, Some(dst), "@@ -1,2 +1,2 @@\n keep\n-one\n+two\n", h("keep\ntwo\n"))];
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(dst, "keep\ntwo\n")])));
        assert_eq!(p.path, dst);
        assert_eq!(write_of(&p), vec![(src.to_string(), Some("keep\none\n".to_string())), (dst.to_string(), None)]);
        let p = only(plan(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(dst, "keep\ntwo\n"), (src, "someone else's\n")])));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::TargetExists);
        // 内容を変えない移動（差分なし）
        let pure = vec![rec("c1", "t1", 10, src, ChangeKind::Modified, Some(dst), "", h("same\n"))];
        let p = only(plan(&chat("c1"), &pure, &[], &Selection::default(), &fs(&[(dst, "same\n")])));
        assert_eq!(write_of(&p), vec![(src.to_string(), Some("same\n".to_string())), (dst.to_string(), None)]);
    }

    #[test]
    fn unobserved_path_is_reported_and_reverted_marks_consume_records() {
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n"))];
        let sel = Selection { turn: None, paths: Some(vec![r"C:\w\other.txt".into()]) };
        let v = plan(&chat("c1"), &r, &[], &sel, &fs(&[]));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].outcome.clone().unwrap_err().code, RevertBlockCode::NotObserved);
        // 戻し済みの記録は、以後の判定に使わない。
        let marks = vec![RevertMark { at: UnixMillis(15), paths: vec![A.into()], records: None }];
        assert!(plan(&chat("c1"), &r, &marks, &Selection::default(), &fs(&[(A, "keep\none\n")])).is_empty());
        // 戻し後に別チャットが変更した記録は有効のまま。
        let r2 = vec![r[0].clone(), rec("c2", "t9", 20, A, ChangeKind::Modified, None, D2, h("x"))];
        assert_eq!(plan(&chat("c2"), &r2, &marks, &Selection::default(), &fs(&[(A, "x")])).len(), 1);
    }

    #[test]
    fn relative_and_binary_and_unreadable_are_blocked_with_reasons() {
        let rel = vec![rec("c1", "t1", 10, "a.txt", ChangeKind::Modified, None, D1, h("x"))];
        assert_eq!(only(plan(&chat("c1"), &rel, &[], &Selection::default(), &fs(&[]))).outcome.unwrap_err().code, RevertBlockCode::PathUnresolved);
        let bin = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, PostState::Hash { sha256: sha256_hex(&[0xff, 0xfe, 0x00]) })];
        let m: HashMap<String, Vec<u8>> = HashMap::from([(norm_path(A), vec![0xff, 0xfe, 0x00])]);
        let read = move |p: &str| -> ReadResult { Ok(m.get(&norm_path(p)).cloned()) };
        assert_eq!(only(plan(&chat("c1"), &bin, &[], &Selection::default(), &read)).outcome.unwrap_err().code, RevertBlockCode::NotText);
        let fail = |_: &str| -> ReadResult { Err("denied".into()) };
        assert_eq!(only(plan(&chat("c1"), &bin, &[], &Selection::default(), &fail)).outcome.unwrap_err().code, RevertBlockCode::ReadFailed);
    }

    /// 作業フォルダ `C:\w` の内側だけを許す検査（字句。実機ではホストが解決後のパスで検査する）。
    fn inside_w(p: &str) -> Result<(), String> {
        if norm_path(p).starts_with(r"c:\w\") { Ok(()) } else { Err("作業フォルダの外".into()) }
    }

    #[test]
    fn paths_outside_the_work_folder_or_with_dotdot_are_blocked() {
        let run = |path: &str, move_to: Option<&str>| {
            let r = vec![rec("c1", "t1", 10, path, ChangeKind::Modified, move_to, D1, h("keep\ntwo\n"))];
            let dst = move_to.unwrap_or(path).to_string();
            only(plan_confined(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(&dst, "keep\ntwo\n")]), &inside_w))
        };
        assert!(run(A, None).outcome.is_ok());
        assert_eq!(run(r"C:\w\..\secret.txt", None).outcome.unwrap_err().code, RevertBlockCode::PathOutside);
        assert_eq!(run(r"C:\other\a.txt", None).outcome.unwrap_err().code, RevertBlockCode::PathOutside);
        assert_eq!(run(r"D:\w\a.txt", None).outcome.unwrap_err().code, RevertBlockCode::PathOutside);
        // 移動元が外（移動先は内側）でも止める。
        assert_eq!(run(r"C:\other\old.txt", Some(r"C:\w\new.txt")).outcome.unwrap_err().code, RevertBlockCode::PathOutside);
        // 解決に失敗した場合（リンク越え・確認不能）も止める。
        let r = vec![rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep\ntwo\n"))];
        let deny = |_: &str| -> Result<(), String> { Err("リンクの先が作業フォルダの外".into()) };
        let p = only(plan_confined(&chat("c1"), &r, &[], &Selection::default(), &fs(&[(A, "keep\ntwo\n")]), &deny));
        assert_eq!(p.outcome.unwrap_err().code, RevertBlockCode::PathOutside);
    }

    #[test]
    fn identified_revert_consumes_only_the_reverted_records_and_legacy_falls_back() {
        let r = vec![
            rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep
two
")),
            rec("c1", "t2", 20, A, ChangeKind::Modified, None, D2, h("keep
three
")),
        ];
        // t2 だけ戻した記録: t1 は消化されない。
        let marks = vec![RevertMark { at: UnixMillis(30), paths: vec![A.into()], records: Some(vec![record_id(&r[1])]) }];
        assert!(!is_consumed(&r[0], &marks));
        assert!(is_consumed(&r[1], &marks));
        let p = only(plan(&chat("c1"), &r, &marks, &Selection::default(), &fs(&[(A, "keep
two
")])));
        assert_eq!(write_of(&p), vec![(A.to_string(), Some("keep
one
".to_string()))]);
        // 別チャットの記録は、同じパス・時刻でも消化されない。
        let other = rec("c2", "t9", 5, A, ChangeKind::Modified, None, D2, h("x"));
        assert!(!is_consumed(&other, &marks));
        // 旧形式（識別子なし）は時刻＋パスの判定。
        let legacy = vec![RevertMark { at: UnixMillis(30), paths: vec![A.into()], records: None }];
        assert!(is_consumed(&r[0], &legacy) && is_consumed(&r[1], &legacy));
        // 識別子ありで空なら何も消化しない。
        let empty = vec![RevertMark { at: UnixMillis(30), paths: vec![A.into()], records: Some(vec![]) }];
        assert!(!is_consumed(&r[0], &empty));
    }

    #[test]
    fn plan_records_the_chain_and_write_expects_the_planned_content() {
        let r = vec![
            rec("c1", "t1", 10, A, ChangeKind::Modified, None, D1, h("keep
two
")),
            rec("c1", "t2", 20, A, ChangeKind::Modified, None, D2, h("keep
three
")),
        ];
        let sel = Selection { turn: Some(ExternalId("t2".into())), paths: None };
        let p = only(plan(&chat("c1"), &r, &[], &sel, &fs(&[(A, "keep
three
")])));
        assert_eq!(p.records, vec![record_id(&r[1])]);
        let w = &p.outcome.unwrap()[0];
        assert!(matches_expect(&w.expect, &Some(b"keep
three
".to_vec())));
        // 計画後に内容が変わった・消えた場合は一致しない。
        assert!(!matches_expect(&w.expect, &Some(b"keep
three EDITED
".to_vec())));
        assert!(!matches_expect(&w.expect, &None));
        assert!(matches_expect(&None, &None));
        assert!(!matches_expect(&None, &Some(vec![])));
    }

    #[test]
    fn sha256_matches_the_known_vector() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}

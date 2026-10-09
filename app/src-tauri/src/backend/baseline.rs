//! 変更の控え（Git基準）の中立型（P3B-1、`app/DESIGN_P3B.md` §2・§7）。
//!
//! - 控えは `chats\<dirId>\baselines\segments.jsonl` に区間（送信1件で始まるturnとその子孫の作業）ごとに記録する。
//! - oid・ブランチ名・木IDは不透明な文字列として扱う（Rust側でgitのoidを計算しない）。パスはリポジトリ相対（`/` 区切り）。
//! - Codex固有の型・メソッド名を含めない。判定ロジックは `rules::baseline`（P3B-2）、取得は `host::baseline`（P3B-3）。

use serde::{Deserialize, Serialize};

use super::changes::{RevertBlockCode, RevertFailure};
use super::model::*;

/// `segments.jsonl` の各行が持つ版。読む側は、これより新しい版の行を推測で解釈せず飛ばす。
pub const BASELINE_SCHEMA_VERSION: u32 = 1;

/// 控えの取得にかける時間の上限（status・ハッシュ・木作成の合計）。超えたら打ち切って `Timeout` として記録する。
pub const BASELINE_TIME_LIMIT_MS: u64 = 10_000;

/// 1ファイルの控えの上限（超えたら控えない）。
pub const BASELINE_FILE_LIMIT: u64 = 64 * 1024 * 1024;

/// 1回の控えで生バイトとして取り込む合計の上限（超えた分は控えない）。
pub const BASELINE_BUDGET: u64 = 512 * 1024 * 1024;

// ───────────────────────────── スナップショット ─────────────────────────────

/// リポジトリの場所。`root` は作業ツリーのルート、`objects_dir` は共通のobjects置き場（参照のみ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    pub root: String,
    pub objects_dir: String,
}

/// 控えなかった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// シンボリックリンク・ジャンクション。
    Link,
    Submodule,
    /// 1ファイルの上限（64MiB）を超えた。
    TooLarge,
    /// 1回の控えの合計上限（512MiB）を超えた分。
    Budget,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedPath {
    pub path: String,
    pub reason: SkipReason,
}

/// 木エントリの形。`Head` はHEADから来たもの（クリーン形。取り出しは `cat-file --filters`）、`Raw` は生バイトで入れたもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryForm {
    Head,
    Raw,
}

/// 木の中のファイル1つ。比較は `(oid, form)` の組で行い、形が違えば内容が同じでも「違う」とする（安全側）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRef {
    pub oid: String,
    pub form: EntryForm,
}

/// 控え（スナップショット）1つ。`tree` は専用objects置き場にある「上書き木」、`raw` は生形で入れたパスの集合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// HEADのコミット（初回コミット前は None）。
    pub head: Option<String>,
    /// ブランチ名（detachedは None）。
    pub branch: Option<String>,
    pub tree: String,
    pub raw: Vec<String>,
    pub skipped: Vec<SkippedPath>,
    pub took_ms: u64,
}

/// 区間の終了の控え。次の送信が子孫の作業中に来たときは、次の区間の基準をそのまま終了とする。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SegmentEnd {
    Snapshot { snapshot: Snapshot },
    EndIsNextBase,
}

// ───────────────────────────── 失敗 ─────────────────────────────

/// 控えを取れなかった理由（保存・UI共通）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BaselineFailure {
    NotARepository,
    GitUnavailable,
    /// Gitが古い（`--path-format=absolute` を使える2.31以上が要る）。`found` は見つかった版。
    GitTooOld { found: String },
    WorkFolderUnknown,
    Timeout,
    InsufficientSpace {
        #[cfg_attr(test, ts(type = "number"))]
        required: u64,
        #[cfg_attr(test, ts(type = "number"))]
        available: u64,
    },
    GitBusy,
    /// 設定で無効（警告は出さない）。
    Disabled,
    Git { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BaselinePhase {
    Base,
    End,
}

/// 同時作業の相手。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConcurrentWith {
    Chat { chat: ChatKey },
    /// AgentDock外（VS Code等）で実行中の会話。
    External { label: String },
}

// ───────────────────────────── 戻しの記録 ─────────────────────────────

/// 戻した先の内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Restored {
    Entry { entry: EntryRef },
    /// 控えの時点でファイルがなかったため、削除した。
    Absent,
}

/// 戻したファイル1件（成功したものだけを記録する）。`forced` は2段目の確認で強制したもの、`overridden` はそのとき無視した理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertedPath {
    pub path: String,
    pub restored: Restored,
    /// 書き換え前の内容の控え（`revert-backup` 内のファイル名）。元がなかった場合は None。
    pub backup_file: Option<String>,
    #[serde(default)]
    pub forced: bool,
    #[serde(default)]
    pub overridden: Vec<RevertBlockCode>,
}

// ───────────────────────────── segments.jsonl ─────────────────────────────

/// `segments.jsonl` の1行の中身。末尾の欠けた行は読込み時に無視する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BaselineLine {
    /// 送信の直前に基準 B を取れた。
    Started { seg: LocalId, at: UnixMillis, attempt: LocalId, repo: RepoRef, cwd: String, base: Snapshot },
    /// 受理でturnと対応付いた。
    Bound { seg: LocalId, turn: ExternalId },
    /// 終了の控え E（または次の基準で代える）。
    Ended { seg: LocalId, at: UnixMillis, end: SegmentEnd },
    /// 送信が受理なしで確定した（区間から外す）。
    Abandoned { seg: LocalId, reason: String },
    /// 控えを取れなかった。
    Failed { seg: Option<LocalId>, at: UnixMillis, attempt: LocalId, phase: BaselinePhase, reason: BaselineFailure },
    /// 同じリポジトリでの同時作業。
    Concurrent { seg: LocalId, with: ConcurrentWith },
    /// 「変更を戻す」の実行結果（成功分だけ `items`、失敗は `failed`）。
    Reverted { at: UnixMillis, from_seg: LocalId, items: Vec<RevertedPath>, failed: Vec<RevertFailure>, backup_dir: Option<String> },
}

/// `segments.jsonl` に書く1行（版つき）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineEntry {
    pub schema_version: u32,
    #[serde(flatten)]
    pub line: BaselineLine,
}

impl BaselineEntry {
    pub fn new(line: BaselineLine) -> Self {
        BaselineEntry { schema_version: BASELINE_SCHEMA_VERSION, line }
    }
}

// ───────────────────────────── IPC ─────────────────────────────

/// 区間の状態（「戻す範囲」の選択肢と理由表示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SegmentState {
    Ready,
    /// 作業中（終了の控えはまだ）。
    Running,
    /// 終了の控えがない（切断・再起動など）。
    EndUnknown,
    Failed { reason: BaselineFailure },
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SegmentStatus {
    /// 対応付いたturn（受理不明のままなら None）。
    pub turn: Option<ExternalId>,
    pub started_at: UnixMillis,
    pub state: SegmentState,
    /// 同じリポジトリで同時に作業していた会話がある。
    pub concurrent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(tree: &str) -> Snapshot {
        Snapshot {
            head: Some("a".repeat(40)),
            branch: Some("main".into()),
            tree: tree.into(),
            raw: vec!["src/a.rs".into()],
            skipped: vec![SkippedPath { path: "l".into(), reason: SkipReason::Link }],
            took_ms: 12,
        }
    }

    fn roundtrip(line: BaselineLine) {
        let text = serde_json::to_string(&BaselineEntry::new(line.clone())).unwrap();
        assert!(text.contains("\"schemaVersion\":1"), "{text}");
        assert!(!text.contains('\n'), "one line per record");
        let back: BaselineEntry = serde_json::from_str(&text).unwrap();
        assert_eq!(back.schema_version, 1);
        assert_eq!(back.line, line);
    }

    #[test]
    fn every_line_kind_survives_a_serde_roundtrip() {
        let seg = LocalId("seg-1".into());
        roundtrip(BaselineLine::Started {
            seg: seg.clone(),
            at: UnixMillis(5),
            attempt: LocalId("att-1".into()),
            repo: RepoRef { root: "C:/repo".into(), objects_dir: "C:/repo/.git/objects".into() },
            cwd: "C:/repo/sub".into(),
            base: snap("t1"),
        });
        roundtrip(BaselineLine::Bound { seg: seg.clone(), turn: ExternalId("turn-1".into()) });
        roundtrip(BaselineLine::Ended { seg: seg.clone(), at: UnixMillis(9), end: SegmentEnd::Snapshot { snapshot: snap("t2") } });
        roundtrip(BaselineLine::Ended { seg: seg.clone(), at: UnixMillis(9), end: SegmentEnd::EndIsNextBase });
        roundtrip(BaselineLine::Abandoned { seg: seg.clone(), reason: "rejected".into() });
        for reason in [
            BaselineFailure::NotARepository,
            BaselineFailure::GitTooOld { found: "2.30.0".into() },
            BaselineFailure::InsufficientSpace { required: 10, available: 3 },
            BaselineFailure::Git { message: "boom".into() },
            BaselineFailure::Disabled,
        ] {
            roundtrip(BaselineLine::Failed { seg: None, at: UnixMillis(1), attempt: LocalId("att".into()), phase: BaselinePhase::Base, reason });
        }
        roundtrip(BaselineLine::Concurrent { seg: seg.clone(), with: ConcurrentWith::External { label: "VS Code".into() } });
        roundtrip(BaselineLine::Concurrent {
            seg: seg.clone(),
            with: ConcurrentWith::Chat { chat: ChatKey { backend: BackendKind::Codex, id: ExternalId("c2".into()) } },
        });
        roundtrip(BaselineLine::Reverted {
            at: UnixMillis(20),
            from_seg: seg,
            items: vec![
                RevertedPath {
                    path: "src/a.rs".into(),
                    restored: Restored::Entry { entry: EntryRef { oid: "b".repeat(40), form: EntryForm::Raw } },
                    backup_file: Some("0001".into()),
                    forced: true,
                    overridden: vec![RevertBlockCode::ChangedAfter, RevertBlockCode::EndUnknown],
                },
                RevertedPath { path: "gone.txt".into(), restored: Restored::Absent, backup_file: None, forced: false, overridden: vec![] },
            ],
            failed: vec![RevertFailure { path: "x".into(), reason: "locked".into() }],
            backup_dir: Some("C:/data/revert-backup/20".into()),
        });
    }

    #[test]
    fn old_settings_and_usage_without_the_new_fields_still_read() {
        use super::super::local::{AppSettings, UsageBreakdown};
        let mut v = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(v["baselines"]["enabled"], true, "on by default");
        v.as_object_mut().unwrap().remove("baselines");
        let s: AppSettings = serde_json::from_value(v).unwrap();
        assert!(s.baselines.enabled);
        let u: UsageBreakdown = serde_json::from_str(r#"{"attachments":1,"artifacts":2,"workspace":3,"activity":4,"metadata":5}"#).unwrap();
        assert_eq!((u.baselines, u.revert_backups), (0, 0));
    }

    #[test]
    fn reverted_items_without_forced_fields_still_read() {
        let text = r#"{"schemaVersion":1,"kind":"reverted","at":1,"fromSeg":"s","items":[{"path":"a","restored":{"kind":"absent"},"backupFile":null}],"failed":[],"backupDir":null}"#;
        let e: BaselineEntry = serde_json::from_str(text).unwrap();
        let BaselineLine::Reverted { items, .. } = e.line else { panic!("not reverted") };
        assert!(!items[0].forced && items[0].overridden.is_empty());
    }
}

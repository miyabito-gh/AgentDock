//! 組織実行の成果物パネルと確認つきの取り込み（DESIGN_ORG_R3）のホスト側。Tauriコマンド（`commands.rs`）が呼ぶ。
//!
//! 守ること:
//! - **閲覧・プレビューでは何も書かない**（`org_artifacts`／`org_artifact_diff`／`org_artifact_preview`／`org_import_preview`／
//!   `org_import_verify`／`org_import_undo_preview`）。作業フォルダ・索引・ブランチ・worktreeを変えない（増えるのはGitのオブジェクトだけ）。
//! - 取り込み・戻すは `&UserConfirmed`（確認ダイアログの後の操作）を要し、自動では呼ばない。同じフォルダのGit操作（準備・プッシュ・
//!   取り込み・戻す）は同時に1本だけ。再送しない（結果が分からなければ「確認できません」）。
//! - 台帳 `imports.jsonl` の `begin` を書けなければ実行しない。`Applied`／`Undone` は読み戻しで確かめたときだけ。
//! - worktree・ブランチ・`worktrees.json` は変えない。成果物パネルからは削除できない。
//! - 「開く…」は blob を AgentDock 領域の読取り専用のコピーへ書き出して開く（worktree 内のファイルは開かない＝誤編集防止）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use org_engine::graph::{CompiledGraph, NodeKind as GNode};
use org_engine::{restore, EventRead, OrgStore, Task, TaskStatus, WorkflowState};

use super::graphs;
use super::ports::RuntimeRead;
use super::view::status_view;
use crate::backend::backend::UserConfirmed;
use crate::backend::ipc::{BlockedReason, HostEvent, IpcError, IpcErrorCode};
use crate::backend::model::{Known, OrgWorktreeAccess, WorktreeRecord};
use crate::backend::org::OrgTaskPhase;
use crate::backend::org_artifacts::*;
use crate::gitops::import as gi;
use crate::gitops::worktree as gw;
use crate::gitops::artifacts as ga;
use crate::host::gitprep::busy_error;
use crate::host::{blocked, err, now_ms, Host};
use crate::rules::org::turn_key_of_ref;
use crate::rules::org_artifacts::*;
use crate::store::atomic::append_line;
use crate::store::layout::sanitize_file_name;
use crate::store::org::{read_delete_record, workflow_dir, ReadOnlyOrgStore, IMPORTS_FILE};

/// 確認（プレビュー）から実行までの有効時間。
const TOKEN_TTL_MS: i64 = 10 * 60 * 1000;

/// 取り込みの確認で作った結果の控え（メモリだけ。再起動で無効になり、再確認になる）。
struct ImportToken {
    workflow_id: String,
    prepared: gi::Prepared,
    not_passed: Vec<String>,
    created_ms: i64,
}

#[derive(Default)]
pub struct ArtifactRuntime {
    tokens: Mutex<HashMap<String, ImportToken>>,
}

impl ArtifactRuntime {
    fn put(&self, id: String, t: ImportToken) {
        let mut m = self.tokens.lock().unwrap();
        let now = now_ms().0;
        m.retain(|_, v| now - v.created_ms <= TOKEN_TTL_MS);
        m.insert(id, t);
    }
    /// 控えを取り出す（1回だけ使える）。期限切れ・ないものは None。
    fn take(&self, id: &str) -> Option<ImportToken> {
        let t = self.tokens.lock().unwrap().remove(id)?;
        (now_ms().0 - t.created_ms <= TOKEN_TTL_MS).then_some(t)
    }
}

/// 1つのワークフローの保存データ（読取り専用で復元したもの）。
struct Wf {
    id: String,
    dir: PathBuf,
    state: WorkflowState,
    events: EventRead,
    /// 開始時のグラフの写し（読めなければ None。グラフの検証役の所見は「未取得」になる）。
    graph: Option<CompiledGraph>,
    delete_pending: bool,
}

impl Wf {
    fn finished(&self) -> bool {
        let t = self.state.tasks();
        !t.is_empty() && t.iter().all(|t| matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled))
    }
    fn name(&self) -> String {
        self.state.request().lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(40).collect()
    }
    fn store(&self) -> ReadOnlyOrgStore {
        ReadOnlyOrgStore::new(self.dir.clone())
    }
}

fn io_err(what: &str, e: impl std::fmt::Display) -> IpcError {
    err(IpcErrorCode::Io, format!("{what}: {e}"))
}

fn join_err(e: tokio::task::JoinError) -> IpcError {
    err(IpcErrorCode::Io, format!("処理が異常終了しました: {e}"))
}

fn wt_message(e: &gw::WtError) -> String {
    e.message()
}

/// 台帳の追記（追記のみ）。
fn append_ledger(dir: &Path, row: &LedgerRow) -> Result<(), String> {
    let line = serde_json::to_string(row).map_err(|e| e.to_string())?;
    append_line(&dir.join(IMPORTS_FILE), &line).map_err(|e| e.to_string())
}

/// 台帳を読む（なければ空。読めない行は飛ばす）。
fn read_ledger(dir: &Path) -> Vec<LedgerRecord> {
    match std::fs::read(dir.join(IMPORTS_FILE)) {
        Ok(b) => fold_ledger(&String::from_utf8_lossy(&b)),
        Err(_) => Vec::new(),
    }
}

/// 一覧の行の印。台帳（取り込み済み）とエンジンの状態だけから導く（Gitは読まない）。
pub fn hint_for(dir: &Path, tasks: &[crate::backend::org::OrgTaskView]) -> Option<OrgArtifactHint> {
    let rows: Vec<(String, Vec<String>, bool, OrgTaskPhase)> = tasks.iter().map(|t| (t.id.clone(), t.deps.clone(), t.commit.is_some(), t.status.phase)).collect();
    if !rows.iter().any(|r| r.2) {
        return None;
    }
    let applied = applied_tasks(&read_ledger(dir), None);
    artifact_hint(&rows, &applied)
}

fn record_view(r: &LedgerRecord, state: OrgImportState) -> OrgImportRecordView {
    OrgImportRecordView {
        id: r.id.clone(),
        at: r.at,
        folder: r.folder.clone(),
        branch: r.branch.clone(),
        before: r.before.clone(),
        result: r.result.clone(),
        tasks: r.tasks.iter().map(|t| t.task.clone()).collect(),
        state,
    }
}

/// 取り込みの確認ダイアログの本文（Gitの語を前に出さない）。
const IMPORT_MESSAGE: &str = "選んだ成果を、作業フォルダの今のブランチに取り込みます。取り込み前の状態は記録し、送信（プッシュ）前なら「取り込み前に戻す」で戻せます。AgentDock は自動で取り込みません。";

impl Host {
    fn org_artifact_root(&self) -> Result<PathBuf, IpcError> {
        self.persist.store().map(|s| s.root().to_path_buf()).ok_or_else(|| err(IpcErrorCode::Io, "保存先が使えないため、ワークフローの記録を扱えません"))
    }

    async fn org_load_wf(self: &Arc<Self>, workflow_id: &str) -> Result<Wf, IpcError> {
        let root = self.org_artifact_root()?;
        let id = workflow_id.to_string();
        tokio::task::spawn_blocking(move || {
            let dir = workflow_dir(&root, &id);
            if dir.file_name().is_none_or(|n| n.to_string_lossy() != id) || !dir.is_dir() {
                return Err(err(IpcErrorCode::NotFound, "ワークフローが見つかりません"));
            }
            let store = ReadOnlyOrgStore::new(dir.clone());
            let (state, _) = restore(&store).map_err(|e| io_err("ワークフローの記録を読めません", format!("{e:?}")))?;
            let events = store.read_events().unwrap_or_default();
            let graph = state.graph().and_then(|g| graphs::load_copies(&dir, g).ok()).map(|(c, _)| c);
            // 削除の記録を読めなければ、削除の途中として扱う（取り込まない側に倒す）。
            let delete_pending = read_delete_record(&dir).map(|r| r.is_some()).unwrap_or(true);
            Ok(Wf { id, dir, state, events, graph, delete_pending })
        })
        .await
        .map_err(join_err)?
    }

    fn org_writer_records(&self, workflow_id: &str) -> Vec<WorktreeRecord> {
        self.read(|d| {
            d.worktrees
                .iter()
                .filter(|r| r.org.as_ref().is_some_and(|o| o.workflow_id == workflow_id && o.access == OrgWorktreeAccess::Writable))
                .cloned()
                .collect()
        })
    }

    // ───────────── 読取り ─────────────

    /// 成果物パネルの中身（読取りのみ）。
    pub async fn org_artifacts(self: &Arc<Self>, args: OrgArtifactsArgs) -> Result<OrgArtifactsView, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        Ok(self.org_build_artifacts(&wf).await)
    }

    async fn org_build_artifacts(self: &Arc<Self>, wf: &Wf) -> OrgArtifactsView {
        let git = self.git_for_worktree();
        let repo = PathBuf::from(wf.state.repo());
        let facts = gi::repo_facts(&git, &repo).await;
        let recs = self.org_writer_records(&wf.id);
        let dir = wf.dir.clone();
        let ledger = tokio::task::spawn_blocking(move || read_ledger(&dir)).await.unwrap_or_default();
        let head = facts.head.clone();
        let states: Vec<(&LedgerRecord, OrgImportState)> = ledger.iter().map(|r| (r, record_state(r, head.as_deref()))).collect();
        let applied: HashSet<String> = states.iter().filter(|(_, s)| *s == OrgImportState::Applied).flat_map(|(r, _)| r.tasks.iter().map(|t| t.task.clone())).collect();

        // 書込みタスク（専用worktreeの記録があるか、コミットがあるもの）。
        struct W<'a> {
            task: &'a Task,
            rec: Option<&'a WorktreeRecord>,
            phase: OrgTaskPhase,
            commit: Option<String>,
            base: Option<String>,
            git_ok: bool,
            in_head: Option<bool>,
            files: Known<Vec<OrgArtifactFile>>,
        }
        let mut ws: Vec<W> = Vec::new();
        for t in wf.state.tasks() {
            let rec = recs.iter().find(|r| r.org.as_ref().is_some_and(|o| o.task_id == t.id.0));
            if rec.is_none() && t.commit().is_none() {
                continue;
            }
            let commit = t.commit().map(str::to_string);
            let base = rec.map(|r| r.base_commit.clone()).filter(|b| !b.is_empty());
            let mut w = W { task: t, rec, phase: status_view(t.status()).phase, commit, base, git_ok: true, in_head: None, files: Known::NotFetched };
            if let Some(c) = w.commit.clone() {
                w.git_ok = gw::resolve_commit(&git, &repo, &c).await.is_ok();
                if w.git_ok {
                    if let Some(h) = &head {
                        w.in_head = ga::is_ancestor(&git, &repo, &c, h).await;
                    }
                    if let Some(b) = &w.base {
                        if let Ok(files) = ga::list_files(&git, &repo, b, &c).await {
                            w.files = Known::direct(files);
                        }
                    }
                }
            }
            ws.push(w);
        }

        // 後の書込みが前の書込みを含むもの（取り込むのは後の成果だけでよい）。
        let mut pair: HashMap<(String, String), Option<bool>> = HashMap::new();
        let open: Vec<&W> = ws.iter().filter(|w| w.commit.is_some() && w.git_ok && w.in_head != Some(true) && matches!(w.phase, OrgTaskPhase::Completed | OrgTaskPhase::Failed | OrgTaskPhase::Cancelled)).collect();
        for a in &open {
            for b in &open {
                let (ca, cb) = (a.commit.clone().unwrap(), b.commit.clone().unwrap());
                if ca != cb && !pair.contains_key(&(ca.clone(), cb.clone())) {
                    let r = ga::is_ancestor(&git, &repo, &ca, &cb).await;
                    pair.insert((ca, cb), r);
                }
            }
        }
        let commits: Vec<(String, String)> = open.iter().map(|w| (w.task.id.0.clone(), w.commit.clone().unwrap())).collect();
        let superseded = supersede(&commits, |a, b| pair.get(&(a.to_string(), b.to_string())).copied().flatten());

        // 取り込みの前提（作業フォルダ側）。
        let blockers = import_blockers(&ImportFacts {
            branch: facts.branch.as_deref(),
            in_progress: facts.in_progress,
            workflow_finished: wf.finished(),
            delete_pending: wf.delete_pending,
            selected: &[("-".to_string(), OrgArtifactStatus::ReadyForReview)],
            git_supported: facts.git_supported,
        });
        let dirty = gi::dirty_paths(&git, &repo).await.map(|p| p.len() as u32);
        let target = OrgImportTargetView {
            folder: wf.state.repo().to_string(),
            branch: facts.branch.clone().map_or(Known::NotFetched, Known::direct),
            head: head.clone().map_or(Known::NotFetched, Known::direct),
            dirty: dirty.map_or(Known::NotFetched, Known::direct),
            blockers: blockers.clone(),
        };

        let store = wf.store();
        let mut items = Vec::new();
        for w in &ws {
            let facts_s = StatusFacts {
                phase: w.phase,
                has_commit: w.commit.is_some(),
                in_head: w.in_head,
                superseded: superseded.contains(&w.task.id.0),
                ledger_applied: applied.contains(&w.task.id.0),
                git_ok: w.git_ok,
            };
            let Some(status) = artifact_status(&facts_s) else { continue };
            let files = w.files.clone();
            let totals = match &files {
                Known::Value { value, .. } => Known::direct(totals(value)),
                _ => Known::NotFetched,
            };
            let report = self.org_report_of(wf, &store, w.task).await;
            let reviews = self.org_reviews_of(wf, &store, w.task).await;
            let mismatch = match (&report, &files) {
                (Known::Value { value: r, .. }, Known::Value { value: f, .. }) => {
                    let diff_paths: Vec<String> = f.iter().map(|x| new_path(x).to_string()).collect();
                    let (a, b) = report_vs_diff(&r.changed_files, &diff_paths);
                    mismatch_lines(&a, &b)
                }
                _ => Vec::new(),
            };
            let uncommitted = match w.rec.map(|r| PathBuf::from(&r.path)).filter(|p| p.is_dir()) {
                Some(p) => gw::dirty_count(&git, &p).await.map_or(Known::NotFetched, Known::direct),
                None => Known::NotFetched,
            };
            let importable = is_candidate(status) && blockers.is_empty();
            let import_refusal = if !is_candidate(status) {
                Some(format!("「{}」のため、取り込めません", status_label(status)))
            } else if !blockers.is_empty() {
                Some(blockers.join(" / "))
            } else {
                None
            };
            let status_note = applied_but_missing(&facts_s).then(|| APPLIED_BUT_MISSING_NOTE.to_string());
            let imports = states.iter().filter(|(r, _)| r.tasks.iter().any(|t| t.task == w.task.id.0)).map(|(r, s)| record_view(r, *s)).collect();
            items.push(OrgArtifactItem {
                task: w.task.id.0.clone(),
                title: w.task.title.clone(),
                role: w.task.role.0.clone(),
                branch: w.rec.map(|r| r.branch.clone()).unwrap_or_default(),
                base: w.base.clone(),
                commit: w.commit.clone(),
                status,
                status_label: status_label(status).to_string(),
                status_note,
                files,
                totals,
                report,
                reviews,
                report_mismatch: mismatch,
                uncommitted,
                importable,
                import_refusal,
                imports,
            });
        }
        let mut records: Vec<OrgImportRecordView> = states.iter().map(|(r, s)| record_view(r, *s)).collect();
        records.reverse();
        OrgArtifactsView { workflow_id: wf.id.clone(), target, items, fetched: Known::direct(now_ms()), git_supports_import: facts.git_supported.map_or(Known::NotFetched, Known::direct), records }
    }

    /// Worker の報告。保存した出力 → 履歴の最終回答の順（どちらも取れなければ「未取得」）。グラフの書込み工程はスロットの値（形が合うときだけ）。
    async fn org_report_of(self: &Arc<Self>, wf: &Wf, store: &ReadOnlyOrgStore, t: &Task) -> Known<OrgWorkerReportView> {
        let found = |v: Option<OrgWorkerReportView>| v.map_or(Known::NotFetched, Known::direct);
        if let Some(node) = t.node() {
            // グラフ: 書込み工程の出力スロットの最新の版。
            let slot = wf.graph.as_ref().and_then(|g| g.node(node)).and_then(|d| match &d.kind {
                GNode::Agent { output, .. } => Some(output.clone()),
                _ => None,
            });
            let Some(slot) = slot else { return Known::NotFetched };
            let Some(version) = latest_slot_version(&wf.events.events, &t.id, &slot) else { return Known::NotFetched };
            return found(store.read_slot(&slot, version).ok().flatten().and_then(|v| parse_report_value(&v)));
        }
        let attempt = t.attempts().saturating_sub(1);
        if let Some(v) = store.read_output(&t.id, attempt).ok().flatten().and_then(|v| parse_report_value(&v)) {
            return Known::direct(v);
        }
        // 保存されていない（旧ワークフロー・保存失敗）: 受理されたturnの最終回答を履歴から読む。
        let text = self.org_final_message_of(t).await;
        found(text.and_then(|x| org_engine::schemas::parse_worker_report(&x).ok()).map(|r| OrgWorkerReportView {
            summary: r.summary,
            changed_files: r.changed_files,
            tests_ran: r.tests.ran,
            tests_passed: r.tests.passed,
        }))
    }

    async fn org_final_message_of(self: &Arc<Self>, t: &Task) -> Option<String> {
        let chat = t.chat().and_then(crate::rules::org::chat_key_of_ref)?;
        let turn = t.accepted_turn()?;
        RuntimeRead::read_final_message(self, &chat, &turn_key_of_ref(&chat, turn)).await.ok().flatten()
    }

    /// 検証役の判定（古い順）。
    ///
    /// - 計画経路: 試行 k の判定は検証役の出力 `outputs\<検証役>.<k>.json`。ない回は、差し戻しの所見の記録（`FindingsRecorded`）→
    ///   最新の回だけ検証役の最終回答（履歴）→ なければ判定「未取得」（合格・不合格を推定しない）。
    /// - グラフ: 書込み工程を `target` にする検証工程のスロットの各版。対象のコミットは、その版の書込み（`SlotWritten`）より前で
    ///   書込み工程が最後にコミットしたもの（検証役は確定済みの最新のコミットへ戻してから読む＝グラフの `Refresh` ＝その間に書込み工程は
    ///   コミットしない、ことを根拠にしている。`graphrun.rs` の `graph_launch`）。
    async fn org_reviews_of(self: &Arc<Self>, wf: &Wf, store: &ReadOnlyOrgStore, t: &Task) -> Known<Vec<OrgReviewView>> {
        let mut out: Vec<OrgReviewView> = Vec::new();
        if let Some(node) = t.node() {
            let Some(g) = wf.graph.as_ref() else { return Known::NotFetched };
            let reviewers: Vec<&Task> = wf
                .state
                .tasks()
                .iter()
                .filter(|r| {
                    r.node().and_then(|n| g.node(n)).is_some_and(|d| matches!(&d.kind, GNode::Agent { target: Some(tn), .. } if tn == node))
                })
                .collect();
            for r in reviewers {
                let Some(slot) = r.node().and_then(|n| g.node(n)).and_then(|d| match &d.kind {
                    GNode::Agent { output, .. } => Some(output.clone()),
                    _ => None,
                }) else {
                    continue;
                };
                for rr in review_rounds(&wf.events.events, &t.id, &r.id, &slot) {
                    let parsed = store.read_slot(&slot, rr.version).ok().flatten().and_then(|v| parse_review_value(&v));
                    let (verdict, findings) = parsed.unwrap_or((OrgReviewVerdict::Unknown, Vec::new()));
                    out.push(OrgReviewView { reviewer_task: r.id.0.clone(), verdict, findings, round: rr.round, commit: rr.commit, from_history: false });
                }
            }
            return Known::direct(out);
        }
        // 計画経路。
        let reviewers: Vec<&Task> = wf.state.tasks().iter().filter(|r| r.role.0 == "reviewer" && r.deps.first() == Some(&t.id)).collect();
        let findings_log = recorded_findings(&wf.events.events, &t.id);
        let attempts = t.attempts();
        for r in reviewers {
            for k in 0..attempts {
                let stored = store.read_output(&r.id, k).ok().flatten().and_then(|v| parse_review_value(&v));
                let (verdict, findings, from_history) = match stored {
                    Some((v, f)) => (v, f, false),
                    None if k + 1 < attempts => match findings_log.get(k as usize) {
                        Some(f) => (OrgReviewVerdict::Rework, f.clone(), true),
                        None => (OrgReviewVerdict::Unknown, Vec::new(), true),
                    },
                    None => {
                        let text = self.org_final_message_of(r).await;
                        match text.and_then(|x| org_engine::schemas::parse_verdict(&x).ok()) {
                            Some(v) => (
                                match v.verdict {
                                    org_engine::schemas::VerdictKind::Pass => OrgReviewVerdict::Pass,
                                    org_engine::schemas::VerdictKind::Rework => OrgReviewVerdict::Rework,
                                },
                                v.findings,
                                true,
                            ),
                            None => (OrgReviewVerdict::Unknown, Vec::new(), true),
                        }
                    }
                };
                out.push(OrgReviewView { reviewer_task: r.id.0.clone(), verdict, findings, round: k + 1, commit: None, from_history });
            }
        }
        Known::direct(out)
    }

    /// タスクの書込み成果（元・コミット）。成果物パネルに出るものだけ。
    fn org_writer_of(&self, wf: &Wf, task: &str) -> Result<(Option<String>, String), IpcError> {
        let t = wf.state.tasks().iter().find(|t| t.id.0 == task).ok_or_else(|| err(IpcErrorCode::NotFound, format!("タスク {task} が見つかりません")))?;
        let commit = t.commit().map(str::to_string).ok_or_else(|| err(IpcErrorCode::NotFound, format!("タスク {task} にはコミットがありません")))?;
        let base = self
            .org_writer_records(&wf.id)
            .into_iter()
            .find(|r| r.org.as_ref().is_some_and(|o| o.task_id == task))
            .map(|r| r.base_commit)
            .filter(|b| !b.is_empty());
        Ok((base, commit))
    }

    /// 1ファイルの差分（読取りのみ）。
    pub async fn org_artifact_diff(self: &Arc<Self>, args: OrgArtifactPathArgs) -> Result<OrgArtifactDiff, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let (base, commit) = self.org_writer_of(&wf, &args.task)?;
        let path = args.move_to.clone().unwrap_or_else(|| args.path.clone());
        let git = self.git_for_worktree();
        let repo = PathBuf::from(wf.state.repo());
        let Some(base) = base else { return Ok(OrgArtifactDiff { path, text: Known::NotFetched }) };
        let old = args.move_to.as_ref().map(|_| args.path.as_str());
        let text = match ga::diff_text(&git, &repo, &base, &commit, &path, old).await {
            Ok(t) => Known::direct(t),
            Err(_) => Known::NotFetched,
        };
        Ok(OrgArtifactDiff { path, text })
    }

    /// プレビュー（読取りのみ）。
    pub async fn org_artifact_preview(self: &Arc<Self>, args: OrgArtifactPathArgs) -> Result<OrgArtifactPreview, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let (_, commit) = self.org_writer_of(&wf, &args.task)?;
        let path = args.move_to.clone().unwrap_or_else(|| args.path.clone());
        let git = self.git_for_worktree();
        let repo = PathBuf::from(wf.state.repo());
        let kind = preview_kind(&path);
        let refuse = |bytes: u64, why: &str| OrgArtifactPreview { kind, text: None, data_url: None, truncated: false, bytes, refusal: Some(why.to_string()) };
        if kind == OrgPreviewKind::None {
            return Ok(refuse(0, "この形式はプレビューできません。「開く…」で関連アプリから確認してください"));
        }
        let size = match ga::blob_size(&git, &repo, &commit, &path).await {
            Ok(s) => s,
            Err(e) => return Ok(refuse(0, &format!("ファイルを読めませんでした（{}）。このコミットにファイルがない（削除された）場合があります", wt_message(&e)))),
        };
        let cap = if kind == OrgPreviewKind::Image { IMAGE_PREVIEW_MAX as u64 } else { crate::gitops::OUTPUT_LIMIT as u64 };
        if size > cap {
            return Ok(refuse(size, if kind == OrgPreviewKind::Image { "画像が大きすぎます（10MB超）。「開く…」で確認してください" } else { "ファイルが大きすぎます（16MB超）。「開く…」で確認してください" }));
        }
        let blob = match ga::read_blob(&git, &repo, &commit, &path, cap).await {
            Ok(Ok(b)) => b,
            Ok(Err(s)) => return Ok(refuse(s, "ファイルが大きすぎます。「開く…」で確認してください")),
            Err(e) => return Ok(refuse(size, &format!("ファイルを読めませんでした（{}）", wt_message(&e)))),
        };
        if kind == OrgPreviewKind::Image {
            let Some(mime) = image_mime(&path) else { return Ok(refuse(size, "画像の種類を判定できません")) };
            let url = format!("data:{mime};base64,{}", base64_encode(&blob.bytes));
            return Ok(OrgArtifactPreview { kind, text: None, data_url: Some(url), truncated: false, bytes: size, refusal: None });
        }
        let cut = blob.bytes.len() > TEXT_PREVIEW_MAX;
        let head = &blob.bytes[..blob.bytes.len().min(TEXT_PREVIEW_MAX)];
        let text = match decode_text(head, cut) {
            Ok(t) => t,
            Err(r) => return Ok(refuse(size, r.message())),
        };
        let (text, rows_cut) = if kind == OrgPreviewKind::Table { limit_rows(&text, TABLE_PREVIEW_ROWS) } else { (text, false) };
        Ok(OrgArtifactPreview { kind, text: Some(text), data_url: None, truncated: cut || rows_cut, bytes: size, refusal: None })
    }

    /// 「開く…」: blob を AgentDock 領域の読取り専用のコピーへ書き出し、そのパスを返す（開くのはコマンド層）。実行形式は確認が要る。
    pub async fn org_artifact_open_target(self: &Arc<Self>, args: OrgArtifactOpenArgs) -> Result<PathBuf, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let (_, commit) = self.org_writer_of(&wf, &args.task)?;
        let path = args.move_to.clone().unwrap_or_else(|| args.path.clone());
        let root = self.org_artifact_root()?;
        let name = sanitize_file_name(path.rsplit(['/', '\\']).next().unwrap_or(&path));
        let target = root.join("tmp").join("artifacts").join(sanitize_file_name(&wf.id)).join(sanitize_file_name(&args.task)).join(&name);
        if !args.risk_confirmed && crate::attach::is_executable_path(&target) {
            return Err(blocked(BlockedReason::OpenNeedsConfirm { path: target.to_string_lossy().into_owned(), outside_project: false, executable: true }, "開く前に確認が必要です"));
        }
        let git = self.git_for_worktree();
        let repo = PathBuf::from(wf.state.repo());
        let blob = match ga::read_blob(&git, &repo, &commit, &path, crate::gitops::OUTPUT_LIMIT as u64).await {
            Ok(Ok(b)) => b,
            Ok(Err(_)) => return Err(err(IpcErrorCode::Rejected, "ファイルが大きすぎて開けません（16MB超）")),
            Err(e) => return Err(err(IpcErrorCode::Rejected, format!("ファイルを読めませんでした（{}）", wt_message(&e)))),
        };
        let out = target.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            // 前に書き出した読取り専用のコピーは、属性を外して置き換える（開かれていれば失敗して、その旨を返す）。
            if out.exists() {
                if let Ok(m) = std::fs::metadata(&out) {
                    let mut p = m.permissions();
                    p.set_readonly(false);
                    let _ = std::fs::set_permissions(&out, p);
                }
                std::fs::remove_file(&out).map_err(|e| format!("前に開いたコピーを置き換えられません（開いたままかもしれません）: {e}"))?;
            }
            std::fs::write(&out, &blob.bytes).map_err(|e| e.to_string())?;
            let mut p = std::fs::metadata(&out).map_err(|e| e.to_string())?.permissions();
            p.set_readonly(true);
            std::fs::set_permissions(&out, p).map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)?
        .map_err(|m| err(IpcErrorCode::Io, m))?;
        Ok(target)
    }

    // ───────────── 取り込み ─────────────

    /// 取り込みの確認の材料（作業フォルダには何も書かない）。止める理由があれば `blockers` に出し、控え（`token`）は空にする。
    pub async fn org_import_preview(self: &Arc<Self>, args: OrgImportPreviewArgs) -> Result<OrgImportPreview, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let view = self.org_build_artifacts(&wf).await;
        let git = self.git_for_worktree();
        let repo = PathBuf::from(wf.state.repo());
        let facts = gi::repo_facts(&git, &repo).await;
        let mut base = OrgImportPreview {
            token: String::new(),
            folder: wf.state.repo().to_string(),
            branch: facts.branch.clone().unwrap_or_default(),
            head: facts.head.clone().unwrap_or_default(),
            candidates: Vec::new(),
            files: Vec::new(),
            totals: OrgArtifactTotals::default(),
            conflicts: Vec::new(),
            dirty_overlap: Vec::new(),
            locked_files: Vec::new(),
            dirty_other: 0,
            not_passed: Vec::new(),
            blockers: Vec::new(),
            message: IMPORT_MESSAGE.to_string(),
        };
        // 選ばれた成果（選んだ順）。
        let mut selected: Vec<(String, OrgArtifactStatus)> = Vec::new();
        let mut cands: Vec<gi::Candidate> = Vec::new();
        for task in &args.tasks {
            let Some(item) = view.items.iter().find(|i| &i.task == task) else {
                base.blockers.push(format!("{task}: 成果物が見つかりません"));
                continue;
            };
            selected.push((task.clone(), item.status));
            if let Some(commit) = &item.commit {
                base.candidates.push(OrgImportCandidate { task: task.clone(), title: item.title.clone(), branch: item.branch.clone(), commit: commit.clone(), status: item.status });
                cands.push(gi::Candidate { task: task.clone(), title: item.title.clone(), commit: commit.clone() });
            }
            if item.status == OrgArtifactStatus::NotPassed {
                base.not_passed.push(task.clone());
            }
        }
        base.blockers.extend(import_blockers(&ImportFacts {
            branch: facts.branch.as_deref(),
            in_progress: facts.in_progress,
            workflow_finished: wf.finished(),
            delete_pending: wf.delete_pending,
            selected: &selected,
            git_supported: facts.git_supported,
        }));
        // 取り込み先は、利用者が作業するリポジトリ本体（AgentDockが作ったworktreeではない）。
        if self.org_writer_records_all().iter().any(|r| crate::rules::baseline::norm_path(&r.path) == crate::rules::baseline::norm_path(wf.state.repo())) {
            base.blockers.push("取り込み先がAgentDockの作業場所（worktree）のため、取り込めません".to_string());
        }
        if !base.blockers.is_empty() {
            return Ok(base);
        }
        match gi::prepare(&git, &repo, &facts, &wf.name(), &cands).await {
            Err(e) => base.blockers.push(format!("成果の合成を確認できませんでした（{}）", wt_message(&e))),
            Ok(gi::PrepareOutcome::Failed(m)) => base.blockers.push(m),
            Ok(gi::PrepareOutcome::NoChange) => base.blockers.push("取り込んでも変わるファイルがありません（すでに同じ内容です）".to_string()),
            Ok(gi::PrepareOutcome::Conflict { files }) => {
                let branches: Vec<String> = base.candidates.iter().map(|c| c.branch.clone()).filter(|b| !b.is_empty()).collect();
                base.blockers.push(format!(
                    "同じ箇所を、あなたのブランチと成果の両方が変えているため、自動では合わせられません（ファイル: {}）。何も変更していません。取り込む成果を減らすか、ブランチ {} を手動で取り込んでください。",
                    files.join("、"),
                    if branches.is_empty() { "（agentdock/…）".to_string() } else { branches.join("、") }
                ));
                base.conflicts = files;
            }
            Ok(gi::PrepareOutcome::Ready(p)) => {
                base.files = p.files.clone();
                base.totals = totals(&p.files);
                match gi::check_worktree(&git, &repo, &p.files).await {
                    Err(e) => base.blockers.push(format!("作業フォルダの状態を確認できませんでした（{}）", wt_message(&e))),
                    Ok(check) => {
                        base.dirty_overlap = check.overlap.clone();
                        base.locked_files = check.locked.clone();
                        base.dirty_other = check.dirty_total.saturating_sub(check.overlap.len() as u32);
                        if !check.overlap.is_empty() {
                            base.blockers.push(format!("あなたの未コミットの変更と重なるファイルがあるため、取り込めません（{}）。何も変更していません。変更をコミットするか戻してから、もう一度お試しください。", check.overlap.join("、")));
                        }
                        if !check.locked.is_empty() {
                            base.blockers.push(format!("他のアプリで開かれているファイルがあるため、取り込めません（{}）。何も変更していません。閉じてから、もう一度お試しください。", check.locked.join("、")));
                        }
                        if base.blockers.is_empty() {
                            let token = self.local_id("impt").0;
                            self.org_rt.artifacts.put(token.clone(), ImportToken { workflow_id: wf.id.clone(), prepared: p, not_passed: base.not_passed.clone(), created_ms: now_ms().0 });
                            base.token = token;
                        }
                    }
                }
            }
        }
        Ok(base)
    }

    fn org_writer_records_all(&self) -> Vec<WorktreeRecord> {
        self.read(|d| d.worktrees.iter().filter(|r| r.org.is_some()).cloned().collect())
    }

    /// 取り込みを実行する（確認ダイアログの「取り込む」だけ）。確認から変わっていれば何も書かない。結果はHEADの読み戻しで決める。
    pub async fn org_import(self: &Arc<Self>, args: OrgImportArgs, _confirmed: &UserConfirmed) -> Result<OrgImportResult, IpcError> {
        let Some(token) = self.org_rt.artifacts.take(&args.token) else {
            return Err(err(IpcErrorCode::Rejected, "確認の有効期限が切れたか、確認の内容がありません。もう一度確認してください"));
        };
        if token.workflow_id != args.workflow_id {
            return Err(err(IpcErrorCode::Rejected, "別のワークフローの確認です。もう一度確認してください"));
        }
        if !token.not_passed.is_empty() && !args.accept_not_passed {
            return Err(err(IpcErrorCode::Rejected, "合格していない成果を含むため、同意のチェックが必要です"));
        }
        let p = token.prepared;
        let Some(_guard) = self.gitprep_rt.begin(&p.folder) else { return Err(busy_error()) };
        // 実行の直前に、ワークフローの状態（完了している・削除の途中でない）をもう一度確かめる。
        let wf = self.org_load_wf(&args.workflow_id).await?;
        if !wf.finished() || wf.delete_pending {
            return Err(err(IpcErrorCode::Rejected, "ワークフローの状態が変わったため、取り込みませんでした。もう一度確認してください"));
        }
        let git = self.git_for_worktree();
        let repo = PathBuf::from(&p.folder);
        let record_id = self.local_id("imp").0;
        let begin = LedgerRow {
            schema_version: LEDGER_SCHEMA_VERSION,
            kind: LedgerKind::Begin,
            id: record_id.clone(),
            at: now_ms(),
            folder: Some(p.folder.clone()),
            branch: Some(p.branch.clone()),
            before: Some(p.head.clone()),
            result: Some(p.result.clone()),
            tasks: p.tasks.clone(),
            outcome: None,
            undo_outcome: None,
        };
        let dir = wf.dir.clone();
        let out = gi::execute(&git, &repo, &p, || append_ledger(&dir, &begin)).await;
        match out {
            gi::ExecOutcome::Refused(m) => Err(err(IpcErrorCode::Rejected, m)),
            gi::ExecOutcome::BeginFailed(m) => Err(err(IpcErrorCode::Io, format!("取り込みの記録を書けないため、何も変更していません（{m}）"))),
            gi::ExecOutcome::Done(outcome) => {
                let end = LedgerRow { kind: LedgerKind::End, at: now_ms(), outcome: Some(outcome.clone()), ..begin.clone() };
                if let Err(m) = append_ledger(&wf.dir, &end) {
                    // 結果は画面に返す。台帳は `begin` だけが残るので、次に開いたときHEADで見分ける。
                    crate::diag::log("org", &format!("import ledger end failed: {m}"));
                }
                self.log_git_op(serde_json::json!({
                    "kind": "orgImport",
                    "folder": p.folder,
                    "branch": p.branch,
                    "workflow": wf.id,
                    "before": p.head,
                    "result": p.result,
                    "tasks": p.tasks,
                    "outcome": &outcome,
                }));
                if import_changes_ledger(&outcome) {
                    self.mutate(|_| ((), vec![HostEvent::OrgUpdated { workflow_id: wf.id.clone() }]));
                }
                let files = matches!(outcome, OrgImportOutcome::Applied { .. }).then_some(p.files.len() as u32);
                Ok(OrgImportResult { record_id, outcome, files, folder: p.folder, branch: p.branch })
            }
        }
    }

    fn org_find_record(&self, wf: &Wf, record_id: &str) -> Result<LedgerRecord, IpcError> {
        read_ledger(&wf.dir).into_iter().find(|r| r.id == record_id).ok_or_else(|| err(IpcErrorCode::NotFound, "取り込みの記録が見つかりません"))
    }

    /// 結果が確認できなかった取り込みの確認（読取りのみ。同じ分類をやり直す。再実行しない）。
    pub async fn org_import_verify(self: &Arc<Self>, args: OrgImportRecordArgs) -> Result<OrgImportResult, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let rec = self.org_find_record(&wf, &args.record_id)?;
        let git = self.git_for_worktree();
        let head = gi::head_now(&git, Path::new(&rec.folder)).await;
        let outcome = match &rec.outcome {
            Some(o @ (OrgImportOutcome::Applied { .. } | OrgImportOutcome::NotApplied { .. } | OrgImportOutcome::Partial { .. })) => o.clone(),
            _ => reconcile_import(&rec.before, &rec.result, head.as_deref()),
        };
        Ok(OrgImportResult { record_id: rec.id, outcome, files: None, folder: rec.folder, branch: rec.branch })
    }

    /// 取り込み前に戻せるかの確認（読取りのみ）。
    pub async fn org_import_undo_preview(self: &Arc<Self>, args: OrgImportRecordArgs) -> Result<OrgUndoPreview, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let rec = self.org_find_record(&wf, &args.record_id)?;
        let git = self.git_for_worktree();
        let repo = PathBuf::from(&rec.folder);
        let head = gi::head_now(&git, &repo).await;
        let state = record_state(&rec, head.as_deref());
        let (d, _) = gi::undo_probe(&git, &repo, &rec, state).await.map_err(|e| err(IpcErrorCode::Io, format!("状態を確認できません: {}", wt_message(&e))))?;
        Ok(OrgUndoPreview {
            allowed: d.allowed,
            refusal: d.refusal,
            mode: d.mode,
            files: d.files,
            before: rec.before,
            head: head.clone().map_or(Known::NotFetched, Known::direct),
            expected_head: head.unwrap_or_default(),
        })
    }

    /// 取り込み前に戻す（確認ダイアログの後だけ）。条件が外れていれば何もしない。
    pub async fn org_import_undo(self: &Arc<Self>, args: OrgUndoArgs, _confirmed: &UserConfirmed) -> Result<OrgUndoResult, IpcError> {
        let wf = self.org_load_wf(&args.workflow_id).await?;
        let rec = self.org_find_record(&wf, &args.record_id)?;
        let Some(_guard) = self.gitprep_rt.begin(&rec.folder) else { return Err(busy_error()) };
        let git = self.git_for_worktree();
        let repo = PathBuf::from(&rec.folder);
        let head = gi::head_now(&git, &repo).await;
        let state = record_state(&rec, head.as_deref());
        let begin = LedgerRow {
            schema_version: LEDGER_SCHEMA_VERSION,
            kind: LedgerKind::UndoBegin,
            id: rec.id.clone(),
            at: now_ms(),
            folder: None,
            branch: None,
            before: None,
            result: None,
            tasks: Vec::new(),
            outcome: None,
            undo_outcome: None,
        };
        let dir = wf.dir.clone();
        match gi::undo_execute(&git, &repo, &rec, state, &args.expected_head, || append_ledger(&dir, &begin)).await {
            gi::UndoExec::Refused(m) => Err(err(IpcErrorCode::Rejected, m)),
            gi::UndoExec::BeginFailed(m) => Err(err(IpcErrorCode::Io, format!("戻す記録を書けないため、何も変更していません（{m}）"))),
            gi::UndoExec::Done(outcome) => {
                let end = LedgerRow { kind: LedgerKind::UndoEnd, at: now_ms(), undo_outcome: Some(outcome.clone()), ..begin };
                if let Err(m) = append_ledger(&wf.dir, &end) {
                    crate::diag::log("org", &format!("undo ledger end failed: {m}"));
                }
                self.log_git_op(serde_json::json!({ "kind": "orgImportUndo", "folder": rec.folder, "record": rec.id, "before": rec.before, "outcome": &outcome }));
                self.mutate(|_| ((), vec![HostEvent::OrgUpdated { workflow_id: wf.id.clone() }]));
                Ok(OrgUndoResult { outcome })
            }
        }
    }
}

/// 取り込みの結果が台帳（レビュー待ちの件数など）を変えたか。変えたときだけ画面へ再取得の合図を出す。
fn import_changes_ledger(o: &OrgImportOutcome) -> bool {
    matches!(o, OrgImportOutcome::Applied { .. } | OrgImportOutcome::Partial { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_applied_or_partial_imports_signal_a_refresh() {
        assert!(import_changes_ledger(&OrgImportOutcome::Applied { head: "h".into() }));
        assert!(import_changes_ledger(&OrgImportOutcome::Partial { files: Vec::new() }));
        assert!(!import_changes_ledger(&OrgImportOutcome::NotApplied { reason: "x".into() }));
        assert!(!import_changes_ledger(&OrgImportOutcome::Unknown { head_now: Known::NotFetched }));
    }
    use crate::backend::model::UnixMillis;
    use std::io::Write;

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentdock-orgart-{}-{}", std::process::id(), now_ms().0));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn prepared() -> gi::Prepared {
        gi::Prepared { folder: "C:/r".into(), branch: "main".into(), head: "H".into(), result: "R".into(), tasks: vec![LedgerTask { task: "t1".into(), commit: "c".into() }], files: Vec::new() }
    }

    #[test]
    fn a_confirmation_token_works_once_and_expires() {
        let rt = ArtifactRuntime::default();
        let tok = |created_ms| ImportToken { workflow_id: "wf".into(), prepared: prepared(), not_passed: Vec::new(), created_ms };
        rt.put("a".into(), tok(now_ms().0));
        assert!(rt.take("a").is_some());
        assert!(rt.take("a").is_none(), "a second use needs a new confirmation");
        rt.put("old".into(), tok(now_ms().0 - TOKEN_TTL_MS - 1));
        assert!(rt.take("old").is_none(), "expired");
        assert!(rt.take("missing").is_none());
        // 新しい控えを入れるとき、期限切れは掃除される。
        rt.put("old2".into(), tok(now_ms().0 - TOKEN_TTL_MS - 5));
        rt.put("fresh".into(), tok(now_ms().0));
        assert!(rt.tokens.lock().unwrap().get("old2").is_none());
    }

    #[test]
    fn the_ledger_is_append_only_and_unreadable_lines_are_skipped() {
        let dir = temp();
        assert!(read_ledger(&dir).is_empty(), "no ledger yet");
        let row = |kind, id: &str| LedgerRow {
            schema_version: LEDGER_SCHEMA_VERSION,
            kind,
            id: id.into(),
            at: UnixMillis(1),
            folder: Some("C:/r".into()),
            branch: Some("main".into()),
            before: Some("H".into()),
            result: Some("R".into()),
            tasks: vec![LedgerTask { task: "t1".into(), commit: "c".into() }],
            outcome: None,
            undo_outcome: None,
        };
        append_ledger(&dir, &row(LedgerKind::Begin, "i1")).unwrap();
        append_ledger(&dir, &LedgerRow { outcome: Some(OrgImportOutcome::Applied { head: "R".into() }), ..row(LedgerKind::End, "i1") }).unwrap();
        std::fs::OpenOptions::new().append(true).open(dir.join(IMPORTS_FILE)).unwrap().write_all(b"{ torn").unwrap();
        append_ledger(&dir, &row(LedgerKind::Begin, "i2")).unwrap();
        let recs = read_ledger(&dir);
        assert_eq!(recs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["i1", "i2"], "the torn line did not hide the next row");
        assert_eq!(record_state(&recs[0], Some("Z")), OrgImportState::Applied);
        assert_eq!(record_state(&recs[1], Some("R")), OrgImportState::Applied, "a begin without an end is settled by the current HEAD");
        assert_eq!(applied_tasks(&recs, Some("Z")), HashSet::from(["t1".to_string()]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

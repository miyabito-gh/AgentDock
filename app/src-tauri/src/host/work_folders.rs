//! 作業フォルダ（複数）のホスト側（WF-3、`app/DESIGN_WORKFOLDERS.md`）。
//!
//! - 保存: `next_folders`（次の送信から）・`accepted_folders`（送信記録。受理確認済みだけ）・`unconfirmed_folders`（受理不明の送信に含まれた指定）。
//! - 送信: 新しいturnにだけ `work_folders` を付ける（追加指示には付けない）。空の並びは付けない（送らない）。送信直前に存在を再確認し、
//!   消えていたら黙って外さず送らない。
//! - 受理不明: `next_folders` は残す（無条件に再送しない。照合で受理と確定したときだけ送信記録へ、受理なしなら捨てる）。
//! - `~/.codex`・config.toml・rolloutは触らない。読取りのためのresumeもしない（`read_work_folders` は読込み済みのときだけ読む）。

use std::sync::Arc;
use std::time::Duration;

use super::{blocked, err, Host};
use crate::backend::backend::{AiBackend, BackendError, ProjectSummary};
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::rules::queue as q;
use crate::rules::work_folders::{check_folder, is_same_or_under, normalize_path, os_probe, path_key, verify_folder_set, FolderEnv, PathFacts, SetReject};
use crate::store::records::ChatLocalFile;

/// プロジェクト機能の一時停止スイッチ（2026-10-14。UI側 `src/ui/features.ts` の `PROJECTS_ENABLED` と合わせる）。
/// false の間は作業フォルダ帯のためにも project/list を呼ばない。
const PROJECTS_ENABLED: bool = false;

// ───────────────────────────── 純粋ロジック ─────────────────────────────

fn key_of(p: &str) -> String {
    path_key(&normalize_path(p).unwrap_or_else(|| p.trim().to_string())).trim_end_matches('\\').to_string()
}

/// 2つの並びが同じか（順序も含む。大文字小文字・区切り・`\\?\` の違いは無視）。
pub fn same_list(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| key_of(x) == key_of(y))
}

/// 新しいturnのときだけ、保存してある「次の送信から」の並びを送信に付ける。追加指示・未設定・空の並びは付けない（空配列を送らない）。
pub fn folders_for_send(is_new_turn: bool, next: Option<&[String]>) -> Option<WorkFolders> {
    if !is_new_turn {
        return None;
    }
    next.filter(|n| !n.is_empty() && n.iter().all(|f| !f.trim().is_empty())).map(|n| WorkFolders { folders: n.to_vec() })
}

/// 送る直前の再確認（存在・フォルダ・UNC・ネットワークドライブ）。UIの確認結果や保存時の判定を信用しない。
pub fn recheck_folders(folders: &WorkFolders, probe: &dyn Fn(&str) -> PathFacts) -> Result<(), String> {
    for f in &folders.folders {
        let Some(n) = normalize_path(f) else { return Err(format!("作業フォルダを絶対パスとして扱えないため送りません: {f}")) };
        let facts = probe(&n);
        if facts.remote {
            return Err(format!("作業フォルダがネットワークドライブのため送りません: {n}"));
        }
        if !facts.exists || !facts.is_dir {
            return Err(format!("作業フォルダが見つからないため送りません（外して送ることはしません）: {n}"));
        }
    }
    Ok(())
}

/// 受理を確認した送信の反映。送信記録を更新し、送った値と同じ `next_folders`・`next_cwd` だけを外す。
pub fn after_accept(f: &mut ChatLocalFile, sent: &WorkFolders, at: UnixMillis) {
    f.accepted_folders = Some(AcceptedFolders { folders: sent.clone(), accepted_at: at });
    f.unconfirmed_folders = None;
    if f.next_folders.as_deref().is_some_and(|n| same_list(n, &sent.folders)) {
        f.next_folders = None;
    }
    if let (Some(first), Some(next_cwd)) = (sent.folders.first(), f.next_cwd.as_deref()) {
        if key_of(first) == key_of(next_cwd) {
            f.next_cwd = None;
        }
    }
}

/// 受理不明の送信に含まれた指定を記録する（`next_folders` は残す）。
pub fn mark_unconfirmed(f: &mut ChatLocalFile, sent: &WorkFolders) {
    f.unconfirmed_folders = Some(sent.folders.clone());
}

/// 照合で受理と確定した。記録してあった指定を送信記録へ移す。移したら `Some`（作業フォルダの現在値の更新に使う）。
pub fn promote_unconfirmed(f: &mut ChatLocalFile, at: UnixMillis) -> Option<WorkFolders> {
    let sent = WorkFolders { folders: f.unconfirmed_folders.clone()? };
    after_accept(f, &sent, at);
    Some(sent)
}

/// 照合で受理なしが確定・明示拒否された。受理不明の指定を捨てる（`next_folders` は残す。再送はユーザー操作のときだけ）。
pub fn discard_unconfirmed(f: &mut ChatLocalFile) {
    f.unconfirmed_folders = None;
}

/// プライマリを変えるときに引き継ぐ追加フォルダ（次の送信用があればそれ、なければ送信記録）。先頭（旧プライマリ）は除く。
pub fn carried_extras(f: Option<&ChatLocalFile>) -> Vec<String> {
    let Some(f) = f else { return Vec::new() };
    let list = f.next_folders.clone().or_else(|| f.accepted_folders.as_ref().map(|a| a.folders.folders.clone()));
    list.map(|l| l.into_iter().skip(1).collect()).unwrap_or_default()
}

/// 分岐先へ引き継ぐ作業フォルダの並び。分岐元の「次の送信から使う値」があればそれ、なければ読込み済みの直接取得、なければ送信記録（受理済み）。
/// 追加フォルダ（2件以上）があるときだけ（プライマリだけなら cwd の継承に任せる）。
pub fn pick_fork_folders(next: Option<Vec<String>>, direct: Option<Vec<String>>, accepted: Option<Vec<String>>) -> Option<Vec<String>> {
    next.or(direct).or(accepted).filter(|l| l.len() >= 2)
}

/// `[cwd] ＋ 追加分`。cwdと同じもの・重複は除く。
pub fn compose_with_primary(cwd: &str, extras: &[String]) -> Vec<String> {
    let mut out = vec![cwd.trim().to_string()];
    for e in extras {
        if !out.iter().any(|o| key_of(o) == key_of(e)) {
            out.push(e.clone());
        }
    }
    out
}

fn reject_text(r: crate::backend::model::FolderReject) -> &'static str {
    use crate::backend::model::FolderReject::*;
    match r {
        Unc => "ネットワーク共有（UNC）のフォルダは追加できません",
        NetworkDrive => "ネットワークドライブのフォルダは追加できません",
        NotAbsolute => "絶対パスではありません",
        NotFound => "フォルダが見つかりません",
        NotDirectory => "フォルダではありません",
        Duplicate => "すでに並んでいるフォルダです",
        SystemFolder => "Windows・Program Files 以下のフォルダは追加できません",
        AppDataOfAgentDock => "AgentDockのデータ領域は追加できません",
        CodexHome => "~/.codex 以下のフォルダは追加できません",
        TooMany => "作業フォルダは8件（プライマリ含む）までです",
    }
}

/// 集合の再検査の失敗をユーザー向けの文にする。
pub fn set_reject_message(folders: &[String], r: &SetReject) -> String {
    match r {
        SetReject::Empty => "作業フォルダ（プライマリ）がありません".into(),
        SetReject::Rejected { index, reason } => format!("{}: {}", reject_text(*reason), folders.get(*index).map(String::as_str).unwrap_or("")),
        SetReject::StrongUnconfirmed { index, .. } => {
            format!("このフォルダ全体を許可する確認が済んでいません: {}", folders.get(*index).map(String::as_str).unwrap_or(""))
        }
    }
}

/// プライマリが、プロジェクトのフォルダと同じか、その配下か。
fn covers_primary(primary: &str, root: &str) -> bool {
    match (normalize_path(primary), normalize_path(root)) {
        (Some(p), Some(r)) => is_same_or_under(&p, &r),
        _ => key_of(primary) == key_of(root),
    }
}

/// プロジェクトとの一致（推定）。チャットのプロジェクトID優先、なければプライマリがプロジェクトのフォルダに含まれるもの。印（`inferred`）は常に付ける。
pub fn match_project(projects: &[ProjectSummary], project_id: Option<&str>, known: &[String]) -> Option<ProjectFolderMatch> {
    let primary = known.first()?;
    let proj = project_id
        .and_then(|id| projects.iter().find(|p| p.id == id))
        .or_else(|| projects.iter().filter(|p| p.roots.iter().any(|r| covers_primary(primary, r))).min_by_key(|p| p.position))?;
    let have: Vec<String> = known.iter().map(|k| key_of(k)).collect();
    let (matched, missing): (Vec<&String>, Vec<&String>) = proj.roots.iter().partition(|r| have.contains(&key_of(r)));
    Some(ProjectFolderMatch { project_name: proj.name.clone(), matched: matched.len() as u32, total: proj.roots.len() as u32, missing: missing.into_iter().cloned().collect(), inferred: true })
}

// ───────────────────────────── ホスト ─────────────────────────────

impl Host {
    /// 次の送信から使う作業フォルダ（保存してあるものだけ）。
    pub(super) fn next_work_folders(&self, chat: &ChatKey) -> Option<Vec<String>> {
        self.read(|d| d.locals.get(chat).and_then(|l| l.next_folders.clone()))
    }

    /// 送信に付ける作業フォルダ。新しいturnのときだけ（追加指示には付けない）。
    pub(super) fn work_folders_for_send(&self, chat: &ChatKey, is_new_turn: bool) -> Option<WorkFolders> {
        folders_for_send(is_new_turn, self.next_work_folders(chat).as_deref())
    }

    /// 送信の直前の再確認。OSへの問い合わせ（ネットワークドライブで待たされ得る）は専用スレッドで行う。
    pub(super) async fn recheck_send_folders(&self, folders: &WorkFolders) -> Result<(), String> {
        let f = folders.clone();
        match tokio::task::spawn_blocking(move || recheck_folders(&f, &os_probe)).await {
            Ok(r) => r,
            Err(_) => Err("作業フォルダの確認を完了できないため送りません".into()),
        }
    }

    /// 作業フォルダ付きで受理された。チャットの作業フォルダ（プライマリ）を更新し、送信記録を更新する。
    pub(super) fn apply_folders_after_accept(self: &Arc<Self>, chat: &ChatKey, sent: WorkFolders) {
        let Some(first) = sent.folders.first().cloned() else { return };
        self.apply_cwd_after_accept(chat, first);
        let now = super::now_ms();
        self.update_local(chat, true, Duration::ZERO, |f| after_accept(f, &sent, now));
    }

    /// 作業フォルダ付きの送信が受理不明になった。
    pub(super) fn note_folders_unconfirmed(self: &Arc<Self>, chat: &ChatKey, sent: &WorkFolders) {
        self.update_local(chat, true, Duration::ZERO, |f| mark_unconfirmed(f, sent));
    }

    /// 作業フォルダ付きの送信が受理されなかった（明示拒否・照合で受理なし）。
    pub(super) fn note_folders_not_accepted(self: &Arc<Self>, chat: &ChatKey) {
        if self.read(|d| d.locals.get(chat).is_some_and(|l| l.unconfirmed_folders.is_some())) {
            self.update_local(chat, true, Duration::ZERO, discard_unconfirmed);
        }
    }

    /// 照合で受理と確定した。受理不明のときに記録した指定があれば、送信記録へ移す。
    pub(super) fn promote_folders_after_reconcile(self: &Arc<Self>, chat: &ChatKey) {
        let pending = self.read(|d| d.locals.get(chat).and_then(|l| l.unconfirmed_folders.clone()));
        if let Some(folders) = pending {
            self.apply_folders_after_accept(chat, WorkFolders { folders });
        }
    }

    /// 次のturnから使う作業フォルダの並びの変更（M44の拡張）。作業中・停止未確認なら止め、送信待ちがあれば確認を求める。
    /// UIの確認結果は信用せず、ここで同じ検査を再実行する。
    pub async fn set_next_work_folders(self: &Arc<Self>, args: SetNextWorkFoldersArgs) -> Result<SettingsImpact, IpcError> {
        if self.visual_bound(&args.chat)? {return Err(err(IpcErrorCode::Blocked,"Visual Editor chat must keep its isolated copy as cwd"));}
        self.check_not_org_owned(&args.chat)?;
        let info = self.read(|d| {
            d.chat(&args.chat).map(|c| {
                let busy = d.agents.iter().any(|v| v.agent.chat == args.chat && d.running_turn.contains_key(&v.agent.key)) || d.open_stop(&args.chat).is_some();
                (c.kind, busy)
            })
        });
        let Some((kind, busy)) = info else { return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません")) };
        if kind == ChatKind::General {
            return Err(err(IpcErrorCode::InvalidArgs, "一般チャットの作業フォルダは変更できません"));
        }
        let busy = busy || self.send_unsettled(&args.chat);
        let queue = self.read(|d| d.queues.get(&args.chat).map(|f| f.queue.clone())).unwrap_or_else(|| q::new_queue(args.chat.clone()));
        match q::check_cwd_change(&queue, busy, args.queue_retarget_confirmed) {
            q::CwdChangeCheck::Busy => {
                return Err(blocked(BlockedReason::ChatBusy, "作業中または停止未確認のため、作業フォルダは変更できません。完了・停止を確認してから変更してください"))
            }
            q::CwdChangeCheck::NeedsQueueConfirmation { waiting } => {
                return Err(blocked(BlockedReason::QueueRetargetUnconfirmed { waiting }, format!("送信待ちの依頼 {waiting} 件も、新しいフォルダが対象になります")))
            }
            q::CwdChangeCheck::Allowed => {}
        }
        // 追加フォルダ（experimental）が使えない接続では、複数の指定を保存しない（送信時に黙って捨てない）。
        if args.folders.len() > 1 && self.backend.capabilities().workspace_roots != Support::Supported {
            return Err(blocked(
                BlockedReason::CapabilityUnsupported { capability: "workspace_roots".into() },
                "この Codex 接続では追加の作業フォルダを使えません（experimental API が無効または非対応）。プライマリだけにしてください",
            ));
        }
        let (folders, confirmed) = (args.folders.clone(), args.confirmed_strong.clone());
        let verified = tokio::task::spawn_blocking(move || verify_folder_set(&folders, &confirmed, &FolderEnv::from_os(), &os_probe))
            .await
            .map_err(|_| err(IpcErrorCode::Io, "作業フォルダの確認を完了できませんでした"))?;
        let list = match verified {
            Ok(w) => w.folders,
            Err(r) => return Err(err(IpcErrorCode::InvalidArgs, set_reject_message(&args.folders, &r))),
        };
        self.precheck_space()?;
        let primary = list[0].clone();
        self.update_local(&args.chat, true, Duration::ZERO, |f| {
            f.next_cwd = Some(primary);
            f.next_folders = Some(list);
        });
        Ok(self.settings_impact(&args.chat))
    }

    /// `set_chat_cwd`（プライマリだけの変更）。追加フォルダは引き継ぐ（接続が追加フォルダを使えないときは外す）。
    pub async fn set_chat_cwd(self: &Arc<Self>, args: SetChatCwdArgs) -> Result<SettingsImpact, IpcError> {
        self.check_not_org_owned(&args.chat)?;
        let usable = self.backend.capabilities().workspace_roots == Support::Supported;
        let extras = if usable { self.read(|d| carried_extras(d.locals.get(&args.chat))) } else { Vec::new() };
        let folders = compose_with_primary(&args.cwd, &extras);
        self.set_next_work_folders(SetNextWorkFoldersArgs { chat: args.chat, folders, confirmed_strong: args.confirmed_strong, queue_retarget_confirmed: args.queue_retarget_confirmed }).await
    }

    /// 作業フォルダの確認表示。読込み済みのときだけ直接取得し（resumeしない）、送信記録・受理不明・次の送信から・プロジェクト一致を区別して返す。
    pub async fn get_work_folders(self: &Arc<Self>, args: GetWorkFoldersArgs) -> Result<WorkFoldersView, IpcError> {
        let (local, project_id, cwd) = self.read(|d| {
            (d.locals.get(&args.chat).cloned(), d.chat(&args.chat).and_then(|c| c.project_id.clone()), d.chat(&args.chat).and_then(|c| c.cwd.value().cloned()))
        });
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        let support = self.backend.capabilities().workspace_roots;
        let current = match support {
            Support::Supported => match self.backend.read_work_folders(&args.chat).await {
                Ok(k) => k,
                // 通信断などは「未確認」（0件や空にしない）。
                Err(BackendError::Unsupported { .. }) => Known::Unsupported,
                Err(_) => Known::NotFetched,
            },
            Support::Unsupported | Support::Experimental => Known::Unsupported,
            Support::Unknown => Known::NotFetched,
        };
        // 降格が起きたかもしれないので、能力は取得後にもう一度読む。
        let support = self.backend.capabilities().workspace_roots;
        let last_accepted = local.as_ref().and_then(|l| l.accepted_folders.clone());
        let known: Vec<String> = match (&current, &last_accepted, &cwd) {
            (Known::Value { value, .. }, _, _) => value.folders.clone(),
            (_, Some(a), _) => a.folders.folders.clone(),
            (_, _, Some(c)) => vec![c.clone()],
            _ => Vec::new(),
        };
        let project_match = if PROJECTS_ENABLED && support == Support::Supported && !known.is_empty() {
            match self.backend.list_projects().await {
                Ok(p) => match_project(&p, project_id.as_deref(), &known),
                Err(_) => None,
            }
        } else {
            None
        };
        Ok(WorkFoldersView {
            current,
            last_accepted,
            unconfirmed: local.as_ref().and_then(|l| l.unconfirmed_folders.clone()).map(|folders| WorkFolders { folders }),
            next: local.as_ref().and_then(|l| l.next_folders.clone()).map(|folders| WorkFolders { folders }),
            project_match,
            support,
        })
    }

    /// 分岐先へ、分岐元の作業フォルダ一式を「次の送信から使う値」として引き継ぐ（thread/fork は追加フォルダを継承しない。実測M6）。
    /// 既存の検査（UNC・システム・強い確認など）を通らない並びは、確認なしに引き継がず、外して送ることもしない（理由を返す）。
    /// 接続が追加フォルダを使えないときも引き継がず、そのことを返す。分岐先に既に値があれば何もしない。
    pub(super) async fn inherit_fork_folders(self: &Arc<Self>, src: &ChatKey, dst: &ChatKey) -> Option<String> {
        let (next, accepted, dst_has) = self.read(|d| {
            (
                d.locals.get(src).and_then(|l| l.next_folders.clone()),
                d.locals.get(src).and_then(|l| l.accepted_folders.as_ref().map(|a| a.folders.folders.clone())),
                d.locals.get(dst).is_some_and(|l| l.next_folders.is_some()),
            )
        });
        if dst_has {
            return None;
        }
        let supported = self.backend.capabilities().workspace_roots == Support::Supported;
        // 次の送信用の値がなければ、読込み済みのときだけ直接取得する（resumeしない）。
        let direct = if next.is_none() && supported {
            match self.backend.read_work_folders(src).await {
                Ok(Known::Value { value, .. }) => Some(value.folders),
                _ => None,
            }
        } else {
            None
        };
        let list = pick_fork_folders(next, direct, accepted)?;
        if !supported {
            return Some("分岐元には追加の作業フォルダがありますが、この接続では使えないため引き継ぎませんでした（プライマリだけの分岐です）".into());
        }
        let l2 = list.clone();
        let verified = tokio::task::spawn_blocking(move || verify_folder_set(&l2, &[], &FolderEnv::from_os(), &os_probe)).await;
        match verified {
            Ok(Ok(w)) => {
                self.update_local(dst, true, Duration::ZERO, |f| f.next_folders = Some(w.folders));
                None
            }
            Ok(Err(r)) => Some(format!("分岐元の追加の作業フォルダを引き継げませんでした（{}）。「作業フォルダ」から指定し直してください", set_reject_message(&list, &r))),
            Err(_) => Some("分岐元の追加の作業フォルダを確認できず、引き継ぎませんでした。「作業フォルダ」から指定し直してください".into()),
        }
    }

    /// 追加前の1件の検査（読取りのみ）。
    pub async fn check_work_folder(&self, args: CheckWorkFolderArgs) -> Result<FolderCheck, IpcError> {
        tokio::task::spawn_blocking(move || check_folder(&args.path, &args.current, &FolderEnv::from_os(), &os_probe))
            .await
            .map_err(|_| err(IpcErrorCode::Io, "作業フォルダの確認を完了できませんでした"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{FolderReject, StrongReason};

    fn v(x: &[&str]) -> Vec<String> {
        x.iter().map(|s| s.to_string()).collect()
    }
    fn local() -> ChatLocalFile {
        ChatLocalFile::new(LocalId("d".into()), None)
    }
    fn ok_dir(_: &str) -> PathFacts {
        PathFacts { exists: true, is_dir: true, remote: false }
    }

    #[test]
    fn send_gets_folders_only_for_a_new_turn_and_never_an_empty_list() {
        let n = v(&["C:\\a", "C:\\b"]);
        assert_eq!(folders_for_send(true, Some(&n)), Some(WorkFolders { folders: n.clone() }));
        assert_eq!(folders_for_send(false, Some(&n)), None, "steer never carries folders");
        assert_eq!(folders_for_send(true, None), None, "unset means omitted (server keeps its roots)");
        assert_eq!(folders_for_send(true, Some(&[])), None, "empty is never sent");
        assert_eq!(folders_for_send(true, Some(&v(&["C:\\a", " "]))), None);
    }

    #[test]
    fn accepted_send_records_and_clears_only_the_same_pending_value() {
        let mut f = local();
        f.next_folders = Some(v(&["C:\\a", "C:\\b"]));
        f.next_cwd = Some("c:/a".into());
        f.unconfirmed_folders = Some(v(&["C:\\old"]));
        after_accept(&mut f, &WorkFolders { folders: v(&["C:\\A", "C:\\b"]) }, UnixMillis(5));
        assert_eq!(f.accepted_folders.as_ref().map(|a| (a.accepted_at, a.folders.folders.clone())), Some((UnixMillis(5), v(&["C:\\A", "C:\\b"]))));
        assert!(f.next_folders.is_none() && f.next_cwd.is_none() && f.unconfirmed_folders.is_none());
        // 送信中に別の値へ変えられていたら、その値は残す（取り違えない）。
        let mut g = local();
        g.next_folders = Some(v(&["C:\\a", "C:\\c"]));
        after_accept(&mut g, &WorkFolders { folders: v(&["C:\\a", "C:\\b"]) }, UnixMillis(6));
        assert_eq!(g.next_folders, Some(v(&["C:\\a", "C:\\c"])));
        assert!(g.accepted_folders.is_some());
    }

    #[test]
    fn unknown_keeps_the_next_value_and_is_resolved_only_by_reconcile() {
        let mut f = local();
        f.next_folders = Some(v(&["C:\\a", "C:\\b"]));
        mark_unconfirmed(&mut f, &WorkFolders { folders: v(&["C:\\a", "C:\\b"]) });
        assert_eq!(f.next_folders, Some(v(&["C:\\a", "C:\\b"])), "no automatic clearing and no re-send decision here");
        assert!(f.accepted_folders.is_none(), "an unconfirmed send is not recorded as accepted");
        // 受理なし: 指定を捨てるが次の送信から使う値は残す。
        let mut n = f.clone();
        discard_unconfirmed(&mut n);
        assert!(n.unconfirmed_folders.is_none() && n.next_folders.is_some() && n.accepted_folders.is_none());
        // 受理と確定: 送信記録へ。
        let sent = promote_unconfirmed(&mut f, UnixMillis(9)).unwrap();
        assert_eq!(sent.folders, v(&["C:\\a", "C:\\b"]));
        assert!(f.accepted_folders.is_some() && f.next_folders.is_none() && f.unconfirmed_folders.is_none());
        assert_eq!(promote_unconfirmed(&mut f, UnixMillis(10)), None, "nothing to promote twice");
    }

    #[test]
    fn fork_picks_pending_then_direct_then_accepted_and_only_with_extras() {
        let (a, b, c) = (Some(v(&["C:/a", "C:/n"])), Some(v(&["C:/a", "C:/d"])), Some(v(&["C:/a", "C:/r"])));
        assert_eq!(pick_fork_folders(a.clone(), b.clone(), c.clone()), a);
        assert_eq!(pick_fork_folders(None, b.clone(), c.clone()), b);
        assert_eq!(pick_fork_folders(None, None, c.clone()), c);
        assert_eq!(pick_fork_folders(None, None, None), None);
        // プライマリだけなら引き継がない（cwd の継承に任せる）。
        assert_eq!(pick_fork_folders(Some(v(&["C:/a"])), None, c), None);
    }

    #[test]
    fn primary_change_carries_extras_and_dedupes() {
        let mut f = local();
        assert!(carried_extras(Some(&f)).is_empty());
        f.accepted_folders = Some(AcceptedFolders { folders: WorkFolders { folders: v(&["C:\\a", "C:\\b"]) }, accepted_at: UnixMillis(1) });
        assert_eq!(carried_extras(Some(&f)), v(&["C:\\b"]));
        f.next_folders = Some(v(&["C:\\a", "C:\\c", "C:\\d"]));
        assert_eq!(carried_extras(Some(&f)), v(&["C:\\c", "C:\\d"]), "the pending value wins");
        assert_eq!(compose_with_primary(" D:\\new ", &v(&["C:\\c", "d:\\NEW", "C:\\c"])), v(&["D:\\new", "C:\\c"]));
    }

    #[test]
    fn recheck_refuses_missing_or_remote_folders_without_dropping_them() {
        let w = WorkFolders { folders: v(&["C:\\a", "C:\\b"]) };
        assert!(recheck_folders(&w, &ok_dir).is_ok());
        let gone = |p: &str| if p.ends_with('b') { PathFacts { exists: false, is_dir: false, remote: false } } else { ok_dir(p) };
        assert!(recheck_folders(&w, &gone).unwrap_err().contains("C:\\b"));
        let remote = |_: &str| PathFacts { exists: true, is_dir: true, remote: true };
        assert!(recheck_folders(&w, &remote).is_err());
        assert!(recheck_folders(&WorkFolders { folders: v(&["rel"]) }, &ok_dir).is_err());
    }

    #[test]
    fn set_rejection_messages_name_the_folder() {
        let fs = v(&["C:\\a", "D:\\"]);
        assert!(set_reject_message(&fs, &SetReject::Rejected { index: 0, reason: FolderReject::Unc }).contains("C:\\a"));
        assert!(set_reject_message(&fs, &SetReject::StrongUnconfirmed { index: 1, reasons: vec![StrongReason::DriveRoot] }).contains("D:\\"));
    }

    #[test]
    fn project_match_prefers_the_project_id_and_counts_missing_roots() {
        let p = |id: &str, name: &str, roots: &[&str], position| ProjectSummary { id: id.into(), name: name.into(), roots: v(roots), position };
        let projects = vec![p("p1", "アプリ", &["C:\\a", "C:\\b", "C:\\docs"], 2), p("p2", "他", &["C:\\a"], 1)];
        let m = match_project(&projects, Some("p1"), &v(&["C:\\a", "C:\\b"])).unwrap();
        assert_eq!((m.project_name.as_str(), m.matched, m.total, m.missing.clone(), m.inferred), ("アプリ", 2, 3, v(&["C:\\docs"]), true));
        // IDが無ければプライマリを含むプロジェクト（position の小さい方）。
        assert_eq!(match_project(&projects, None, &v(&["c:/A"])).unwrap().project_name, "他");
        assert!(match_project(&projects, None, &v(&["C:\\zzz"])).is_none());
        assert!(match_project(&projects, None, &[]).is_none());
    }
}

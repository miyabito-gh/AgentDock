//! 添付・成果物の台帳（P4、要件§3.8、DESIGN_P2 §3）。
//!
//! - 台帳の正本は `HostData.locals[chat].attachments／artifacts`（保存は `chat.json`）。送信前の添付は `draft.attachments` が持つ。
//! - コピーは `attach.rs` の手順どおり。台帳への `Copying` 登録は、書込みまで終えてからコピーを始める（落ちても検出できる）。
//! - 実在確認は会話を開いたとき・起動時・開く／保存の直前。ないものは欠損として表示し、送信には使わない。
//! - 自動では外部アプリを起動しない。開く・保存はユーザー操作のコマンドからだけ。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::persist::save_failure_message;
use super::{blocked, err, now_ms, Host};
use crate::attach::{self, AttachError};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::pathref::{self, OpenPathArgs, PathInfo, PathKind, RevealPathArgs, StatPathsArgs};
use crate::store::records::ChatLocalFile;
use crate::store::{layout, StoreError};

/// 画像のアプリ内プレビューに読み込む上限。これを超えるものは「開く」で関連アプリに任せる。
pub const PREVIEW_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// 選択中のモデルが、その種別の入力に対応していないと分かっているか。入力種別が取得できていない（空）ときは分からないので false。
pub fn model_lacks_input(models: &[ModelInfo], model_id: &str, kind: AttachmentKind) -> bool {
    models.iter().find(|m| m.id == model_id).is_some_and(|m| !m.input_kinds.is_empty() && !m.input_kinds.contains(&kind))
}

fn not_ready_message(state: &AttachmentState) -> &'static str {
    match state {
        AttachmentState::Copying => "コピー中の添付があります。完了してから送信してください",
        AttachmentState::CopyFailed { .. } => "コピーに失敗した添付があります。取り外すか、もう一度添付してください",
        AttachmentState::Missing => "添付のコピーが見つかりません（欠損）。取り外して、もう一度添付してください",
        AttachmentState::Ready => "添付を確認できません",
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// 会話中のリンクを開く／表示してよいか。作業フォルダの実体の中（シンボリックリンク・ジャンクション経由の脱出は外）だけ。
/// 開く場合は実行形式も止める。確認では解除できない。理由は画面にそのまま出す。
fn workspace_link_verdict(path: &Path, cwd: Option<&str>, for_open: bool) -> Result<(), &'static str> {
    if for_open && attach::is_executable_path(path) {
        return Err("実行形式のファイルは、リンクからは開きません（パスのコピーのみできます）");
    }
    if !cwd.is_some_and(|root| attach::is_inside_project_resolved(path, root)) {
        return Err("作業フォルダの外にあるため、リンクからは開きません（パスのコピーのみできます）");
    }
    Ok(())
}

impl Host {
    // ───────────── 台帳の更新 ─────────────

    /// チャットの補足情報を更新する（なければ作る）。保存は呼び出し側が依頼する。
    /// 返したイベントがあれば、補足情報の更新（`chatLocalUpdated`）も続けて出す。
    pub(super) fn edit_local<R>(&self, chat: &ChatKey, f: impl FnOnce(&mut ChatLocalFile) -> (R, Vec<HostEvent>)) -> R {
        let dir_id = self.persist.dir_id_for(chat);
        self.mutate(|d| {
            let file = d.locals.entry(chat.clone()).or_insert_with(|| ChatLocalFile::new(dir_id, Some(chat.clone())));
            let (r, mut ev) = f(file);
            if !ev.is_empty() {
                ev.extend(d.local_view(chat).map(|local| HostEvent::ChatLocalUpdated { local }));
            }
            (r, ev)
        })
    }

    pub(super) fn save_soon(self: &Arc<Self>, chat: &ChatKey) {
        self.schedule_save(SaveScope::ChatLocal { chat: chat.clone() }, Duration::ZERO);
    }

    /// 添付を台帳へ追加または置き換える。`to_draft` なら送信前の添付にも加える。
    pub(super) fn upsert_attachment(&self, chat: &ChatKey, entry: AttachmentEntry, to_draft: bool) {
        self.edit_local(chat, |f| {
            match f.attachments.iter_mut().find(|a| a.id == entry.id) {
                Some(old) => *old = entry.clone(),
                None => f.attachments.push(entry.clone()),
            }
            if to_draft && !f.draft.attachments.contains(&entry.id) {
                f.draft.attachments.push(entry.id.clone());
                f.draft.updated_at = Some(now_ms());
            }
            ((), vec![HostEvent::AttachmentUpdated { entry }])
        });
    }

    fn drop_attachment(&self, chat: &ChatKey, id: &LocalId) {
        self.edit_local(chat, |f| {
            f.attachments.retain(|a| &a.id != id);
            f.draft.attachments.retain(|a| a != id);
            ((), vec![])
        });
    }

    /// 手順3: `Copying` を台帳へ登録し、書込みまで終える。書けなければ登録を取り消してコピーを始めない。
    fn register_attachment(&self, chat: &ChatKey, entry: &AttachmentEntry) -> Result<(), StoreError> {
        self.upsert_attachment(chat, entry.clone(), true);
        match self.write_scope(&SaveScope::ChatLocal { chat: chat.clone() }) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.drop_attachment(chat, &entry.id);
                Err(e)
            }
        }
    }

    fn attach_dir(&self, chat: &ChatKey) -> Result<PathBuf, IpcError> {
        if self.read(|d| d.chat(chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        let store = self.persist.store().ok_or_else(|| err(IpcErrorCode::Unsupported, "保存先が使えないため、添付できません"))?;
        store.ensure_chat_dir(chat).map_err(|e| err(IpcErrorCode::Io, format!("チャットの保存領域を用意できません: {e}")))
    }

    fn attach_error(&self, e: AttachError, path: &str) -> IpcError {
        match e {
            AttachError::FolderIsWorkspace => blocked(
                BlockedReason::FolderIsWorkspace { path: path.to_string() },
                "フォルダは添付できません。作業フォルダとして指定してください（フォルダの中身はコピーしません）",
            ),
            AttachError::InsufficientSpace { required, available } => {
                blocked(BlockedReason::InsufficientSpace { required, available }, save_failure_message(&StoreError::InsufficientSpace { required, available }))
            }
            AttachError::Copy(_) => err(IpcErrorCode::Io, "元のファイルを読めないため、添付できません"),
            AttachError::Register(m) => err(IpcErrorCode::Io, format!("添付を記録できないため、コピーを始めていません: {m}")),
        }
    }

    /// コピー結果を台帳へ反映して保存する。失敗した添付も台帳に残し（失敗の理由を示す）、取り外すまで残る。
    async fn finish_attachment(
        self: &Arc<Self>,
        chat: &ChatKey,
        res: Result<Result<AttachmentEntry, AttachError>, tokio::task::JoinError>,
        path: &str,
    ) -> Result<AttachmentEntry, IpcError> {
        let entry = match res {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => return Err(self.attach_error(e, path)),
            Err(e) => return Err(err(IpcErrorCode::Io, format!("コピー処理が中断されました: {e}"))),
        };
        self.upsert_attachment(chat, entry.clone(), false);
        self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await;
        Ok(entry)
    }

    // ───────────── IPC ─────────────

    /// ファイルを添付する（選択・ドロップ）。フォルダは止める。元ファイルは読むだけ。
    pub async fn add_attachment_file(self: &Arc<Self>, args: AddAttachmentFileArgs) -> Result<AttachmentEntry, IpcError> {
        let chat_dir = self.attach_dir(&args.chat)?;
        let (id, chat, source) = (self.local_id("att"), args.chat.clone(), PathBuf::from(&args.path));
        let host = self.clone();
        let res = tokio::task::spawn_blocking(move || {
            attach::copy_file_into(&chat_dir, &chat, id, &source, now_ms(), &mut |e| host.register_attachment(&chat, e))
        })
        .await;
        self.finish_attachment(&args.chat, res, &args.path).await
    }

    /// クリップボード画像（UIから生バイトで受けたPNG）を添付する。
    pub async fn add_attachment_image_bytes(self: &Arc<Self>, chat: ChatKey, name: String, bytes: Vec<u8>) -> Result<AttachmentEntry, IpcError> {
        if bytes.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "画像のデータが空です"));
        }
        let chat_dir = self.attach_dir(&chat)?;
        let (id, c) = (self.local_id("att"), chat.clone());
        let host = self.clone();
        let res = tokio::task::spawn_blocking(move || {
            attach::write_image_into(&chat_dir, &c, id, &name, &bytes, now_ms(), &mut |e| host.register_attachment(&c, e))
        })
        .await;
        self.finish_attachment(&chat, res, "").await
    }

    /// 元のチャットの添付（台帳のID）を、別のチャットへ新しいコピーとして付ける（編集して再送で添付を引き継ぐ）。
    /// 元の台帳に無い・使えない状態（コピー中・失敗・欠損）のものがあれば、その旨を返して止める（付けずに黙って進めない）。
    /// 付け終えたものは送信前の添付（下書き）に入り、新しい台帳の項目を元の順で返す。
    pub async fn carry_attachments(self: &Arc<Self>, from: &ChatKey, to: &ChatKey, ids: &[LocalId]) -> Result<Vec<AttachmentEntry>, String> {
        let mut out = Vec::new();
        for id in ids {
            let Some(src) = self.read(|d| d.locals.get(from).and_then(|f| f.attachments.iter().find(|a| &a.id == id).cloned())) else {
                return Err("元の発言の添付が台帳に見つかりません".into());
            };
            if src.state != AttachmentState::Ready {
                return Err(format!("元の添付「{}」は使えない状態です（{}）", src.display_name, not_ready_message(&src.state)));
            }
            let made = match &src.source {
                AttachmentSource::Skill { name, path } => self.add_skill_attachment(AddSkillArgs { chat: to.clone(), name: name.clone(), path: path.clone() }).await,
                _ => match src.copy_path.clone() {
                    Some(p) => self.add_attachment_file(AddAttachmentFileArgs { chat: to.clone(), path: p }).await,
                    None => return Err(format!("元の添付「{}」のコピーの場所が分かりません", src.display_name)),
                },
            };
            match made {
                Ok(e) => out.push(e),
                Err(e) => return Err(format!("添付「{}」を引き継げませんでした（{}）", src.display_name, e.message)),
            }
        }
        Ok(out)
    }

    /// 送信前の添付を取り外す。まだ使っていないコピーは消し、送信・キューに使った添付は台帳に残す（履歴の参照を保つ）。
    pub async fn remove_attachment(self: &Arc<Self>, args: AttachmentArgs) -> Result<(), IpcError> {
        let Some(entry) = self.read(|d| d.locals.get(&args.chat).and_then(|f| f.attachments.iter().find(|a| a.id == args.attachment).cloned())) else {
            return Err(err(IpcErrorCode::NotFound, "添付が見つかりません"));
        };
        if entry.state == AttachmentState::Copying {
            return Err(err(IpcErrorCode::InvalidArgs, "コピー中の添付は取り外せません。完了してからお試しください"));
        }
        let unused = entry.used_by.is_empty();
        self.edit_local(&args.chat, |f| {
            f.draft.attachments.retain(|a| a != &args.attachment);
            f.draft.updated_at = Some(now_ms());
            if unused {
                f.attachments.retain(|a| a.id != args.attachment);
            }
            ((), vec![])
        });
        if unused {
            // この添付のためのコピーだけを消す（添付ごとの専用フォルダ。元ファイル・他の添付には触れない）。
            if let Some(store) = self.persist.store() {
                let dir = layout::chat_dir(store.root(), &self.persist.dir_id_for(&args.chat))
                    .join(layout::ATTACHMENTS_DIR)
                    .join(layout::sanitize_file_name(&args.attachment.0));
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await;
            }
        }
        self.save_now(SaveScope::ChatLocal { chat: args.chat }).await;
        Ok(())
    }

    // ───────────── 実在確認 ─────────────

    /// 台帳の実在を確認し、実体のない添付を欠損にする・成果物の実在を更新する（読み取りのみ）。
    pub(super) fn spawn_recheck_files(self: &Arc<Self>, only: Option<ChatKey>) {
        let host = self.clone();
        tokio::task::spawn_blocking(move || host.recheck_files(only));
    }

    fn recheck_files(self: &Arc<Self>, only: Option<ChatKey>) {
        let chats: Vec<ChatKey> = self.read(|d| d.locals.keys().filter(|k| only.as_ref().is_none_or(|o| o == *k)).cloned().collect());
        for chat in chats {
            let (atts, arts) = self.read(|d| d.locals.get(&chat).map(|f| (f.attachments.clone(), f.artifacts.clone())).unwrap_or_default());
            let missing: Vec<LocalId> = atts
                .iter()
                .filter(|a| a.state == AttachmentState::Ready)
                .filter(|a| attach::backing_path(a).is_none_or(|p| attach::check_exists(Path::new(p)) == Known::direct(false)))
                .map(|a| a.id.clone())
                .collect();
            let checked: Vec<(LocalId, Known<bool>)> = arts.iter().map(|a| (a.id.clone(), attach::check_exists(Path::new(&a.path)))).collect();
            let now = now_ms();
            let changed = self.edit_local(&chat, |f| {
                let mut ev = Vec::new();
                for a in f.attachments.iter_mut().filter(|a| missing.contains(&a.id) && a.state == AttachmentState::Ready) {
                    a.state = AttachmentState::Missing;
                    ev.push(HostEvent::AttachmentUpdated { entry: a.clone() });
                }
                for (id, exists) in &checked {
                    if let Some(a) = f.artifacts.iter_mut().find(|a| &a.id == id) {
                        if a.exists != *exists {
                            a.exists = exists.clone();
                            a.checked_at = Some(now);
                            ev.push(HostEvent::ArtifactUpdated { entry: a.clone() });
                        }
                    }
                }
                (!ev.is_empty(), ev)
            });
            if changed {
                self.save_soon(&chat);
            }
        }
    }

    // ───────────── 成果物 ─────────────

    /// ファイル変更の候補を確認し、実在するものだけ成果物の台帳に加える（読み取りのみ）。
    pub(super) fn observe_artifact(self: &Arc<Self>, agent: AgentKey, item: ItemKey, path: String) {
        let Some((chat, cwd)) = self.read(|d| d.view(&agent).map(|v| (v.agent.chat.clone(), d.chat(&v.agent.chat).and_then(|c| c.cwd.value().cloned())))) else { return };
        let host = self.clone();
        tokio::task::spawn_blocking(move || {
            let p = Path::new(&path);
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                // 相対パスは作業フォルダ基準。作業フォルダが分からなければ、実在を確認できないので記録しない。
                match &cwd {
                    Some(c) => Path::new(c).join(p),
                    None => return,
                }
            };
            if !full.is_file() {
                return;
            }
            let path = full.to_string_lossy().into_owned();
            let in_chat_area = host.persist.store().is_some_and(|s| layout::is_inside(&layout::chat_dir(s.root(), &host.persist.dir_id_for(&chat)), &full));
            let now = now_ms();
            let id = host.local_id("art");
            host.edit_local(&chat, |f| {
                let entry = match f.artifacts.iter_mut().find(|a| same_path(&a.path, &path)) {
                    Some(old) => {
                        old.item = Some(item.clone());
                        old.exists = Known::direct(true);
                        old.checked_at = Some(now);
                        old.clone()
                    }
                    None => {
                        let e = ArtifactEntry {
                            id,
                            chat: chat.clone(),
                            agent: agent.clone(),
                            item: Some(item.clone()),
                            path,
                            observed_at: now,
                            exists: Known::direct(true),
                            checked_at: Some(now),
                            in_chat_area,
                        };
                        f.artifacts.push(e.clone());
                        e
                    }
                };
                ((), vec![HostEvent::ArtifactUpdated { entry }])
            });
            host.save_soon(&chat);
        });
    }

    // ───────────── 開く・保存 ─────────────

    /// 開く・保存・プレビューの対象のパス。実在を確認し、なければ欠損にして止める（`Err`）。
    pub async fn resolve_file(self: &Arc<Self>, target: &FileRef) -> Result<PathBuf, IpcError> {
        let (chat, path) = match target {
            FileRef::Attachment { chat, id } => {
                let e = self
                    .read(|d| d.locals.get(chat).and_then(|f| f.attachments.iter().find(|a| &a.id == id).cloned()))
                    .ok_or_else(|| err(IpcErrorCode::NotFound, "添付が見つかりません"))?;
                if e.state != AttachmentState::Ready {
                    return Err(blocked(BlockedReason::AttachmentNotReady { attachment: id.clone() }, not_ready_message(&e.state)));
                }
                (chat.clone(), e.copy_path.ok_or_else(|| err(IpcErrorCode::NotFound, "コピーの場所が分かりません"))?)
            }
            FileRef::Artifact { chat, id } => {
                let e = self
                    .read(|d| d.locals.get(chat).and_then(|f| f.artifacts.iter().find(|a| &a.id == id).cloned()))
                    .ok_or_else(|| err(IpcErrorCode::NotFound, "成果物が見つかりません"))?;
                (chat.clone(), e.path)
            }
        };
        let p = PathBuf::from(&path);
        let probe = p.clone();
        let exists = tokio::task::spawn_blocking(move || attach::check_exists(&probe)).await.unwrap_or(Known::NotFetched);
        match exists {
            Known::Value { value: true, .. } => Ok(p),
            Known::Value { value: false, .. } => {
                self.mark_file_missing(&chat, target);
                Err(err(IpcErrorCode::NotFound, "ファイルが見つかりません（欠損）。移動または削除された可能性があります"))
            }
            _ => Err(err(IpcErrorCode::Io, "ファイルの有無を確認できませんでした")),
        }
    }

    /// 開く前の確認（ユーザー操作のときだけ呼ぶ）。プロジェクト外の成果物、または実行形式は、`confirmed` でなければ止める。
    /// プロジェクトはチャットの作業フォルダ。不明なら外とみなす。添付のコピー（アプリの領域）はプロジェクト判定の対象外。
    pub fn check_open_risk(&self, target: &FileRef, path: &Path, confirmed: bool) -> Result<(), IpcError> {
        if confirmed {
            return Ok(());
        }
        let executable = attach::is_executable_path(path);
        let outside_project = match target {
            FileRef::Artifact { chat, .. } => {
                let cwd = self.read(|d| d.chats.iter().find(|c| &c.key == chat).and_then(|c| c.cwd.value().cloned()));
                !cwd.is_some_and(|root| attach::is_inside_project_resolved(path, &root))
            }
            FileRef::Attachment { .. } => false,
        };
        if !executable && !outside_project {
            return Ok(());
        }
        let path = path.to_string_lossy().into_owned();
        Err(blocked(BlockedReason::OpenNeedsConfirm { path, outside_project, executable }, "開く前に確認が必要です"))
    }

    fn mark_file_missing(self: &Arc<Self>, chat: &ChatKey, target: &FileRef) {
        let now = now_ms();
        self.edit_local(chat, |f| {
            let mut ev = Vec::new();
            match target {
                FileRef::Attachment { id, .. } => {
                    if let Some(a) = f.attachments.iter_mut().find(|a| &a.id == id && a.state == AttachmentState::Ready) {
                        a.state = AttachmentState::Missing;
                        ev.push(HostEvent::AttachmentUpdated { entry: a.clone() });
                    }
                }
                FileRef::Artifact { id, .. } => {
                    if let Some(a) = f.artifacts.iter_mut().find(|a| &a.id == id) {
                        a.exists = Known::direct(false);
                        a.checked_at = Some(now);
                        ev.push(HostEvent::ArtifactUpdated { entry: a.clone() });
                    }
                }
            }
            ((), ev)
        });
        self.save_soon(chat);
    }

    /// 名前を付けて保存。同名があり、確認がなければ `TargetExists` で止める（無断上書きしない）。
    pub async fn save_file_as(self: &Arc<Self>, args: SaveFileAsArgs) -> Result<(), IpcError> {
        let src = self.resolve_file(&args.target).await?;
        let dest = PathBuf::from(&args.dest);
        let overwrite = args.overwrite_confirmed;
        match tokio::task::spawn_blocking(move || attach::save_copy_as(&src, &dest, overwrite)).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(blocked(BlockedReason::TargetExists { path: args.dest.clone() }, "同じ名前のファイルがあります。上書きする場合は確認してください")),
            Ok(Err(e)) => Err(err(IpcErrorCode::Io, format!("保存できませんでした（保存先は変更していません）: {e}"))),
            Err(e) => Err(err(IpcErrorCode::Io, format!("保存処理が中断されました: {e}"))),
        }
    }

    /// 画像のアプリ内プレビュー用にバイト列を読む（画像だけ、大きすぎるものは返さない）。
    pub async fn read_image_preview(self: &Arc<Self>, target: &FileRef) -> Result<Vec<u8>, IpcError> {
        let path = self.resolve_file(target).await?;
        if attach::kind_for(&path) != AttachmentKind::Image {
            return Err(err(IpcErrorCode::InvalidArgs, "画像ではないため、プレビューできません"));
        }
        let read = tokio::task::spawn_blocking(move || -> std::io::Result<Option<Vec<u8>>> {
            if std::fs::metadata(&path)?.len() > PREVIEW_MAX_BYTES {
                return Ok(None);
            }
            std::fs::read(&path).map(Some)
        })
        .await;
        match read {
            Ok(Ok(Some(b))) => Ok(b),
            Ok(Ok(None)) => Err(err(IpcErrorCode::InvalidArgs, "大きいため、アプリ内ではプレビューしません。「開く」で表示してください")),
            Ok(Err(e)) => Err(err(IpcErrorCode::Io, format!("読み込めませんでした: {e}"))),
            Err(e) => Err(err(IpcErrorCode::Io, format!("読み込みが中断されました: {e}"))),
        }
    }

    // ───────────── 会話内のファイルパス（読取りのメタデータ確認だけ） ─────────────

    fn chat_cwd(&self, chat: &ChatKey) -> Option<String> {
        self.read(|d| d.chats.iter().find(|c| &c.key == chat).and_then(|c| c.cwd.value().cloned()))
    }

    /// パスの実在とメタデータだけを確認する（内容は読まない）。遅い・失敗は `Unconfirmed`。
    pub async fn stat_paths(self: &Arc<Self>, args: StatPathsArgs) -> Vec<PathInfo> {
        let cwd = self.chat_cwd(&args.chat);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(pathref::STAT_TIMEOUT_MS);
        let unconfirmed = |input: &str| PathInfo { input: input.to_string(), kind: PathKind::Unconfirmed, absolute: None, relative: None };
        let mut jobs = Vec::new();
        for (i, input) in args.paths.iter().enumerate() {
            // ネットワークパス（UNC等）は、ファイルシステムに触れず未確認で返す（認証情報の送信を避ける）。
            let resolved = (i < pathref::MAX_PATHS && input.chars().count() <= pathref::MAX_PATH_CHARS && !pathref::is_network_path(input))
                .then(|| pathref::resolve(input, cwd.as_deref()))
                .flatten()
                .filter(|abs| !pathref::is_network_path(abs));
            jobs.push(resolved.map(|abs| {
                let a = abs.clone();
                (abs, tokio::task::spawn_blocking(move || pathref::stat_blocking(&a)))
            }));
        }
        let mut out = Vec::new();
        for (input, job) in args.paths.iter().zip(jobs) {
            let Some((abs, handle)) = job else { out.push(unconfirmed(input)); continue };
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(Ok((kind, canon))) => {
                    let (absolute, relative) = if matches!(kind, PathKind::File | PathKind::Folder) {
                        let absolute = canon.unwrap_or(abs.clone());
                        let relative = cwd.as_deref().and_then(|root| pathref::relative_to(&abs, root).or_else(|| pathref::relative_to(&absolute, root)));
                        (Some(absolute), relative)
                    } else {
                        (None, None)
                    };
                    out.push(PathInfo { input: input.clone(), kind, absolute, relative });
                }
                _ => out.push(unconfirmed(input)),
            }
        }
        out
    }

    /// 会話内のパスを実在確認し、絶対パスにする。確認できなければ止める。
    async fn resolve_chat_path(self: &Arc<Self>, chat: &ChatKey, path: &str) -> Result<PathBuf, IpcError> {
        if pathref::is_network_path(path) {
            return Err(err(IpcErrorCode::InvalidArgs, "ネットワークパスは自動確認も開く操作もしません（接続するとWindowsの認証情報が相手に送信されることがあります）"));
        }
        let info = self.stat_paths(StatPathsArgs { chat: chat.clone(), paths: vec![path.to_string()] }).await.into_iter().next();
        match info {
            Some(PathInfo { kind: PathKind::File | PathKind::Folder, absolute: Some(a), .. }) => Ok(PathBuf::from(a)),
            Some(PathInfo { kind: PathKind::Missing, .. }) => Err(err(IpcErrorCode::NotFound, "ファイルが見つかりません。移動または削除された可能性があります")),
            _ => Err(err(IpcErrorCode::Io, "ファイルの有無を確認できませんでした")),
        }
    }

    /// 会話内のパスを開く前の確認。作業フォルダの外、または実行形式は、`confirmed` でなければ止める（成果物の「開く」と同じ条件）。
    pub async fn open_path_target(self: &Arc<Self>, args: &OpenPathArgs) -> Result<PathBuf, IpcError> {
        let path = self.resolve_chat_path(&args.chat, &args.path).await?;
        if args.workspace_only {
            workspace_link_verdict(&path, self.chat_cwd(&args.chat).as_deref(), true).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
            return Ok(path);
        }
        if args.risk_confirmed {
            return Ok(path);
        }
        let executable = attach::is_executable_path(&path);
        let outside_project = !self.chat_cwd(&args.chat).is_some_and(|root| attach::is_inside_project_resolved(&path, &root));
        if !executable && !outside_project {
            return Ok(path);
        }
        let shown = path.to_string_lossy().into_owned();
        Err(blocked(BlockedReason::OpenNeedsConfirm { path: shown, outside_project, executable }, "開く前に確認が必要です"))
    }

    /// エクスプローラーで表示する対象（実在確認のみ。実行はしないので確認は要らない）。
    pub async fn reveal_path_target(self: &Arc<Self>, args: &RevealPathArgs) -> Result<PathBuf, IpcError> {
        let path = self.resolve_chat_path(&args.chat, &args.path).await?;
        if args.workspace_only {
            workspace_link_verdict(&path, self.chat_cwd(&args.chat).as_deref(), false).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        }
        Ok(path)
    }

    // ───────────── 送信への受け渡し ─────────────

    /// 台帳から添付を取り出す（指定順）。
    fn ledger_entries(&self, chat: &ChatKey, ids: &[LocalId]) -> Result<Vec<AttachmentEntry>, IpcError> {
        self.read(|d| {
            let f = d.locals.get(chat);
            ids.iter()
                .map(|id| f.and_then(|f| f.attachments.iter().find(|a| &a.id == id).cloned()).ok_or_else(|| err(IpcErrorCode::NotFound, "添付が見つかりません")))
                .collect()
        })
    }

    /// 送信・キュー登録に使える添付か確認する（`Ready` かつコピーの実体がある）。実体がなければ欠損にして止める。
    pub(super) fn verify_attachments(self: &Arc<Self>, chat: &ChatKey, ids: &[LocalId]) -> Result<Vec<AttachmentEntry>, IpcError> {
        let entries = self.ledger_entries(chat, ids)?;
        let gone: Vec<LocalId> = entries
            .iter()
            .filter(|e| e.state == AttachmentState::Ready && attach::backing_path(e).is_none_or(|p| attach::check_exists(Path::new(p)) != Known::direct(true)))
            .map(|e| e.id.clone())
            .collect();
        if let Some(first) = gone.first().cloned() {
            self.edit_local(chat, |f| {
                let mut ev = Vec::new();
                for a in f.attachments.iter_mut().filter(|a| gone.contains(&a.id)) {
                    a.state = AttachmentState::Missing;
                    ev.push(HostEvent::AttachmentUpdated { entry: a.clone() });
                }
                ((), ev)
            });
            self.save_soon(chat);
            return Err(blocked(BlockedReason::AttachmentNotReady { attachment: first }, not_ready_message(&AttachmentState::Missing)));
        }
        if let Some(bad) = entries.iter().find(|e| e.state != AttachmentState::Ready) {
            return Err(blocked(BlockedReason::AttachmentNotReady { attachment: bad.id.clone() }, not_ready_message(&bad.state)));
        }
        Ok(entries)
    }

    /// 使えなかった添付の名前（停止理由の表示用）。エラーが添付を特定していなければ、指定した添付の先頭を使う。
    pub(super) fn attachment_name_for(&self, chat: &ChatKey, e: &IpcError, ids: &[LocalId]) -> String {
        let id = match &e.blocked {
            Some(BlockedReason::AttachmentNotReady { attachment }) => Some(attachment.clone()),
            _ => ids.first().cloned(),
        };
        id.and_then(|id| self.read(|d| d.locals.get(chat).and_then(|f| f.attachments.iter().find(|a| a.id == id).map(|a| a.display_name.clone()))))
            .unwrap_or_else(|| "（名前を取得できません）".to_string())
    }

    /// 送信用の添付を用意する（確認→変換→モデルの入力対応の確認）。使えない添付があれば送らず止める。
    pub(super) async fn prepare_attachments(self: &Arc<Self>, chat: &ChatKey, ids: &[LocalId]) -> Result<Vec<Attachment>, IpcError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let entries = self.verify_attachments(chat, ids)?;
        let atts = attach::to_send_attachments(&entries).map_err(|id| blocked(BlockedReason::AttachmentNotReady { attachment: id }, "添付を送信用に用意できません"))?;
        if atts.iter().any(|a| a.kind == AttachmentKind::Image) {
            if let Some(sel) = self.read(|d| d.model_settings.get(chat).and_then(|s| s.selected.clone())) {
                // 一覧を取得できないときは止めない（対応は未確認。UIも「モデルが読めるかは未確認」と表示する）。
                if let Ok(models) = self.backend.list_models(ModelQuery { include_hidden: true }).await {
                    if model_lacks_input(&models, &sel.model, AttachmentKind::Image) {
                        return Err(blocked(
                            BlockedReason::ModelLacksInput { input: AttachmentKind::Image },
                            "選択中のモデルは画像の入力に対応していません。モデルを変えるか、画像を取り外してください",
                        ));
                    }
                }
            }
        }
        Ok(atts)
    }

    /// 送信・キューに使った添付を記録する（`usedBy`）。送信前の添付（下書き）からは外す。
    pub(super) fn attachments_used(self: &Arc<Self>, chat: &ChatKey, ids: &[LocalId], by: &LocalId) {
        if ids.is_empty() {
            return;
        }
        self.edit_local(chat, |f| {
            let mut ev = Vec::new();
            for a in f.attachments.iter_mut().filter(|a| ids.contains(&a.id)) {
                if !a.used_by.contains(by) {
                    a.used_by.push(by.clone());
                }
                ev.push(HostEvent::AttachmentUpdated { entry: a.clone() });
            }
            f.draft.attachments.retain(|a| !ids.contains(a));
            f.draft.updated_at = Some(now_ms());
            ((), ev)
        });
        self.save_soon(chat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, kinds: Vec<AttachmentKind>) -> ModelInfo {
        ModelInfo { id: id.into(), display_name: id.into(), description: None, efforts: vec![], default_effort: None, is_default: false, hidden: false, input_kinds: kinds, speed_tiers: vec![], default_speed_tier: None }
    }

    #[test]
    fn lacking_input_is_only_claimed_when_the_model_lists_its_inputs() {
        let ms = vec![model("text-only", vec![AttachmentKind::File]), model("vision", vec![AttachmentKind::Image]), model("unknown", vec![])];
        assert!(model_lacks_input(&ms, "text-only", AttachmentKind::Image));
        assert!(!model_lacks_input(&ms, "vision", AttachmentKind::Image));
        assert!(!model_lacks_input(&ms, "unknown", AttachmentKind::Image), "unknown inputs are not treated as unsupported");
        assert!(!model_lacks_input(&ms, "absent", AttachmentKind::Image));
    }

    #[test]
    fn workspace_link_verdict_allows_only_inside_non_executable() {
        let root = std::env::temp_dir().join(format!("agentdock-wl-{}", std::process::id()));
        let sub = root.join("work");
        std::fs::create_dir_all(&sub).unwrap();
        let (txt, exe, out) = (sub.join("a.txt"), sub.join("run.EXE"), root.join("outside.txt"));
        for f in [&txt, &exe, &out] {
            std::fs::write(f, b"x").unwrap();
        }
        let cwd = sub.to_string_lossy().into_owned();
        assert!(workspace_link_verdict(&txt, Some(&cwd), true).is_ok());
        assert!(workspace_link_verdict(&exe, Some(&cwd), true).is_err());
        assert!(workspace_link_verdict(&exe, Some(&cwd), false).is_ok(), "revealing an executable does not run it");
        assert!(workspace_link_verdict(&out, Some(&cwd), true).is_err());
        assert!(workspace_link_verdict(&out, Some(&cwd), false).is_err());
        assert!(workspace_link_verdict(&sub.join("..").join("outside.txt"), Some(&cwd), false).is_err());
        assert!(workspace_link_verdict(&txt, None, true).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn artifact_paths_compare_without_case_or_separator_differences() {
        assert!(same_path(r"C:\W\a.txt", "c:/w/A.txt"));
        assert!(!same_path(r"C:\w\a.txt", r"C:\w\b.txt"));
    }
}

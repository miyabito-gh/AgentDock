//! 入力欄へ入れるものの用意（P3-4、`app/DESIGN_P3.md` §1 #8・#11）: 過去会話の参考指定、Skillの指定、指示ファイル（AGENTS.md）。
//!
//! - 参考指定は履歴の読取り（`thread/read`）だけ。resumeしない・参照先の会話を変更しない。抜粋は文字列で返し、入力欄への挿入と送信はユーザーが行う（要約しない）。
//! - Skillの指定は、作業フォルダのSkill一覧（読取り）から選んだものを、コピーしない参照の添付（チップ）にする。送信時に `skill` 入力として明示呼出しになる。
//!   Skillの有効・無効の切替は作らない。`~/.codex` 配下のSkill・AGENTS.mdは読むだけ。
//! - 指示ファイルの確認は、この接続で応答が示した読み込み済みのパスと実在確認だけ（読むためにresumeしない。まだなら未取得）。
//!   雛形の作成は新規作成のみ（既存があれば何も書かない）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::state::agent_key_of;
use super::{blocked, err, now_ms, Host};
use crate::attach;
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::backend::parity::*;
use crate::rules::compose;

/// 会話の作業フォルダ（取得できているときだけ）。
fn cwd_of(host: &Host, chat: &ChatKey) -> Option<String> {
    host.read(|d| d.chat(chat).and_then(|c| c.cwd.value().cloned())).filter(|c| !c.trim().is_empty())
}

impl Host {
    // ───────────── 過去会話の参考指定（#8） ─────────────

    async fn read_history_for_reference(self: &Arc<Self>, chat: &ChatKey) -> Result<AgentHistory, IpcError> {
        self.require_op(ParityOp::ReferenceChat)?;
        self.require_chat(chat)?;
        // 履歴の読取りだけ（resumeしない。アーカイブ・外部の会話も読める）。
        self
            .read_history(agent_key_of(chat), ReadOptions { include_turns: true })
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("履歴を取得できませんでした（未取得）: {e}")))
    }

    /// 参考にできるturnの一覧（読取りのみ）。取得できなければエラー（空の一覧にしない）。
    pub async fn list_reference_turns(self: &Arc<Self>, args: ChatArgs) -> Result<ReferenceTurns, IpcError> {
        let history = self.read_history_for_reference(&args.chat).await?;
        Ok(ReferenceTurns { title: self.chat_title(&args.chat), turns: compose::reference_turns(&history.turns) })
    }

    /// 選んだturnの抜粋（読取りのみ）。入力欄へ入れるのはUI。取得できなかったturnは `missing` と本文の「未取得」で示す。
    pub async fn build_reference(self: &Arc<Self>, args: BuildReferenceArgs) -> Result<ReferenceBlock, IpcError> {
        if args.turns.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "参考にするturnを選んでください"));
        }
        let history = self.read_history_for_reference(&args.chat).await?;
        let title = self.chat_title(&args.chat);
        Ok(compose::build_reference(&title, &crate::win::clock::local_time_text(), &history.turns, &args.turns))
    }

    // ───────────── Skill（#11） ─────────────

    /// 作業フォルダのSkill一覧（読取りのみ）。`force_reload` はディスクの再走査（明示操作のときだけ）。
    pub async fn list_skills(self: &Arc<Self>, args: ListSkillsArgs) -> Result<SkillList, IpcError> {
        self.require_op(ParityOp::Skills)?;
        self.require_chat(&args.chat)?;
        let Some(cwd) = cwd_of(self, &args.chat) else {
            return Err(err(IpcErrorCode::InvalidArgs, "作業フォルダが分からないため、Skillを一覧できません"));
        };
        Ok(self.backend.list_skills(cwd, args.force_reload).await?)
    }

    /// Skillを入力欄の指定（チップ）にする。コピーはせず、定義ファイルの実在を確認して台帳に載せる。同じSkillが下書きにあればそれを返す。
    pub async fn add_skill_attachment(self: &Arc<Self>, args: AddSkillArgs) -> Result<AttachmentEntry, IpcError> {
        self.require_op(ParityOp::Skills)?;
        self.require_chat(&args.chat)?;
        let (name, path) = (args.name.trim().to_string(), args.path.trim().to_string());
        if name.is_empty() || path.is_empty() || !Path::new(&path).is_absolute() {
            return Err(err(IpcErrorCode::InvalidArgs, "Skillの名前と定義ファイルの場所（絶対パス）が必要です"));
        }
        let probe = path.clone();
        let exists = tokio::task::spawn_blocking(move || attach::check_exists(Path::new(&probe))).await.unwrap_or(Known::NotFetched);
        match exists {
            Known::Value { value: true, .. } => {}
            Known::Value { value: false, .. } => return Err(err(IpcErrorCode::NotFound, "Skillの定義ファイルが見つかりません（移動または削除された可能性があります）")),
            _ => return Err(err(IpcErrorCode::Io, "Skillの定義ファイルの有無を確認できませんでした")),
        }
        let existing = self.read(|d| {
            let f = d.locals.get(&args.chat)?;
            f.draft.attachments.iter().filter_map(|id| f.attachments.iter().find(|a| &a.id == id)).find(|a| matches!(&a.source, AttachmentSource::Skill { path: p, .. } if p == &path)).cloned()
        });
        if let Some(e) = existing {
            return Ok(e);
        }
        let entry = AttachmentEntry {
            id: self.local_id("att"),
            chat: args.chat.clone(),
            kind: AttachmentKind::Skill,
            display_name: name.clone(),
            source: AttachmentSource::Skill { name, path },
            copy_path: None,
            size: Known::NotFetched,
            attached_at: now_ms(),
            state: AttachmentState::Ready,
            used_by: Vec::new(),
        };
        self.upsert_attachment(&args.chat, entry.clone(), true);
        self.save_now(SaveScope::ChatLocal { chat: args.chat.clone() }).await;
        Ok(entry)
    }

    // ───────────── 指示ファイル（#11） ─────────────

    /// 指示ファイルの確認（読取りのみ）。読み込み済みの一覧は、この接続で応答が示したものだけ（まだなら未取得）。
    pub async fn get_instruction_files(self: &Arc<Self>, args: ChatArgs) -> Result<InstructionFiles, IpcError> {
        self.require_op(ParityOp::InstructionFiles)?;
        self.require_chat(&args.chat)?;
        let cwd = cwd_of(self, &args.chat);
        let seen = self.backend.instruction_sources(args.chat.clone()).await?;
        let agents_md = cwd.as_ref().map(|c| Path::new(c).join("AGENTS.md"));
        tokio::task::spawn_blocking(move || {
            let sources = match seen {
                Known::Value { value, basis } => Known::Value {
                    value: value.into_iter().map(|path| InstructionFileEntry { exists: attach::check_exists(Path::new(&path)), path }).collect(),
                    basis,
                },
                Known::NotFetched => Known::NotFetched,
                Known::Unsupported => Known::Unsupported,
                Known::Missing => Known::Missing,
            };
            InstructionFiles {
                cwd: cwd.map(Known::direct).unwrap_or(Known::NotFetched),
                sources,
                agents_md_exists: agents_md.map(|p| attach::check_exists(&p)).unwrap_or(Known::NotFetched),
            }
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("指示ファイルの確認が中断されました: {e}")))
    }

    /// 開いてよい指示ファイルのパス（ユーザー操作の「開く」の直前に確認する）。その会話の指示ファイルとして確認できたもの
    /// （この接続で応答が示した一覧、または作業フォルダ直下の AGENTS.md）だけ。実行形式・実在しないものは開かない。
    pub async fn resolve_instruction_file(self: &Arc<Self>, args: &OpenInstructionFileArgs) -> Result<PathBuf, IpcError> {
        self.require_op(ParityOp::InstructionFiles)?;
        self.require_chat(&args.chat)?;
        let mut known: Vec<String> = match self.backend.instruction_sources(args.chat.clone()).await? {
            Known::Value { value, .. } => value,
            _ => Vec::new(),
        };
        if let Some(cwd) = cwd_of(self, &args.chat) {
            known.push(Path::new(&cwd).join("AGENTS.md").to_string_lossy().into_owned());
        }
        let norm = |s: &str| s.replace('/', "\\").to_lowercase();
        if !known.iter().any(|k| norm(k) == norm(&args.path)) {
            return Err(err(IpcErrorCode::InvalidArgs, "この会話の指示ファイルとして確認できないパスは開けません"));
        }
        let path = PathBuf::from(&args.path);
        if attach::is_executable_path(&path) {
            return Err(err(IpcErrorCode::InvalidArgs, "実行形式のファイルは、ここからは開きません"));
        }
        let probe = path.clone();
        match tokio::task::spawn_blocking(move || attach::check_exists(&probe)).await.unwrap_or(Known::NotFetched) {
            Known::Value { value: true, .. } => Ok(path),
            Known::Value { value: false, .. } => Err(err(IpcErrorCode::NotFound, "ファイルが見つかりません")),
            _ => Err(err(IpcErrorCode::Io, "ファイルの有無を確認できませんでした")),
        }
    }

    /// 作業フォルダに AGENTS.md の雛形を作る。新規作成のみで、既存のファイルがあれば何も書かない（`AlreadyExists`）。
    /// ユーザーの確認の後だけ呼ぶ。作ったあとに開くのはUI（開く操作）。
    pub async fn create_instruction_template(self: &Arc<Self>, args: ChatArgs, _confirmed: &UserConfirmed) -> Result<InstructionTemplateResult, IpcError> {
        self.require_op(ParityOp::InstructionFiles)?;
        self.require_chat(&args.chat)?;
        let Some(cwd) = cwd_of(self, &args.chat) else {
            return Err(err(IpcErrorCode::InvalidArgs, "作業フォルダが分からないため、雛形を作れません"));
        };
        let dir = PathBuf::from(&cwd);
        if !dir.is_dir() {
            return Err(err(IpcErrorCode::NotFound, format!("作業フォルダが見つかりません: {cwd}")));
        }
        let path = dir.join("AGENTS.md");
        let target = path.clone();
        let written = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            // create_new: 既存のファイル（どんな内容でも）を上書き・追記しない。
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
            f.write_all(compose::AGENTS_TEMPLATE.as_bytes())?;
            f.flush()
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("雛形の作成が中断されました: {e}")))?;
        let shown = path.to_string_lossy().into_owned();
        match written {
            Ok(()) => Ok(InstructionTemplateResult { path: shown }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(blocked(BlockedReason::AlreadyExists, format!("すでに AGENTS.md があります（上書きしません）: {shown}")))
            }
            Err(e) => Err(err(IpcErrorCode::Io, format!("雛形を作れませんでした: {e}"))),
        }
    }
}

//! MCP・Plugins（P3-5、`app/DESIGN_P3.md` §1 #12）。
//!
//! - 一覧・認可・再読込みはバックエンドの読取り／明示操作。Plugin管理系のApp Server API（install・uninstall）は使わない。
//! - 管理操作（導入・削除・サーバーの追加・削除）は、UIの確認画面を経たユーザー操作だけが呼ぶ。実行の手順は
//!   「開始の記録を保存 → バックエンドがCLI補助の前後で設定を読み直して照合 → 結果の記録」で、アプリ内で直列化する。
//!   記録を保存できなければ実行しない。自動rollbackはしない。結果は `extension-ops.jsonl` へ追記する
//!   （起動コマンド・環境変数・認証情報・出力の原文は書かない）。開始だけで終了の記録がないものは「結果未確認」。
//! - 認可URLは返すだけ（開くのはUIの明示クリック）。ホストはURLを保持・記録しない。完了通知で更新し、
//!   5分来なければ「完了未確認」（失敗とは言わない）。
//! - 再読込みはユーザーのボタンだけ。受付は既存会話への反映を意味しない（UIが「未確認」と表示する）。
//! - 一覧に出ることを既存会話への反映と扱わない（項目別の `Known` は変換元が持つ）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use super::{err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::model::*;
use crate::backend::parity::*;

/// 認可の完了通知を待つ時間。過ぎたら「完了未確認」。
const LOGIN_WAIT: Duration = Duration::from_secs(5 * 60);
/// 管理操作の履歴として返す件数（新しい順）。
const OPS_LIST_LIMIT: usize = 50;

/// 管理操作の直列化と、認可の進行（URL・トークンは持たない）。
#[derive(Default)]
pub struct ExtensionsRuntime {
    lock: tokio::sync::Mutex<()>,
    logins: StdMutex<HashMap<String, ToolServerLogin>>,
}

/// 操作の種別と対象名（起動コマンドは含めない）。
fn op_label(op: &ExtensionOp) -> (ExtensionOpKind, String) {
    match op {
        ExtensionOp::Install { id } => (ExtensionOpKind::Install, id.clone()),
        ExtensionOp::Remove { id } => (ExtensionOpKind::Remove, id.clone()),
        ExtensionOp::AddToolServer { name, .. } => (ExtensionOpKind::AddToolServer, name.clone()),
        ExtensionOp::RemoveToolServer { name } => (ExtensionOpKind::RemoveToolServer, name.clone()),
    }
}

/// 記録の行を、新しい順の操作記録へ畳む（純粋）。終了の行がなければ `result` なし（結果未確認）。
pub fn fold_ops(lines: Vec<ExtensionOpLine>, limit: usize) -> Vec<ExtensionOpRecord> {
    let mut records: Vec<ExtensionOpRecord> = Vec::new();
    for line in lines {
        match line {
            ExtensionOpLine::Started { id, at, kind, target } => records.push(ExtensionOpRecord { id, at, kind, target, result: None }),
            ExtensionOpLine::Finished { id, result, .. } => {
                if let Some(r) = records.iter_mut().rev().find(|r| r.id == id) {
                    r.result = Some(result);
                }
            }
        }
    }
    records.reverse();
    records.truncate(limit);
    records
}

/// 保存する結果。原文のまま見せる出力は保存しない（要約できたものだけ残す）。
fn loggable(result: &ExtensionOpResult) -> ExtensionOpResult {
    let mut r = result.clone();
    if r.summary_is_raw {
        r.output_summary = String::new();
    }
    r
}

fn not_run_result(reason: String) -> ExtensionOpResult {
    ExtensionOpResult {
        status: ExtensionOpStatus::NotRun { reason: reason.clone() },
        exit_code: Known::NotFetched,
        config_compare: ConfigCompare::Unverified { reason: "実行していないため照合していません".into() },
        config_hash_before: Known::NotFetched,
        config_hash_after: Known::NotFetched,
        output_summary: reason,
        summary_is_raw: false,
    }
}

impl Host {
    // ───────────── ツールサーバー（MCP） ─────────────

    /// 一覧（読取りのみ）。取得できなければエラー（空の一覧にしない）。
    pub async fn list_tool_servers(self: &Arc<Self>) -> Result<ToolServerList, IpcError> {
        self.require_op(ParityOp::ToolServers)?;
        let servers = self.backend.list_tool_servers().await?;
        let mut logins: Vec<ToolServerLogin> = self.ext_rt.logins.lock().unwrap().values().cloned().collect();
        logins.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(ToolServerList { servers, logins })
    }

    /// 認可を始め、認可URLを返す。URLは保持・記録しない。開くのはUIの明示クリックだけ。
    pub async fn login_tool_server(self: &Arc<Self>, args: ToolServerNameArgs, confirmed: &UserConfirmed) -> Result<ToolServerLoginStart, IpcError> {
        self.require_op(ParityOp::ToolServers)?;
        let name = args.name.trim().to_string();
        if name.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "サーバー名が必要です"));
        }
        let url = self.backend.login_tool_server(name.clone(), confirmed).await?;
        let login = ToolServerLogin { name: name.clone(), state: ToolServerLoginState::Waiting, since: now_ms() };
        self.set_login(login.clone());
        let host = self.clone();
        let since = login.since;
        tokio::spawn(async move {
            tokio::time::sleep(LOGIN_WAIT).await;
            host.expire_login(&name, since);
        });
        Ok(ToolServerLoginStart { authorization_url: url })
    }

    fn set_login(&self, login: ToolServerLogin) {
        self.ext_rt.logins.lock().unwrap().insert(login.name.clone(), login.clone());
        self.mutate(|_| ((), vec![HostEvent::ToolServerLoginUpdated { login }]));
    }

    /// 待機のまま5分たったものを「完了未確認」にする（同じ認可の待機だけ。後から始めた認可は変えない）。
    fn expire_login(&self, name: &str, since: UnixMillis) {
        let expired = {
            let mut g = self.ext_rt.logins.lock().unwrap();
            match g.get_mut(name) {
                Some(l) if l.since == since && l.state == ToolServerLoginState::Waiting => {
                    l.state = ToolServerLoginState::CompletionUnconfirmed;
                    Some(l.clone())
                }
                _ => None,
            }
        };
        if let Some(login) = expired {
            self.mutate(|_| ((), vec![HostEvent::ToolServerLoginUpdated { login }]));
        }
    }

    /// ツールサーバー関連の通知の反映（認可の完了・起動状態の変化）。状態の判定には使わず、UIが取り直す合図にする。
    pub(super) fn ext_observe(self: &Arc<Self>, ev: &BackendEvent) {
        match ev {
            BackendEvent::ToolServerLoginCompleted { name, success } => {
                let waiting = self.ext_rt.logins.lock().unwrap().contains_key(name);
                if waiting {
                    self.set_login(ToolServerLogin { name: name.clone(), state: ToolServerLoginState::Completed { success: *success }, since: now_ms() });
                }
                self.mutate(|_| ((), vec![HostEvent::ToolServersChanged { name: name.clone() }]));
            }
            BackendEvent::ToolServerStatusChanged { name, .. } => {
                self.mutate(|_| ((), vec![HostEvent::ToolServersChanged { name: name.clone() }]));
            }
            _ => {}
        }
    }

    /// 設定の再読込みの要求（ユーザーのボタンだけ）。受付は「要求が受け付けられた」だけで、既存会話への反映は確認できない。
    pub async fn reload_tool_servers(self: &Arc<Self>, confirmed: &UserConfirmed) -> Result<OpAck, IpcError> {
        self.require_op(ParityOp::ToolServers)?;
        Ok(self.backend.reload_tool_servers(confirmed).await?)
    }

    // ───────────── 拡張（Plugins）・管理操作 ─────────────

    /// 一覧（読取りのみ）。
    pub async fn list_extensions(self: &Arc<Self>) -> Result<Vec<ExtensionView>, IpcError> {
        self.require_op(ParityOp::Extensions)?;
        Ok(self.backend.list_extensions().await?)
    }

    async fn append_ext_line(&self, line: ExtensionOpLine) -> Result<(), IpcError> {
        let Some(store) = self.persist.store().cloned() else {
            return Err(err(IpcErrorCode::Io, "操作の記録を保存できないため、実行しません"));
        };
        tokio::task::spawn_blocking(move || store.append_extension_op(&line))
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("操作の記録が中断されました: {e}")))?
            .map_err(|e| err(IpcErrorCode::Io, format!("操作の記録を保存できないため、実行しません（{e}）")))
    }

    /// 管理操作の履歴（新しい順）。終了の記録がないものは `result` なし（結果未確認）。
    pub async fn list_extension_ops(self: &Arc<Self>) -> Result<Vec<ExtensionOpRecord>, IpcError> {
        let Some(store) = self.persist.store().cloned() else { return Ok(Vec::new()) };
        let lines = tokio::task::spawn_blocking(move || store.read_extension_ops())
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("記録の読込みが中断されました: {e}")))?
            .map_err(|e| err(IpcErrorCode::Io, format!("操作の記録を読めませんでした: {e}")))?;
        Ok(fold_ops(lines, OPS_LIST_LIMIT))
    }

    /// 管理操作を実行する（確認画面を経たユーザー操作だけ）。同時に実行できるのは1件。
    pub async fn manage_extension(self: &Arc<Self>, args: ManageExtensionArgs, confirmed: &UserConfirmed) -> Result<ExtensionOpRecord, IpcError> {
        self.require_op(ParityOp::Extensions)?;
        let _serial = self.ext_rt.lock.lock().await;
        let (kind, target) = op_label(&args.op);
        let id = self.local_id("extop");
        let at = now_ms();
        // 開始の記録を保存してから実行する（保存できなければ実行しない。異常終了後は「結果未確認」として残る）。
        self.append_ext_line(ExtensionOpLine::Started { id: id.clone(), at, kind, target: target.clone() }).await?;
        let mut record = ExtensionOpRecord { id: id.clone(), at, kind, target, result: None };
        self.mutate(|_| ((), vec![HostEvent::ExtensionOpUpdated { record: record.clone() }]));
        let outcome = self.backend.manage_extension(args.op, confirmed).await;
        let (result, failure) = match outcome {
            Ok(r) => (r, None),
            // 実行前に断られた（未接続・引数不正・非対応）。CLIは動いていない。
            Err(e) => (not_run_result(e.to_string()), Some(e)),
        };
        if let Err(e) = self.append_ext_line(ExtensionOpLine::Finished { id, at: now_ms(), result: loggable(&result) }).await {
            self.warn(format!("管理操作の結果を記録できませんでした: {}", e.message));
        }
        record.result = Some(result);
        self.mutate(|_| ((), vec![HostEvent::ExtensionOpUpdated { record: record.clone() }]));
        match failure {
            Some(e) => Err(e.into()),
            None => Ok(record),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str, at: i64) -> ExtensionOpLine {
        ExtensionOpLine::Started { id: LocalId(id.into()), at: UnixMillis(at), kind: ExtensionOpKind::Install, target: "p@m".into() }
    }

    #[test]
    fn started_without_finished_is_unconfirmed_and_newest_first() {
        let finished = ExtensionOpLine::Finished { id: LocalId("a".into()), at: UnixMillis(2), result: not_run_result("x".into()) };
        let recs = fold_ops(vec![started("a", 1), finished, started("b", 3)], 10);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].id.0, "b");
        assert!(recs[0].result.is_none());
        assert!(recs[1].result.is_some());
        assert_eq!(fold_ops(vec![started("a", 1), started("b", 3)], 1).len(), 1);
    }

    #[test]
    fn raw_output_is_not_persisted() {
        let mut r = not_run_result("x".into());
        r.summary_is_raw = true;
        r.output_summary = "secret-ish raw output".into();
        assert_eq!(loggable(&r).output_summary, "");
        r.summary_is_raw = false;
        assert_eq!(loggable(&r).output_summary, "secret-ish raw output");
    }
}

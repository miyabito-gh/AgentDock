//! 計画／実行・Goal・状態表示・memories（P3-3、`app/DESIGN_P3.md` §1 #5・#9・#15）。
//!
//! - 計画／実行・速度の選択は、モデルと同じ「選択値」を保持するだけで、次のturnの送信時に付ける（受理は設定の更新通知で確認する）。
//! - Goalの取得は読取りだけで、表示のためにresumeしない。設定・解除・memoriesの設定変更はユーザー操作なので、その中でだけ `ensure_live` を含める。
//! - 外部で実行中の会話・削除保留中の会話には、状態を変える操作を出さない。
//! - memoriesのリセットは、影響を表示して確認した後（`confirmed=true`）だけ。結果は読み直した値を観測値として返す。
//! - 通信断・取得不能は「未取得」のまま。成功・失敗に変換しない。

use std::sync::Arc;
use std::time::Duration;

use super::state::external_send_locked;
use super::{blocked, err, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::{ChatArgs, SettingsImpact};
use crate::backend::model::*;
use crate::backend::parity::*;

/// バックエンドの結果を、読取り用の `Known` にする。非対応は `Unsupported`、読めなかったものは `NotFetched`（成功・失敗に変換しない）。
pub fn known_of<T>(r: Result<Known<T>, BackendError>) -> Known<T> {
    match r {
        Ok(k) => k,
        Err(BackendError::Unsupported { .. }) => Known::Unsupported,
        Err(_) => Known::NotFetched,
    }
}

/// 操作を送った後に、結果を読み直す価値があるか。拒否されたなら状態は変わっていないので読み直さない。
pub fn should_refresh_after(ack: &OpAck) -> bool {
    !matches!(ack, OpAck::Rejected { .. })
}

impl Host {
    /// 操作の対象にできるチャットか（削除保留中・外部で実行中は不可）。
    fn check_op_target(&self, chat: &ChatKey) -> Result<(), IpcError> {
        if self.read(|d| d.chat(chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        self.check_not_delete_pending(chat)?;
        let external = self.read(|d| d.chat(chat).is_some_and(|c| external_send_locked(c.origin, d.root_view(chat).map(|v| v.freshness))));
        if external {
            return Err(blocked(BlockedReason::ExternalRunning, "外部で作成された会話です。外部の実行が終わったことを確認して再開するまで、この操作はできません"));
        }
        Ok(())
    }

    // ───────────── 計画／実行（#5） ─────────────

    /// 計画／実行の選択。次のturnから適用し、送信待ちの依頼にも送信時の値が適用される（影響を返す）。受理は通知で確認する。
    /// None＝選択を外す（常に可）。選ぶときは、この接続で使えること（降格・experimental無効でないこと）が前提。
    pub fn set_work_mode(self: &Arc<Self>, args: SetWorkModeArgs) -> Result<SettingsImpact, IpcError> {
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        if args.mode.is_some() {
            let cap = self.backend.capabilities().ops.into_iter().find(|c| c.op == ParityOp::WorkMode);
            if !cap.is_some_and(|c| c.support == Support::Supported) {
                return Err(IpcError {
                    code: IpcErrorCode::Unsupported,
                    message: "計画／実行の切替は、この Codex 接続では使えません".into(),
                    blocked: Some(BlockedReason::CapabilityUnsupported { capability: ParityOp::WorkMode.name() }),
                });
            }
        }
        self.precheck_space()?;
        self.mutate(|d| {
            let s = d.model_settings.entry(args.chat.clone()).or_insert_with(|| ChatModelSettings::blank(ApplyTiming::NextTurn));
            s.work_mode = args.mode;
            s.applies = ApplyTiming::NextTurn;
            let s = s.clone();
            ((), vec![HostEvent::ModelSettingsUpdated { chat: args.chat.clone(), settings: s }])
        });
        // 選択値だけを保存する（受理値は保存しない）。
        self.update_local(&args.chat, true, Duration::ZERO, |f| f.work_mode = args.mode);
        Ok(self.settings_impact(&args.chat))
    }

    pub async fn list_work_modes(&self) -> Result<Vec<WorkModeInfo>, IpcError> {
        Ok(self.backend.list_work_modes().await?)
    }

    // ───────────── Goal（#9） ─────────────

    /// 目標の取得（読取りのみ。resumeしない）。読めなければ「未取得」、目標がなければ「値なし（Missing）」。
    pub async fn get_goal(&self, args: ChatArgs) -> Result<Known<Goal>, IpcError> {
        let r = self.backend.get_goal(args.chat).await;
        if let Err(e) = &r {
            crate::diag::log("goal-get", &crate::diag::error_kind(e));
        }
        Ok(known_of(r))
    }

    pub async fn set_goal(self: &Arc<Self>, args: SetGoalArgs, confirmed: &UserConfirmed) -> Result<OpAck, IpcError> {
        self.check_op_target(&args.chat)?;
        // ユーザー操作の中でだけ、必要ならresumeして購読を戻す。
        self.ensure_live(&args.chat, confirmed).await?;
        let ack = self.backend.set_goal(args.chat.clone(), args.update, confirmed).await?;
        self.refresh_goal_after(&args.chat, &ack).await;
        Ok(ack)
    }

    pub async fn clear_goal(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<OpAck, IpcError> {
        self.check_op_target(&args.chat)?;
        self.ensure_live(&args.chat, confirmed).await?;
        let ack = self.backend.clear_goal(args.chat.clone(), confirmed).await?;
        self.refresh_goal_after(&args.chat, &ack).await;
        Ok(ack)
    }

    /// 操作の後に、目標を読み直して画面へ渡す（受理不明の照合も兼ねる。読めた値をそのまま表示する）。読めなければ何も出さない。
    async fn refresh_goal_after(self: &Arc<Self>, chat: &ChatKey, ack: &OpAck) {
        if !should_refresh_after(ack) {
            return;
        }
        let goal = match self.backend.get_goal(chat.clone()).await {
            Ok(Known::Value { value, .. }) => Some(value),
            Ok(Known::Missing) => None,
            _ => return,
        };
        let chat = chat.clone();
        self.mutate(|_| ((), vec![HostEvent::GoalUpdated { chat, goal }]));
    }

    // ───────────── 状態（#15） ─────────────

    pub async fn get_backend_status(&self) -> Result<BackendStatus, IpcError> {
        Ok(self.backend.backend_status().await?)
    }

    // ───────────── memories（#15） ─────────────

    pub async fn get_memory_status(&self) -> Result<MemoryStatus, IpcError> {
        Ok(self.backend.memory_status().await?)
    }

    /// チャット単位のmemoriesの設定。受け付けられたときだけ、要求した値を保存する。
    pub async fn set_memory_mode(self: &Arc<Self>, args: SetMemoryModeArgs, confirmed: &UserConfirmed) -> Result<OpAck, IpcError> {
        self.check_op_target(&args.chat)?;
        self.ensure_live(&args.chat, confirmed).await?;
        let ack = self.backend.set_memory_mode(args.chat.clone(), args.mode.clone(), confirmed).await?;
        if matches!(ack, OpAck::Accepted) {
            self.update_local(&args.chat, true, Duration::ZERO, |f| f.memory_mode = Some(args.mode));
        }
        Ok(ack)
    }

    /// 記憶データのリセット。影響を表示して確認した後（`confirmed=true`）だけ。実行後に読み直した値を観測値として返す。
    pub async fn reset_memory(self: &Arc<Self>, args: ResetMemoryArgs, confirmed: &UserConfirmed) -> Result<ResetMemoryResult, IpcError> {
        if !args.confirmed {
            return Err(err(IpcErrorCode::InvalidArgs, "リセットには、影響を確認したうえでの明示的な確認が必要です"));
        }
        let ack = self.backend.reset_memory(confirmed).await?;
        let status_after = if should_refresh_after(&ack) { self.backend.memory_status().await.ok() } else { None };
        Ok(ResetMemoryResult { ack, status_after })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_of_maps_unsupported_and_unreadable_without_inventing_values() {
        let unsupported: Result<Known<u32>, BackendError> = Err(BackendError::Unsupported { capability: "goal".into() });
        assert_eq!(known_of(unsupported), Known::Unsupported);
        let unreadable: Result<Known<u32>, BackendError> = Err(BackendError::Rejected { code: None, message: "thread not loaded".into() });
        assert_eq!(known_of(unreadable), Known::NotFetched);
        let timeout: Result<Known<u32>, BackendError> = Err(BackendError::OutcomeUnknown { message: "t".into() });
        assert_eq!(known_of(timeout), Known::NotFetched);
        assert_eq!(known_of(Ok(Known::Missing::<u32>)), Known::Missing);
        assert_eq!(known_of(Ok(Known::direct(3u32))), Known::direct(3));
    }

    #[test]
    fn rejected_operations_are_not_refreshed_but_unknown_ones_are_reconciled() {
        assert!(should_refresh_after(&OpAck::Accepted));
        assert!(should_refresh_after(&OpAck::Unknown { message: "t".into() }));
        assert!(!should_refresh_after(&OpAck::Rejected { message: "no".into() }));
    }
}

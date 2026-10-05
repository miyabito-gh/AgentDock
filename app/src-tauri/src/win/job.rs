//! App ServerのJob Object（P6、§3.4・§32.6）。
//!
//! - `codex app-server` を起動した直後、`initialize` を送る前に `assign` する（それ以前に子は生まれない前提）。
//! - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` は付けない。AgentDockの異常終了で作業を無断停止しないため
//!   （App Serverはstdinの切断で自ら終了する。強制終了はユーザーの明示操作だけ）。
//! - 強制終了は監視元（App Server）単位。同じ監視元の全チャットが止まるので、確認画面に全対象を出す。
//! - 管理外のプロセス（Job外）は対象にしない。終了後も `active_processes()==0` を確認するまでは停止未確認のまま。

/// 監視元1つ分のJob。Drop でハンドルを閉じるだけ（中のプロセスは止めない）。
pub struct ProcessJob {
    _private: (),
}

impl ProcessJob {
    /// Jobを作り、プロセスを入れる。
    pub fn assign(pid: u32) -> std::io::Result<Self> {
        let _ = pid;
        todo!("P6")
    }

    /// Job内の稼働中プロセス数（`JobObjectBasicAccountingInformation.ActiveProcesses`）。
    pub fn active_processes(&self) -> std::io::Result<u32> {
        todo!("P6")
    }

    /// ユーザーの明示確認後にだけ呼ぶ（`UserConfirmed` を要求する）。`TerminateJobObject`。
    pub fn terminate(&self, _confirmed: &crate::backend::backend::UserConfirmed) -> std::io::Result<()> {
        todo!("P6")
    }
}

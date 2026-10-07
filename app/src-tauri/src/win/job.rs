//! App ServerのJob Object（P6、§3.4・§32.6）。
//!
//! - `codex app-server` を起動した直後、`initialize` を送る前に `assign` する（それ以前に子は生まれない前提）。
//! - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` は付けない。AgentDockの異常終了で作業を無断停止しないため
//!   （App Serverはstdinの切断で自ら終了する。強制終了はユーザーの明示操作だけ）。
//! - 強制終了は監視元（App Server）単位。同じ監視元の全チャットが止まるので、確認画面に全対象を出す。
//! - 管理外のプロセス（Job外）は対象にしない。終了後も `active_processes()==0` を確認するまでは停止未確認のまま。

/// 監視元1つ分のJob。Drop でハンドルを閉じるだけ（中のプロセスは止めない）。
pub struct ProcessJob {
    /// Job のハンドル（`HANDLE` を整数で持つ。`Send`/`Sync` にするため）。
    handle: usize,
}

#[cfg(windows)]
mod sys {
    use super::ProcessJob;
    use std::io::{Error, Result};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, QueryInformationJobObject, TerminateJobObject,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    pub fn assign(pid: u32) -> Result<ProcessJob> {
        unsafe {
            let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(Error::last_os_error());
            }
            let process: HANDLE = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                let e = Error::last_os_error();
                CloseHandle(job);
                return Err(e);
            }
            let ok = AssignProcessToJobObject(job, process);
            let assign_err = if ok == 0 { Some(Error::last_os_error()) } else { None };
            CloseHandle(process);
            if let Some(e) = assign_err {
                CloseHandle(job);
                return Err(e);
            }
            Ok(ProcessJob { handle: job as usize })
        }
    }

    pub fn active_processes(job: &ProcessJob) -> Result<u32> {
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                job.handle as HANDLE,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                return Err(Error::last_os_error());
            }
            Ok(info.ActiveProcesses)
        }
    }

    pub fn terminate(job: &ProcessJob) -> Result<()> {
        unsafe {
            if TerminateJobObject(job.handle as HANDLE, 1) == 0 {
                return Err(Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn close(job: &ProcessJob) {
        unsafe {
            CloseHandle(job.handle as HANDLE);
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use super::ProcessJob;
    use std::io::{Error, ErrorKind, Result};
    fn unsupported<T>() -> Result<T> {
        Err(Error::new(ErrorKind::Unsupported, "Job Object is Windows-only"))
    }
    pub fn assign(_pid: u32) -> Result<ProcessJob> {
        unsupported()
    }
    pub fn active_processes(_job: &ProcessJob) -> Result<u32> {
        unsupported()
    }
    pub fn terminate(_job: &ProcessJob) -> Result<()> {
        unsupported()
    }
    pub fn close(_job: &ProcessJob) {}
}

impl ProcessJob {
    /// Jobを作り、プロセスを入れる。
    pub fn assign(pid: u32) -> std::io::Result<Self> {
        sys::assign(pid)
    }

    /// Job内の稼働中プロセス数（`JobObjectBasicAccountingInformation.ActiveProcesses`）。
    pub fn active_processes(&self) -> std::io::Result<u32> {
        sys::active_processes(self)
    }

    /// AgentDockが自分で起動した外部コマンド（`exec::run_bounded`）の時間切れ・出力超過で、孫プロセスごと終了させる。
    /// 監視元（App Server）には使わない（そちらは確認つきの `terminate`）。
    pub fn terminate_own(&self) -> std::io::Result<()> {
        sys::terminate(self)
    }

    /// ユーザーの明示確認後にだけ呼ぶ（`UserConfirmed` を要求する）。`TerminateJobObject`。
    pub fn terminate(&self, _confirmed: &crate::backend::backend::UserConfirmed) -> std::io::Result<()> {
        sys::terminate(self)
    }
}

impl Drop for ProcessJob {
    /// ハンドルを閉じるだけ。`KILL_ON_JOB_CLOSE` を付けていないので、中のプロセスは止まらない。
    fn drop(&mut self) {
        sys::close(self);
    }
}

//! agy プロセスの生存確認（OS への問い合わせだけ。停止・resume・送信はしない）。
//!
//! 方針（2026-11-26、結果不明後の送信ガード）:
//! - 起動した agy の PID と生成時刻を turn の記録に残す（[`ProcessMark`]）。終了を確認できたら消す。
//! - 記録が残っている間は、同じチャットへの次の送信を拒否する（同じ会話IDで agy が2本動かないように）。
//!   解放は「終了を確認できたとき」だけ。推測では解放しない（問い合わせに失敗したら「不明」で、送信は止めたまま）。
//! - PID の再利用で別のプロセスを agy と取り違えないよう、生成時刻が一致するときだけ同じプロセスとみなす。

use serde::{Deserialize, Serialize};

/// このアプリが起動した agy プロセスの識別（記録用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMark {
    pub pid: u32,
    /// プロセスの生成時刻（FILETIME、100ns 単位）。取れなかったら None（PID だけで判定する＝止める側に倒れる）。
    #[serde(default)]
    pub created: Option<u64>,
}

/// 生存確認の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// 同じプロセスが動いている。
    Alive,
    /// 終了した（または PID が別のプロセスに再利用されている）。
    Gone,
    /// 確かめられなかった（権限など）。解放しない。
    Unknown(String),
}

/// 起動直後のプロセスの識別を取る。生成時刻が取れなければ `created: None`。
pub fn mark_of(pid: u32) -> ProcessMark {
    ProcessMark { pid, created: sys::creation_time(pid).ok().flatten() }
}

/// 記録したプロセスがまだ動いているかを OS に問い合わせる。
pub fn check(mark: &ProcessMark) -> Liveness {
    sys::check(mark)
}

#[cfg(windows)]
mod sys {
    use super::{Liveness, ProcessMark};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE};

    enum Open {
        Handle(HANDLE),
        /// その PID のプロセスは無い。
        NoSuchProcess,
        Failed(std::io::Error),
    }

    fn open(pid: u32) -> Open {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
        if !h.is_null() {
            return Open::Handle(h);
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Open::NoSuchProcess
        } else {
            Open::Failed(e)
        }
    }

    fn created(h: HANDLE) -> Option<u64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut x, mut k, mut u) = (zero, zero, zero, zero);
        let ok = unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) };
        (ok != 0).then(|| (u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime))
    }

    pub fn creation_time(pid: u32) -> std::io::Result<Option<u64>> {
        match open(pid) {
            Open::Handle(h) => {
                let t = created(h);
                unsafe { CloseHandle(h) };
                Ok(t)
            }
            Open::NoSuchProcess => Ok(None),
            Open::Failed(e) => Err(e),
        }
    }

    pub fn check(mark: &ProcessMark) -> Liveness {
        let h = match open(mark.pid) {
            Open::Handle(h) => h,
            Open::NoSuchProcess => return Liveness::Gone,
            Open::Failed(e) => return Liveness::Unknown(format!("プロセス（PID {}）の状態を確認できませんでした: {e}", mark.pid)),
        };
        let result = (|| {
            if let Some(expected) = mark.created {
                match created(h) {
                    // PID が再利用されている（記録したプロセスは終了済み）。
                    Some(actual) if actual != expected => return Liveness::Gone,
                    Some(_) => {}
                    None => return Liveness::Unknown(format!("プロセス（PID {}）の生成時刻を確認できませんでした", mark.pid)),
                }
            }
            match unsafe { WaitForSingleObject(h, 0) } {
                WAIT_TIMEOUT => Liveness::Alive,
                WAIT_OBJECT_0 => Liveness::Gone,
                other => Liveness::Unknown(format!("プロセス（PID {}）の終了を確認できませんでした（{other}: {}）", mark.pid, std::io::Error::last_os_error())),
            }
        })();
        unsafe { CloseHandle(h) };
        result
    }
}

#[cfg(not(windows))]
mod sys {
    use super::{Liveness, ProcessMark};
    pub fn creation_time(_pid: u32) -> std::io::Result<Option<u64>> {
        Ok(None)
    }
    pub fn check(mark: &ProcessMark) -> Liveness {
        Liveness::Unknown(format!("この OS ではプロセス（PID {}）の生存を確認できません", mark.pid))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn own_process_is_alive_and_a_different_creation_time_is_gone() {
        let me = mark_of(std::process::id());
        assert!(me.created.is_some());
        assert_eq!(check(&me), Liveness::Alive);
        // 同じ PID でも生成時刻が違えば、記録したプロセスではない（PID の再利用）。
        let reused = ProcessMark { pid: me.pid, created: me.created.map(|c| c + 1) };
        assert_eq!(check(&reused), Liveness::Gone);
    }

    #[test]
    fn exited_process_is_gone() {
        let mut child = std::process::Command::new("cmd.exe").args(["/C", "exit 0"]).spawn().unwrap();
        let mark = mark_of(child.id());
        // 終了後もハンドルを持つ間は PID が再利用されないので、確実に「終了済み」を観測できる。
        child.wait().unwrap();
        assert_eq!(check(&mark), Liveness::Gone);
        drop(child);
    }

    #[test]
    fn missing_pid_is_gone() {
        // PID 0（System Idle Process）は OpenProcess が ERROR_INVALID_PARAMETER で失敗すると文書化されている。
        assert_eq!(check(&ProcessMark { pid: 0, created: None }), Liveness::Gone);
    }
}

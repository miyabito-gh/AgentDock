//! 履歴の読取り（`thread/read`）の同時実行数の上限（P3-9、`app/DESIGN_P3.md` §1 #18）。
//!
//! - 許可証は読取り1回のあいだだけ持つ。許可証を持ったまま別の許可証を待つ経路はない（読取りの中で読取りをしない）ので、デッドロックしない。
//! - 待たされるだけで、読取りを捨てたり並べ替えて欠落させたりしない（待ち行列は先着順）。
//! - 上限は1接続あたりの同時の読取り数で、チャット数が増えても定期処理は増えない。

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Semaphore;

/// 読取りの同時実行の最大数。
pub const MAX_CONCURRENT_READS: usize = 4;

pub struct ReadLimiter {
    sem: Arc<Semaphore>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl Default for ReadLimiter {
    fn default() -> Self {
        Self::new(MAX_CONCURRENT_READS)
    }
}

impl ReadLimiter {
    pub fn new(max: usize) -> Self {
        ReadLimiter { sem: Arc::new(Semaphore::new(max)), active: AtomicUsize::new(0), peak: AtomicUsize::new(0) }
    }

    /// 許可を得てから `fut` を実行する（許可はawaitが終わるまで）。
    pub async fn run<T>(&self, fut: impl Future<Output = T>) -> T {
        // セマフォは閉じないので取得は失敗しない。
        let _permit = self.sem.acquire().await.expect("read limiter is never closed");
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let out = fut.await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        out
    }

    /// これまでの同時実行の最大値（診断・テスト用）。
    #[allow(dead_code)]
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_reads_never_exceed_the_limit_and_all_complete() {
        let lim = Arc::new(ReadLimiter::default());
        let mut tasks = Vec::new();
        for i in 0..30u32 {
            let lim = lim.clone();
            tasks.push(tokio::spawn(async move {
                lim.run(async {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    i
                })
                .await
            }));
        }
        let mut got = Vec::new();
        for t in tasks {
            got.push(t.await.unwrap());
        }
        got.sort();
        assert_eq!(got, (0..30).collect::<Vec<_>>(), "no read is dropped");
        assert!(lim.peak() <= MAX_CONCURRENT_READS, "peak was {}", lim.peak());
        assert_eq!(lim.peak(), MAX_CONCURRENT_READS, "the limit is actually reached under load");
    }
}

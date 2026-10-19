//! アプリからのプッシュ（DESIGN_ORG_INTAKE §3）。許可した `GitOp` だけを使う。
//!
//! - プレビュー（`preview`）は読取りだけ（ネットワークなし・fetchしない）。
//! - 送る（`push`）のは、確認時のHEADのコミット（`expected_sha`）だけ。強制・削除・全ブランチ・タグは `GitOp::Push` が表現できない。
//! - 資格情報は扱わない（`GIT_TERMINAL_PROMPT=0` で端末入力は出さず、Gitの資格情報マネージャに任せる）。
//! - 結果が分からないものは成功にしない。確認は読取りの `verify`（`ls-remote`）だけ。

use std::path::Path;

use super::worktree::dirty_count;
use super::{Git, GitError, GitOp};
use crate::backend::git_ops::*;
use crate::rules::git_push::*;

/// 表示するコミットの最大（超えたら `truncated`）。
const SHOWN_COMMITS: usize = 200;

fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn parse_log(text: &str) -> Vec<PushCommitView> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let sha = it.next()?.trim();
            let author = it.next()?.to_string();
            let subject = it.next().unwrap_or("").to_string();
            (!sha.is_empty()).then(|| PushCommitView { sha: sha.to_string(), short: short(sha), author, subject })
        })
        .collect()
}

async fn text_of(git: &Git, folder: &Path, op: &GitOp) -> Option<String> {
    let out = git.run(folder, op).await.ok()?;
    out.success().then(|| out.stdout_text().trim().to_string())
}

/// 送る前の確認用の情報（読取りのみ）。
pub async fn preview(git: &Git, folder: &Path, remote_pref: Option<&str>) -> PushPreview {
    let mut p = PushPreview {
        folder: folder.to_string_lossy().into_owned(),
        remotes: Vec::new(),
        remote: None,
        url: None,
        branch: None,
        upstream: None,
        head_sha: None,
        commits: Vec::new(),
        commit_total: 0,
        truncated: false,
        dirty_count: 0,
        blocker: None,
    };
    let branch = text_of(git, folder, &GitOp::CurrentBranch).await;
    let head = text_of(git, folder, &GitOp::Head).await;
    let remotes_out = git.run(folder, &GitOp::Remotes).await;
    let readable = branch.is_some() && head.is_some() && matches!(&remotes_out, Ok(o) if o.success());
    p.branch = branch.clone();
    p.head_sha = head;
    if let Ok(o) = &remotes_out {
        p.remotes = o.stdout_text().lines().map(str::trim).filter(|l| valid_remote_name(l)).map(str::to_string).collect();
    }
    p.upstream = text_of(git, folder, &GitOp::Upstream).await.filter(|u| !u.is_empty());
    // 送り先のリモート: 追跡先があればその名前、なければ指定、`origin`、先頭の順。
    p.remote = p
        .upstream
        .as_deref()
        .and_then(|u| u.split_once('/').map(|(r, _)| r.to_string()))
        .filter(|r| p.remotes.contains(r))
        .or_else(|| remote_pref.map(str::to_string).filter(|r| p.remotes.contains(r)))
        .or_else(|| p.remotes.iter().find(|r| r.as_str() == "origin").cloned())
        .or_else(|| p.remotes.first().cloned());
    if let Some(r) = &p.remote {
        p.url = text_of(git, folder, &GitOp::RemoteUrl { remote: r.clone() }).await.map(|u| mask_url(&u));
    }
    // 一覧の基準は実際の送り先（`<remote>/<ローカルのブランチ名>`）に合わせる。追跡先がその名前のときだけ範囲指定で数え、
    // 違う名前（別名のブランチを追跡している）のときは、そのリモートのどこにもないコミットを数える。
    let target = p.remote.as_ref().zip(p.branch.as_ref()).map(|(r, b)| format!("{r}/{b}"));
    let log_op = match (&p.upstream, &p.remote) {
        (Some(u), _) if target.as_deref() == Some(u.as_str()) => Some(GitOp::LogRange { upstream: u.clone() }),
        (_, Some(r)) => Some(GitOp::LogNotRemote { remote: r.clone() }),
        _ => None,
    };
    let mut commits_readable = readable;
    if let Some(op) = log_op {
        match git.run(folder, &op).await {
            Ok(o) if o.success() => {
                let all = parse_log(&o.stdout_text());
                p.truncated = all.len() > SHOWN_COMMITS;
                p.commits = all.into_iter().take(SHOWN_COMMITS).collect();
                p.commit_total = p.commits.len() as u32;
            }
            _ => commits_readable = false,
        }
    }
    p.dirty_count = dirty_count(git, folder).await.unwrap_or(0);
    let kind = push_blocker(&PushFacts { readable: commits_readable, branch, remote_count: p.remotes.len(), commit_total: p.commit_total });
    p.blocker = kind.map(|k| PushBlockerView { kind: k, message: blocker_message(k).to_string() });
    p
}

/// 送る。送る前の条件（リモート名・ブランチ・HEAD）が確認時と違えば、何も送らず理由を返す。
pub async fn push(git: &Git, folder: &Path, remote: &str, branch: &str, expected_sha: &str, set_upstream: bool) -> Result<PushResult, String> {
    if !valid_remote_name(remote) || !valid_push_branch(branch) {
        return Err("送り先の指定が正しくありません（何も送っていません）".to_string());
    }
    let remotes = text_of(git, folder, &GitOp::Remotes).await.unwrap_or_default();
    if !remotes.lines().any(|l| l.trim() == remote) {
        return Err(format!("リモート「{remote}」が見つかりません（何も送っていません）"));
    }
    let current = text_of(git, folder, &GitOp::CurrentBranch).await;
    if current.as_deref() != Some(branch) {
        return Err("現在のブランチが確認したときと違います。もう一度確認してください（何も送っていません）".to_string());
    }
    let head = text_of(git, folder, &GitOp::Head).await;
    if head.as_deref() != Some(expected_sha) {
        return Err("コミットが確認したときと変わっています。もう一度確認してください（何も送っていません）".to_string());
    }
    let out = match git.run(folder, &GitOp::Push { remote: remote.to_string(), branch: branch.to_string(), sha: expected_sha.to_string() }).await {
        Ok(o) => o,
        Err(GitError::Timeout) => return Ok(classify_push(true, None, "", "", expected_sha)),
        Err(e @ GitError::Unavailable(_)) | Err(e @ GitError::InvalidArgument(_)) => return Err(format!("Gitを実行できませんでした（何も送っていません）: {e}")),
        Err(e) => return Ok(PushResult::Unknown { message: format!("送れたか確認できません: {e}") }),
    };
    let mut result = classify_push(false, out.exit_code, &out.stdout_text(), &String::from_utf8_lossy(&out.stderr), expected_sha);
    if set_upstream {
        if let PushResult::Pushed { upstream_note, .. } = &mut result {
            let set = git.run(folder, &GitOp::SetUpstream { remote: remote.to_string(), branch: branch.to_string() }).await;
            *upstream_note = match set {
                Ok(o) if o.success() => None,
                Ok(o) => Some(format!("追跡先を設定できませんでした: {}", String::from_utf8_lossy(&o.stderr).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(""))),
                Err(e) => Some(format!("追跡先を設定できませんでした: {e}")),
            };
        }
    }
    Ok(result)
}

/// 結果が分からないときの確認（読取りのみ）。リモートのブランチが送ったコミットを指しているか。
pub async fn verify(git: &Git, folder: &Path, remote: &str, branch: &str, expected_sha: &str) -> PushVerify {
    if !valid_remote_name(remote) || !valid_push_branch(branch) {
        return PushVerify::Unavailable { message: "送り先の指定が正しくありません".to_string() };
    }
    match git.run(folder, &GitOp::LsRemoteBranch { remote: remote.to_string(), branch: branch.to_string() }).await {
        Ok(o) if o.success() => {
            let text = o.stdout_text();
            let line = text.lines().find(|l| l.trim_end().ends_with(&format!("refs/heads/{branch}")));
            match line.and_then(|l| l.split_whitespace().next()) {
                Some(sha) if sha == expected_sha => PushVerify::Confirmed,
                _ => PushVerify::NotThere,
            }
        }
        Ok(o) => PushVerify::Unavailable { message: String::from_utf8_lossy(&o.stderr).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("リモートを確認できません").to_string() },
        Err(e) => PushVerify::Unavailable { message: e.to_string() },
    }
}

#[cfg(test)]
mod tests;

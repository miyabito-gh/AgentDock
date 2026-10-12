//! 作業フォルダ（複数）の `runtimeWorkspaceRoots` との相互変換（純粋。`app/DESIGN_WORKFOLDERS.md` (b)・実測結果）。
//!
//! 実測で確定した規則:
//! - サーバーは `cwd` を roots に自動では含めない。送るリストの先頭に必ず `cwd`（プライマリ）を入れる。
//! - roots を省略すると現状維持（sticky）。**空配列は送らない**（省略とは別の意味になるため、空なら送らず拒否する）。
//! - experimental未対応・拒否は `Unsupported`（成功扱いにしない）。パス検査はサーバーがしないのでホスト側で済ませたものだけが来る。

use crate::backend::backend::*;
use crate::backend::model::*;
use crate::rules::work_folders::{is_same_or_under, normalize_path, path_key};
use serde_json::{json, Value};

fn reject(message: &str) -> BackendError {
    BackendError::Rejected { code: None, message: message.into() }
}

/// 送る一覧を検査する。空・空文字の要素は送らず拒否（空配列を絶対に送らない）。先頭＝cwd。
pub fn validated_folders(folders: &WorkFolders) -> BackendResult<&[String]> {
    if folders.folders.is_empty() {
        return Err(reject("作業フォルダが空のため送りません"));
    }
    if folders.folders.iter().any(|f| f.trim().is_empty()) {
        return Err(reject("空の作業フォルダを含むため送りません"));
    }
    Ok(&folders.folders)
}

/// `turn/start` の params に `cwd`＝先頭、`runtimeWorkspaceRoots`＝全件（先頭を含む）を付ける。
pub fn apply_turn_folders(p: &mut Value, folders: &WorkFolders) -> BackendResult<()> {
    let list = validated_folders(folders)?;
    p["cwd"] = json!(list[0]);
    p["runtimeWorkspaceRoots"] = json!(list);
    Ok(())
}

/// `thread/start` の params に付ける。追加フォルダが無ければ何も付けない（省略。空配列は送らない）。
/// 追加フォルダがあれば `[cwd] + 追加分`（cwdと同じものは除く）を送る。
pub fn apply_start_folders(p: &mut Value, cwd: &str, extra: &[String]) -> BackendResult<()> {
    if extra.is_empty() {
        return Ok(());
    }
    if cwd.trim().is_empty() || extra.iter().any(|f| f.trim().is_empty()) {
        return Err(reject("空の作業フォルダを含むため送りません"));
    }
    let primary_key = path_key(&strip_verbatim(cwd));
    let mut list = vec![cwd.to_string()];
    for f in extra {
        let k = path_key(&strip_verbatim(f));
        if k == primary_key || list.iter().skip(1).any(|e| path_key(&strip_verbatim(e)) == k) {
            continue;
        }
        list.push(f.clone());
    }
    p["runtimeWorkspaceRoots"] = json!(list);
    Ok(())
}

/// 表示用に `\\?\` を外す（`\\?\UNC\…` はそのまま）。正規化できるものは正規化する。
fn strip_verbatim(s: &str) -> String {
    if let Some(n) = normalize_path(s) {
        return n;
    }
    match s.strip_prefix("\\\\?\\") {
        Some(rest) if !rest.to_ascii_lowercase().starts_with("unc\\") => rest.to_string(),
        _ => s.to_string(),
    }
}

/// `cwd`（プライマリ）と roots 配列から作業フォルダを組み立てる。
/// roots が配列でない・文字列以外を含む・cwd を含まない（プライマリを表せない）ときは `None`（取得できたことにしない）。
pub fn folders_from_parts(cwd: Option<&str>, roots: Option<&Value>) -> Option<WorkFolders> {
    let cwd = strip_verbatim(cwd.filter(|c| !c.is_empty())?);
    let roots: Vec<String> = roots?.as_array()?.iter().map(|v| v.as_str().filter(|s| !s.is_empty()).map(strip_verbatim)).collect::<Option<_>>()?;
    let cwd_key = path_key(&cwd);
    if !roots.iter().any(|r| path_key(r) == cwd_key) {
        return None;
    }
    let mut out = vec![cwd];
    for r in roots {
        let k = path_key(&r);
        if !out.iter().any(|o| path_key(o) == k) {
            out.push(r);
        }
    }
    Some(WorkFolders { folders: out })
}

/// `thread/start`・`thread/resume`・`thread/fork` の応答（`cwd`＋`runtimeWorkspaceRoots`）から。
pub fn folders_of_response(resp: &Value) -> Known<WorkFolders> {
    match folders_from_parts(resp.get("cwd").and_then(Value::as_str), resp.get("runtimeWorkspaceRoots")) {
        Some(v) => Known::direct(v),
        None => Known::Missing,
    }
}

/// `Thread`（`thread/read` の `thread`）の `environments` から（先頭環境の `cwd`＋`runtimeWorkspaceRoots`）。
/// `null`・項目なし＝未読込み／非公開で `NotFetched`。`[]`・形が合わないものは `Missing`。`environmentId` は読まない・保存しない。
pub fn folders_of_thread(thread: &Value) -> Known<WorkFolders> {
    let envs = match thread.get("environments") {
        None | Some(Value::Null) => return Known::NotFetched,
        Some(v) => v,
    };
    let first = envs.as_array().and_then(|a| a.first());
    let parsed = first.and_then(|e| folders_from_parts(e.get("cwd").and_then(Value::as_str), e.get("runtimeWorkspaceRoots")));
    match parsed {
        Some(v) => Known::direct(v),
        None => Known::Missing,
    }
}

/// 受理を確認した指定とバックエンドの現在値が一致するか（「反映を確認」の判定用。順序も含めて比較）。
pub fn same_folders(a: &WorkFolders, b: &WorkFolders) -> bool {
    a.folders.len() == b.folders.len() && a.folders.iter().zip(&b.folders).all(|(x, y)| path_key(&strip_verbatim(x)) == path_key(&strip_verbatim(y)))
}

/// `path` が一覧のいずれかの配下（または同じ）か。
pub fn covers(folders: &WorkFolders, path: &str) -> bool {
    let p = strip_verbatim(path);
    folders.folders.iter().any(|f| is_same_or_under(&p, &strip_verbatim(f)))
}

/// 追加フォルダ指定を伴う操作がexperimental未対応の拒否だったか（`-32601` も含む）。
pub fn is_roots_unsupported(e: &BackendError) -> bool {
    super::parity::is_experimental_rejection(e) || matches!(e, BackendError::Rejected { code: Some(-32601), .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(v: &[&str]) -> WorkFolders {
        WorkFolders { folders: v.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn turn_folders_send_cwd_and_all_roots_and_never_empty() {
        let mut p = json!({"threadId": "t"});
        apply_turn_folders(&mut p, &wf(&["C:\\a", "C:\\b"])).unwrap();
        assert_eq!(p["cwd"], "C:\\a");
        assert_eq!(p["runtimeWorkspaceRoots"], json!(["C:\\a", "C:\\b"]));
        let mut q = json!({});
        assert!(matches!(apply_turn_folders(&mut q, &wf(&[])), Err(BackendError::Rejected { .. })));
        assert!(q.get("runtimeWorkspaceRoots").is_none() && q.get("cwd").is_none());
        assert!(matches!(apply_turn_folders(&mut q, &wf(&["C:\\a", " "])), Err(BackendError::Rejected { .. })));
    }

    #[test]
    fn start_folders_omit_when_no_extra_and_put_cwd_first() {
        let mut p = json!({"cwd": "C:\\a"});
        apply_start_folders(&mut p, "C:\\a", &[]).unwrap();
        assert!(p.get("runtimeWorkspaceRoots").is_none(), "no extras means omitted, never []");
        apply_start_folders(&mut p, "C:\\a", &["C:\\b".into(), "c:\\A".into(), "C:\\b".into(), "D:\\c".into()]).unwrap();
        assert_eq!(p["runtimeWorkspaceRoots"], json!(["C:\\a", "C:\\b", "D:\\c"]));
        assert!(apply_start_folders(&mut json!({}), "C:\\a", &["".into()]).is_err());
    }

    #[test]
    fn response_roots_are_rebuilt_with_cwd_first_and_verbatim_stripped() {
        let r = json!({"cwd": "\\\\?\\C:\\a", "runtimeWorkspaceRoots": ["C:\\b", "C:\\a"]});
        assert_eq!(folders_of_response(&r), Known::direct(wf(&["C:\\a", "C:\\b"])));
        // cwdがrootsに無い（プライマリを表せない）・形が不正・欠落は取得できたことにしない。
        assert_eq!(folders_of_response(&json!({"cwd": "C:\\a", "runtimeWorkspaceRoots": ["C:\\b"]})), Known::Missing);
        assert_eq!(folders_of_response(&json!({"cwd": "C:\\a", "runtimeWorkspaceRoots": ["C:\\a", 1]})), Known::Missing);
        assert_eq!(folders_of_response(&json!({"cwd": "C:\\a"})), Known::Missing);
        assert_eq!(folders_of_response(&json!({"runtimeWorkspaceRoots": ["C:\\a"]})), Known::Missing);
    }

    #[test]
    fn thread_environments_distinguish_unloaded_from_missing() {
        let env = json!({"environmentId": "e1", "cwd": "C:\\a", "runtimeWorkspaceRoots": ["C:\\a", "C:\\b"]});
        assert_eq!(folders_of_thread(&json!({"id": "t", "environments": [env]})), Known::direct(wf(&["C:\\a", "C:\\b"])));
        // 未読込み・非公開は未確認（0件や空にしない）。
        assert_eq!(folders_of_thread(&json!({"id": "t", "environments": null})), Known::NotFetched);
        assert_eq!(folders_of_thread(&json!({"id": "t"})), Known::NotFetched);
        // 環境なし・形が合わない。
        assert_eq!(folders_of_thread(&json!({"environments": []})), Known::Missing);
        assert_eq!(folders_of_thread(&json!({"environments": [{"cwd": "C:\\a"}]})), Known::Missing);
    }

    #[test]
    fn same_and_covers_ignore_case_and_verbatim() {
        assert!(same_folders(&wf(&["c:/a", "C:\\b"]), &wf(&["\\\\?\\C:\\A", "C:\\B"])));
        assert!(!same_folders(&wf(&["C:\\a", "C:\\b"]), &wf(&["C:\\b", "C:\\a"])));
        assert!(covers(&wf(&["C:\\a"]), "C:\\a\\x"));
        assert!(!covers(&wf(&["C:\\a"]), "C:\\ab"));
    }

    #[test]
    fn unsupported_detection_covers_experimental_message_and_method_not_found() {
        let r = |code, m: &str| BackendError::Rejected { code, message: m.into() };
        assert!(is_roots_unsupported(&r(Some(-32600), "thread/start.runtimeWorkspaceRoots requires experimentalApi capability")));
        assert!(is_roots_unsupported(&r(Some(-32601), "method not found")));
        assert!(!is_roots_unsupported(&r(Some(-32600), "Invalid request: AbsolutePathBuf deserialized without a base path")));
        assert!(!is_roots_unsupported(&BackendError::NotConnected));
    }
}

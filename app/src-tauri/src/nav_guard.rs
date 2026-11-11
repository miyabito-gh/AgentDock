//! 本体の特権WebView（main・monitor・pet）が、アプリ自身のURL以外へ遷移しないようにする（多層防御）。
//! Markdown プレビューなど本体内で描く内容がリンク等から外部へ遷移しても、ここで拒否する。
//! 対象は label で判定する。Visual Editor の HTML プレビュー用の子WebView（label は `visual-preview-*`）は対象外で、従来どおり。
use tauri::{plugin::{Builder, TauriPlugin}, Runtime, Url};

const GUARDED: [&str; 3] = ["main", "monitor", "pet"];

pub fn guarded_label(label: &str) -> bool {
    GUARDED.contains(&label)
}

/// アプリ自身のオリジンか。`tauri://localhost`、`http(s)://tauri.localhost`（Windows）、`about:blank`、開発時だけ devUrl（localhost:5173）。
pub fn app_url_allowed(url: &Url, dev: bool) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost") || (dev && url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1")) && url.port() == Some(5173)),
        "about" => url.as_str() == "about:blank",
        _ => false,
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("nav-guard")
        .on_navigation(|webview, url| {
            if !guarded_label(webview.label()) || app_url_allowed(url, cfg!(debug_assertions)) {
                return true;
            }
            // 記録はスキームとホストだけ（パス・クエリは残さない）。
            crate::diag::log("navigation-blocked", &format!("{} {}://{}", webview.label(), url.scheme(), url.host_str().unwrap_or("")));
            false
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ok(u: &str, dev: bool) -> bool { app_url_allowed(&u.parse().unwrap(), dev) }
    #[test]
    fn app_origins_are_allowed() {
        assert!(ok("tauri://localhost/index.html", false));
        assert!(ok("http://tauri.localhost/index.html?view=pet", false));
        assert!(ok("https://tauri.localhost/", false));
        assert!(ok("about:blank", false));
    }
    #[test]
    fn external_and_look_alike_urls_are_blocked() {
        for u in ["https://example.com/", "http://tauri.localhost.evil.com/", "https://evil.com/tauri.localhost", "file:///C:/x.html", "data:text/html,x", "javascript:alert(1)", "tauri://evil/", "ftp://tauri.localhost/", "about:srcdoc"] {
            assert!(!ok(u, false), "{u}");
        }
    }
    #[test]
    fn dev_url_only_in_debug() {
        assert!(ok("http://localhost:5173/", true));
        assert!(!ok("http://localhost:5173/", false));
        assert!(!ok("http://localhost:8080/", true));
    }
    #[test]
    fn only_privileged_labels_are_guarded() {
        for l in ["main", "monitor", "pet"] { assert!(guarded_label(l)); }
        assert!(!guarded_label("visual-preview-abc"));
    }
}

//! Read-only native preview. Selection is untrusted context, never authority.
mod assets;
#[cfg(windows)]
mod native;
mod selection;
use super::{ProjectRoot, RelativePath};
use serde::{Deserialize, Serialize};
use tauri::{
    webview::WebviewBuilder, AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl,
};
use tokio::sync::Mutex;
#[derive(Default)]
pub struct PreviewManager(Mutex<Option<Session>>);
struct Session {
    id: String,
    root: ProjectRoot,
    html: String,
    view: Webview,
    server: assets::Server,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Bounds {
    fn check(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0 || *v > 100000.0)
            || self.width < 1.0
            || self.height < 1.0
        {
            Err("invalid preview bounds".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    session_id: String,
    root_path: String,
    html_path: String,
}
/// Application commands have no automatic ACL manifest. Gate the *entire* handler.
pub fn application_invoke_allowed(label: &str) -> bool {
    matches!(label, "main" | "monitor")
}
pub fn guarded_invoke_handler<F: Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static>(
    handler: F,
) -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    move |invoke| {
        if !application_invoke_allowed(invoke.message.webview_ref().label()) {
            invoke
                .resolver
                .reject("application commands are unavailable to this webview");
            return true;
        }
        handler(invoke)
    }
}
fn controller(webview: &Webview) -> Result<(), String> {
    if webview.label() == "main" {
        Ok(())
    } else {
        Err("preview control requires the main webview".into())
    }
}
fn close(session: &Session) -> Result<(), String> {
    if session
        .view
        .app_handle()
        .get_webview(session.view.label())
        .is_none()
    {
        return Ok(());
    }
    session.view.hide().map_err(|e| e.to_string())?;
    session.view.close().map_err(|e| e.to_string())
}
/// Only strip the canonical drive prefix. UNC/device namespaces remain rejected.
pub(crate) fn normal_drive_path(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        match path.components().next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(_) => Ok(path.to_path_buf()),
                Prefix::VerbatimDisk(_) => Ok(std::path::PathBuf::from(
                    path.as_os_str()
                        .to_string_lossy()
                        .strip_prefix(r"\\?\")
                        .ok_or("invalid drive path")?,
                )),
                _ => Err("HTML path must use a local drive".into()),
            },
            _ => Err("HTML path must use an absolute local drive".into()),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(path.to_path_buf())
    }
}
fn display_path(path: &std::path::Path) -> Result<String, String> {
    Ok(normal_drive_path(path)?.to_string_lossy().into_owned())
}
pub(crate) fn relative_entry(root: &std::path::Path, supplied: &str) -> Result<RelativePath, String> {
    let candidate = std::path::Path::new(supplied);
    if !candidate.is_absolute() {
        return RelativePath::parse(&supplied.replace('\\', "/")).map_err(|e| e.to_string());
    }
    let root = normal_drive_path(root)?;
    let candidate = normal_drive_path(candidate)?;
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
        || !super::scope::root_contains(&root, &candidate)
    {
        return Err("HTML is outside selected root".into());
    }
    let relative: std::path::PathBuf = candidate
        .components()
        .skip(root.components().count())
        .collect();
    RelativePath::parse(&relative.to_string_lossy().replace('\\', "/")).map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn visual_preview_open(
    webview: Webview,
    app: AppHandle,
    state: tauri::State<'_, PreviewManager>,
    root_path: String,
    html_path: String,
    bounds: Bounds,
) -> Result<OpenResult, String> {
    controller(&webview)?;
    bounds.check()?;
    let mut slot = state.0.lock().await;
    if let Some(old) = slot.as_ref() {
        close(old)?;
    }
    *slot = None;
    let root = ProjectRoot::open(std::path::Path::new(&root_path)).map_err(|e| e.to_string())?;
    let html = relative_entry(root.path(), &html_path)?;
    if !matches!(
        html.as_str()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "html" | "htm"
    ) {
        return Err("preview entry must be HTML".into());
    }
    let assets = assets::capture(&root)?;
    // Match the actual captured spelling after Windows path comparison.
    let html = assets
        .keys()
        .find(|p| p.to_uppercase() == html.as_str().to_uppercase())
        .ok_or("selected HTML was not found in readable preview assets")?;
    let html = RelativePath::parse(html).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let id = format!("{:?}", unsafe {
        windows::Win32::System::Com::CoCreateGuid().map_err(|e| e.to_string())?
    })
    .replace(['{', '}', '-'], "")
    .to_lowercase();
    #[cfg(not(windows))]
    return Err("native preview requires Windows".into());
    #[cfg(windows)]
    {
        let server = assets::Server::start(assets, id.clone())?;
        let url = server.url(html.as_str());
        let window = app.get_window("main").ok_or("main window unavailable")?;
        let profile = app
            .path()
            .app_local_data_dir()
            .map_err(|e| e.to_string())?
            .join("visual-preview")
            .join("profile");
        let view = window
            .add_child(
                WebviewBuilder::new(
                    format!("visual-preview-{id}"),
                    WebviewUrl::External("about:blank".parse().unwrap()),
                )
                .data_directory(profile)
                .initialization_script(include_str!("selection.js"))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .focused(false)
                .disable_drag_drop_handler(),
                LogicalPosition::new(bounds.x, bounds.y),
                LogicalSize::new(bounds.width, bounds.height),
            )
            .map_err(|e| e.to_string())?;
        // The about:blank child must stay hidden until a controller bounds update.
        if let Err(e) = view.hide() {
            let _ = view.close();
            return Err(e.to_string());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let native_id = id.clone();
        let handle = app.clone();
        let result = view.with_webview(move |platform| {
            let result = unsafe {
                platform.controller().CoreWebView2().and_then(|core| {
                    native::install(core, platform.environment(), url, native_id, handle)
                })
            }
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        let session = Session {
            id: id.clone(),
            root,
            html: html.as_str().into(),
            view,
            server,
        };
        let root_path = display_path(session.root.path())?;
        let html_path = display_path(&html.join(session.root.path()))?;

        // Preserve the hidden session if cleanup fails so a new open cannot orphan it.
        *slot = Some(session);
        let installed = match result {
            Ok(()) => rx
                .await
                .map_err(|_| "native preview installation did not complete".to_string())
                .and_then(|v| v),
            Err(e) => Err(e.to_string()),
        };
        if let Err(error) = installed {
            if close(slot.as_ref().unwrap()).is_ok() {
                *slot = None;
            }
            return Err(error);
        }
        Ok(OpenResult {
            session_id: id,
            root_path,
            html_path,
        })
    }
}
#[tauri::command]
pub async fn visual_preview_bounds(
    webview: Webview,
    state: tauri::State<'_, PreviewManager>,
    session_id: String,
    bounds: Bounds,
    visible: bool,
) -> Result<(), String> {
    controller(&webview)?;
    bounds.check()?;
    let slot = state.0.lock().await;
    let s = slot
        .as_ref()
        .filter(|s| s.id == session_id)
        .ok_or("preview session unavailable")?;
    if !visible {
        s.view.hide().map_err(|e| e.to_string())?;
    }
    s.view
        .set_position(LogicalPosition::new(bounds.x, bounds.y))
        .map_err(|e| e.to_string())?;
    s.view
        .set_size(LogicalSize::new(bounds.width, bounds.height))
        .map_err(|e| e.to_string())?;
    if visible {
        s.view.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[tauri::command]
pub async fn visual_preview_action(
    webview: Webview,
    state: tauri::State<'_, PreviewManager>,
    session_id: String,
    action: String,
) -> Result<(), String> {
    controller(&webview)?;
    let slot = state.0.lock().await;
    let s = slot
        .as_ref()
        .filter(|s| s.id == session_id)
        .ok_or("preview session unavailable")?;
    match action.as_str() {
        "reload" => {
            let assets = assets::capture(&s.root)?;
            if !assets.contains_key(&s.html) {
                return Err("selected HTML disappeared".into());
            }
            *s.server
                .assets
                .write()
                .map_err(|_| "preview asset lock failed")? = assets;
            s.view.reload().map_err(|e| e.to_string())?;
        }
        "select" | "interact" | "parent" | "clear" => {
            s.view.eval(format!("window.dispatchEvent(new CustomEvent('agentdock-preview-action',{{detail:{}}}));",serde_json::to_string(&action).unwrap())).map_err(|e|e.to_string())?;
        }
        _ => return Err("invalid preview action".into()),
    }
    Ok(())
}
#[tauri::command]
pub async fn visual_preview_close(
    webview: Webview,
    state: tauri::State<'_, PreviewManager>,
    session_id: String,
) -> Result<(), String> {
    controller(&webview)?;
    let mut slot = state.0.lock().await;
    if let Some(s) = slot.as_ref() {
        if s.id != session_id {
            return Err("preview session mismatch".into());
        }
        close(s)?;
        *slot = None;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_and_preview_webviews_cannot_invoke_application_commands() {
        for label in [
            "visual-preview-x",
            "",
            "main-child",
            "MAIN",
            "monitor-child",
        ] {
            assert!(!application_invoke_allowed(label));
        }
        assert!(application_invoke_allowed("main"));
        assert!(application_invoke_allowed("monitor"));
    }
    #[cfg(windows)]
    #[test]
    fn canonical_drive_entries_round_trip_across_root_changes_and_case() {
        let root = std::path::Path::new(r"\\?\C:\work\page");
        for html in [
            r"C:\work\page\index.html",
            r"\\?\C:\work\page\index.html",
            r"c:\WORK\PAGE\index.html",
        ] {
            assert_eq!(relative_entry(root, html).unwrap().as_str(), "index.html");
        }
        assert_eq!(
            relative_entry(
                std::path::Path::new(r"C:\work"),
                r"\\?\C:\work\page\index.html"
            )
            .unwrap()
            .as_str(),
            "page/index.html"
        );
        assert_eq!(display_path(root).unwrap(), r"C:\work\page");
        for html in [
            r"C:\work\page2\index.html",
            r"C:\work\page\..\outside.html",
            r"\\server\share\index.html",
            r"\\?\UNC\server\share\index.html",
            r"\\?\GLOBALROOT\Device\index.html",
        ] {
            assert!(relative_entry(root, html).is_err(), "{html}");
        }
    }
}

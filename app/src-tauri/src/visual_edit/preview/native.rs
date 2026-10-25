use tauri::{AppHandle, Emitter, Manager};
use webview2_com::{
    DownloadStartingEventHandler, Microsoft::Web::WebView2::Win32::*,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::{
    core::{w, Interface, PCWSTR, PWSTR},
    Win32::System::Com::{CoTaskMemFree, IStream},
};
unsafe fn read_string(
    get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>,
) -> windows::core::Result<String> {
    let mut p = PWSTR::null();
    get(&mut p)?;
    if p.is_null() {
        return Ok(String::new());
    }
    // COM owns the terminated UTF-16 string. Bound scanning/allocation before parsing untrusted messages.
    let mut len = 0;
    while len <= 65536 && *p.as_ptr().add(len) != 0 {
        len += 1;
    }
    if len > 65536 {
        CoTaskMemFree(Some(p.as_ptr().cast()));
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80070057u32 as i32,
        )));
    }
    let s = String::from_utf16(std::slice::from_raw_parts(p.as_ptr(), len));
    CoTaskMemFree(Some(p.as_ptr().cast()));
    Ok(s?)
}
#[derive(Clone)]
struct StopTarget {
    app: AppHandle,
    session: String,
}
impl StopTarget {
    fn guard(&self, result: windows::core::Result<()>) -> windows::core::Result<()> {
        if result.is_err() {
            // Use the Tauri lifecycle so its webview registry also observes destruction.
            if let Some(view) = self
                .app
                .get_webview(&format!("visual-preview-{}", self.session))
            {
                let _ = view.hide();
                let _ = view.close();
            }
            let _=self.app.emit_to(tauri::EventTarget::webview("main"),"visual-preview-error",serde_json::json!({"sessionId":self.session,"error":"安全制限を適用できず、プレビューの停止を確認できません。閉じるを再試行してください。"}));
        }
        result
    }
}
fn resource_allowed(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "data" | "blob")
        || (matches!(url.scheme(), "http" | "https")
            && !matches!(
                url.host_str(),
                Some("tauri.localhost" | "ipc.localhost" | "asset.localhost")
            ))
}
fn root_relative_redirect(url: &tauri::Url, entry: &tauri::Url, prefix: &str) -> Option<String> {
    if url.origin() != entry.origin() || url.path().starts_with(prefix) {
        return None;
    }
    let mut target = url.clone();
    target.set_path(&format!("{}{}", prefix, url.path().trim_start_matches('/')));
    Some(target.to_string())
}
/// Install every native denial before the first untrusted navigation.
pub(super) unsafe fn install(
    core: ICoreWebView2,
    environment: ICoreWebView2Environment,
    url: String,
    session: String,
    app: AppHandle,
) -> windows::core::Result<()> {
    let settings = core.Settings()?;
    settings.SetAreHostObjectsAllowed(false)?;
    settings.SetIsWebMessageEnabled(true)?;
    let mut token = 0;
    let stop = StopTarget {
        app: app.clone(),
        session: session.clone(),
    };
    let response = environment.CreateWebResourceResponse(
        None::<&IStream>,
        403,
        w!("Forbidden"),
        w!("Content-Length: 0\r\nCache-Control: no-store"),
    )?;
    let request_stop = stop.clone();
    let request_environment = environment.clone();
    let entry = tauri::Url::parse(&url).unwrap();
    let prefix = format!("/{session}/");
    core.add_WebResourceRequested(&WebResourceRequestedEventHandler::create(Box::new(move |_,args| {
        if let Some(args)=args {
            request_stop.guard(args.SetResponse(&response))?;
            let uri=read_string(|p|args.Request()?.Uri(p))?;
            if let Ok(uri)=tauri::Url::parse(&uri) {if resource_allowed(&uri) {
                if let Some(target)=root_relative_redirect(&uri,&entry,&prefix) {
                    // Root-relative URLs retain root semantics without exposing a prefixless server endpoint.
                    let headers=format!("Location: {target}\r\nContent-Length: 0\r\nCache-Control: no-store\r\n");let wide:Vec<u16>=headers.encode_utf16().chain([0]).collect();
                    let redirect=request_environment.CreateWebResourceResponse(None::<&IStream>,302,w!("Found"),PCWSTR(wide.as_ptr()))?;
                    request_stop.guard(args.SetResponse(&redirect))?;
                } else {args.SetResponse(None::<&ICoreWebView2WebResourceResponse>)?;}
            }}
        } Ok(())
    })),&mut token)?;
    core.cast::<ICoreWebView2_22>()?
        .AddWebResourceRequestedFilterWithRequestSourceKinds(
            w!("*"),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        )?;
    let expected = tauri::Url::parse(&url).unwrap();
    let navigation_stop = stop.clone();
    core.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                navigation_stop.guard(args.SetCancel(true))?;
                let uri = read_string(|p| args.Uri(p))?;
                let allow = tauri::Url::parse(&uri)
                    .map(|mut u| {
                        u.set_query(None);
                        u.set_fragment(None);
                        u == expected
                    })
                    .unwrap_or(false);
                if allow {
                    args.SetCancel(false)?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let frame_stop = stop.clone();
    core.add_FrameNavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                frame_stop.guard(args.SetCancel(true))?;
                let uri = read_string(|p| args.Uri(p))?;
                if uri == "about:blank"
                    || tauri::Url::parse(&uri)
                        .map(|u| resource_allowed(&u))
                        .unwrap_or(false)
                {
                    args.SetCancel(false)?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let popup_stop = stop.clone();
    core.add_NewWindowRequested(
        &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                popup_stop.guard(args.SetHandled(true))?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let download_stop = stop;
    core.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
        &DownloadStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                download_stop.guard(args.SetCancel(true))?;
                args.SetHandled(true)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let source = tauri::Url::parse(&url).unwrap();
    let mut budget = super::selection::MessageBudget::new();
    core.add_WebMessageReceived(
        &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            if !budget.admit(std::time::Instant::now()) {
                return Ok(());
            }
            if let Some(args) = args {
                let uri = read_string(|p| args.Source(p))?;
                let allowed = tauri::Url::parse(&uri)
                    .map(|mut u| {
                        u.set_query(None);
                        u.set_fragment(None);
                        u == source
                    })
                    .unwrap_or(false);
                if allowed {
                    let raw = read_string(|p| args.TryGetWebMessageAsString(p))?;
                    if let Some((selection, truncated)) = super::selection::validate(&raw) {
                        let event = super::selection::SelectionEvent {
                            session_id: session.clone(),
                            selection,
                            truncated,
                        };
                        if serde_json::to_vec(&event)
                            .map(|v| v.len() <= 65536)
                            .unwrap_or(false)
                        {
                            let _ = app.emit_to(
                                tauri::EventTarget::webview("main"),
                                "visual-preview-selection",
                                event,
                            );
                        }
                    }
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let wide: Vec<u16> = url.encode_utf16().chain([0]).collect();
    core.Navigate(PCWSTR(wide.as_ptr()))?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_resources_and_root_relative_urls_preserve_ordinary_page_scope() {
        let entry = tauri::Url::parse("http://127.0.0.1:12345/token/nested/index.html").unwrap();
        let root = tauri::Url::parse("http://127.0.0.1:12345/assets/a.css?v=1").unwrap();
        assert_eq!(
            root_relative_redirect(&root, &entry, "/token/").as_deref(),
            Some("http://127.0.0.1:12345/token/assets/a.css?v=1")
        );
        for uri in [
            "https://cdn.example/a.js",
            "http://localhost:5678/api",
            "http://127.0.0.1:5678/api",
            "data:image/png;base64,AA==",
            "blob:https://example.com/id",
        ] {
            let url = tauri::Url::parse(uri).unwrap();
            assert!(resource_allowed(&url));
            assert!(root_relative_redirect(&url, &entry, "/token/").is_none());
        }
        for uri in [
            "file:///C:/secret.txt",
            "tauri://localhost",
            "ipc://localhost",
            "http://tauri.localhost",
            "http://ipc.localhost",
            "http://asset.localhost",
        ] {
            assert!(!resource_allowed(&tauri::Url::parse(uri).unwrap()));
        }
    }
}

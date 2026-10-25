//! Opt-in V0 experiment. Does not import AgentDock's library, commands or plugins.
#[cfg(not(windows))]
fn main() {
    panic!("visual_preview_probe requires Windows WebView2");
}
#[cfg(windows)]
fn main() {
    probe::run();
}
#[cfg(windows)]
mod probe {
    use serde_json::{json, Value};
    use std::{
        collections::BTreeMap,
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };
    use tauri::{webview::WebviewBuilder, LogicalPosition, LogicalSize, WebviewUrl, WindowBuilder};
    use webview2_com::{
        DownloadStartingEventHandler, Microsoft::Web::WebView2::Win32::*,
        NavigationStartingEventHandler, NewWindowRequestedEventHandler,
        WebMessageReceivedEventHandler, WebResourceRequestedEventHandler,
    };
    use windows::{
        core::{w, Interface, PCWSTR, PWSTR},
        Win32::System::Com::{CoCreateGuid, CoTaskMemFree, IStream},
    };
    type Events = Arc<Mutex<Vec<Value>>>;
    fn event(events: &Events, value: Value) {
        let mut log = events.lock().unwrap();
        if log.len() < 256 {
            log.push(value);
        }
    }
    // WebView2 out strings are CoTaskMem-allocated. Copy and release once.
    unsafe fn read_string(
        get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>,
    ) -> windows::core::Result<String> {
        let mut raw = PWSTR::null();
        get(&mut raw)?;
        let value = raw.to_string();
        CoTaskMemFree(Some(raw.as_ptr().cast()));
        Ok(value?)
    }
    fn fail_closed(result: windows::core::Result<()>, events: &Events) {
        if let Err(error) = result {
            event(
                events,
                json!({"kind":"fatal-native-denial-error", "error":error.to_string()}),
            );
            eprintln!("native default denial failed; terminating isolated probe: {error}");
            std::process::exit(4);
        }
    }
    fn pcw(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([0]).collect()
    }
    fn classify(uri: &str, origin: &str, token: &str) -> String {
        uri.strip_prefix(&format!("{origin}/{token}/"))
            .map(|path| format!("fixture/{path}"))
            .unwrap_or_else(|| uri.chars().take(180).collect())
    }
    pub fn run() {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let layered_csp = match arguments.as_slice() {
            [] => false,
            [argument] if argument == "--layered-csp" => true,
            _ => {
                eprintln!("usage: visual_preview_probe [--layered-csp]");
                std::process::exit(64);
            }
        };
        let run_id = unsafe {
            CoCreateGuid()
                .expect("probe run GUID")
                .to_u128()
                .to_string()
        };
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/visual-probe")
            .join(format!("run-{run_id}"));
        fs::create_dir_all(&root).expect("isolated probe directory");
        let events: Events = Arc::default();
        let server = TcpListener::bind("127.0.0.1:0").expect("fixture loopback bind");
        let canary = TcpListener::bind("127.0.0.1:0").expect("canary loopback bind");
        let origin = format!("http://{}", server.local_addr().unwrap());
        let attack_origin = format!("http://{}", canary.local_addr().unwrap());
        let token = unsafe { CoCreateGuid().expect("probe GUID").to_u128().to_string() };
        let prefix = format!("/{token}/");
        let static_base = format!("{origin}{prefix}");
        let csp_header = if layered_csp {
            format!("Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline' {static_base}fixture.mjs; style-src {static_base}fixture.css; img-src {static_base}fixture.svg; font-src 'none'; connect-src {static_base}redirect.json; frame-src {static_base}fixture.html; object-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'\r\n")
        } else {
            String::new()
        };
        let fixtures = BTreeMap::from([
            (
                "shell.html",
                (
                    "text/html",
                    include_str!("../../verification/visual-editor/shell.html").to_owned(),
                ),
            ),
            (
                "fixture.html",
                (
                    "text/html",
                    include_str!("../../verification/visual-editor/fixture.html")
                        .replace("@@CANARY@@", &attack_origin),
                ),
            ),
            (
                "fixture.css",
                (
                    "text/css",
                    include_str!("../../verification/visual-editor/fixture.css").to_owned(),
                ),
            ),
            (
                "fixture.mjs",
                (
                    "text/javascript",
                    include_str!("../../verification/visual-editor/fixture.mjs").to_owned(),
                ),
            ),
            (
                "fixture.svg",
                (
                    "image/svg+xml",
                    include_str!("../../verification/visual-editor/fixture.svg").to_owned(),
                ),
            ),
        ]);
        let mut allowed: Vec<String> = fixtures
            .keys()
            .map(|path| format!("{origin}{prefix}{path}"))
            .collect();
        allowed.push(format!("{origin}{prefix}redirect.json"));
        let shell_url = format!("{origin}{prefix}shell.html");
        let frame_url = format!("{origin}{prefix}fixture.html");
        let redirect_target = format!("{attack_origin}/redirect");
        let server_events = events.clone();
        thread::spawn(move || {
            for incoming in server.incoming() {
                let Ok(mut stream) = incoming else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..count]);
                let mut fields = request.lines().next().unwrap_or("").split_whitespace();
                let method = fields.next().unwrap_or("");
                let path = fields.next().unwrap_or("");
                if method == "GET" && path.strip_prefix(&prefix) == Some("redirect.json") {
                    let response = format!("HTTP/1.1 302 Found\r\n{csp_header}Location: {redirect_target}\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n");
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                let asset = path
                    .strip_prefix(&prefix)
                    .and_then(|name| fixtures.get(name));
                if method == "GET" {
                    if let Some((mime, body)) = asset {
                        let headers = format!("HTTP/1.1 200 OK\r\n{csp_header}Content-Type: {mime}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(headers.as_bytes());
                        let _ = stream.write_all(body.as_bytes());
                        continue;
                    }
                }
                event(&server_events, json!({"kind":"server-denied-request"}));
                let _ = stream.write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let canary_events = events.clone();
        thread::spawn(move || {
            for incoming in canary.incoming() {
                let Ok(mut stream) = incoming else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
                let mut buffer = [0; 512];
                let (count, read_outcome) = match stream.read(&mut buffer) {
                    Ok(count) => (count, if count == 0 { "closed" } else { "read" }),
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        (0, "timeout")
                    }
                    Err(_) => (0, "read-error"),
                };
                let request = String::from_utf8_lossy(&buffer[..count]);
                let first_line = request.lines().next().unwrap_or("");
                // A TCP connection remains a failed boundary even when no HTTP bytes arrive.
                event(
                    &canary_events,
                    json!({"kind":"CANARY_REACHED", "readBytes":count,
                    "readOutcome":read_outcome,"firstHttpLine":first_line}),
                );
                let _ = stream.write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let output = root.join("report.json");
        println!("V0 report: {}", output.display());
        // Also bound WebView/environment startup, before an AppHandle is available.
        let watchdog_output = output.clone();
        let watchdog_events = events.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(20));
            let report = json!({"schema":1,"status":"watchdog-timeout",
                "events":watchdog_events.lock().unwrap().clone()});
            let _ = fs::write(watchdog_output, serde_json::to_vec_pretty(&report).unwrap());
            eprintln!("isolated V0 probe startup/exit timeout");
            std::process::exit(5);
        });
        let setup_events = events.clone();
        let report_events = events.clone();
        let report_root = root.clone();
        let context = tauri::generate_context!(
            "../verification/visual-editor/tauri.conf.json",
            capabilities = []
        );
        tauri::Builder::default()
            .setup(move |app| {
                let window = WindowBuilder::new(app, "visual-probe-host")
                    .inner_size(1000.0, 700.0).visible(false).build()?;
                let preview = window.add_child(
                    WebviewBuilder::new("visual-probe", WebviewUrl::External("about:blank".parse()?))
                        .data_directory(root.join("webview-profile")),
                    LogicalPosition::new(180.0, 80.0), LogicalSize::new(640.0, 520.0),
                )?;
                let install_events = setup_events.clone();
                let handle = app.handle().clone();
                preview.with_webview(move |platform| {
                    let mut bounds = Default::default();
                    if unsafe { platform.controller().Bounds(&mut bounds) }.is_ok() {
                        event(&install_events, json!({"kind":"child-native-bounds","left":bounds.left,"top":bounds.top,"right":bounds.right,"bottom":bounds.bottom}));
                    }
                    let result = unsafe {
                        install(
                            platform.controller().CoreWebView2(),
                            platform.environment(),
                            allowed,
                            shell_url,
                            frame_url,
                            origin,
                            token,
                            install_events.clone(),
                        )
                    };
                    if let Err(error) = result {
                        // Fail closed: install only navigates after every handler succeeds.
                        event(
                            &install_events,
                            json!({"kind":"installation-failed", "error":error.to_string()}),
                        );
                    }
                })?;
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(6));
                    let observations = report_events.lock().unwrap().clone();
                    let canary_reached = observations
                        .iter()
                        .any(|value| value["kind"] == "CANARY_REACHED");
                    let installed = observations
                        .iter()
                        .any(|value| value["kind"] == "native-installed-before-navigation")
                        && !observations.iter().any(|value| value["kind"] == "installation-failed");
                    let report = json!({"schema":1,"probe":"isolated-child-webview","csp":layered_csp,
                    "status":"observations-only-not-V0-acceptance", "canaryReached":canary_reached,
                    "productionIpcTested":false,"centralEmbedTested":false,
                    "profile":report_root.join("webview-profile"),"events":observations});
                    match fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()) {
                        Ok(()) => {
                            println!("{}", report);
                            handle.exit(if canary_reached || !installed { 2 } else { 0 });
                        }
                        Err(error) => {
                            eprintln!("report write failed: {error}");
                            handle.exit(3);
                        }
                    }
                });
                Ok(())
            })
            .run(context)
            .expect("isolated V0 probe runtime");
    }
    unsafe fn install(
        core: windows::core::Result<ICoreWebView2>,
        environment: ICoreWebView2Environment,
        allowed: Vec<String>,
        shell_url: String,
        frame_url: String,
        origin: String,
        token: String,
        events: Events,
    ) -> windows::core::Result<()> {
        let core = core?;
        let settings = core.Settings()?;
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetAreDevToolsEnabled(false)?;
        let mut web_message_enabled = false.into();
        settings.IsWebMessageEnabled(&mut web_message_enabled)?;
        settings.SetIsWebMessageEnabled(true)?;
        event(
            &events,
            json!({"kind":"web-message-setting", "before":web_message_enabled.as_bool(), "configured":true}),
        );
        // Web messages carry bounded observations only. No application handler is registered.
        // add_* retains the COM handler until the WebView is destroyed at process exit.
        let mut registration = 0;
        let request_events = events.clone();
        let deny_response = environment.CreateWebResourceResponse(
            None::<&IStream>,
            403,
            w!("Forbidden"),
            w!("Content-Length: 0\r\nCache-Control: no-store"),
        )?;
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                fail_closed(args.SetResponse(&deny_response), &request_events);
                let request = args.Request()?;
                let uri = read_string(|value| request.Uri(value))?;
                let method = read_string(|value| request.Method(value))?;
                let permitted = method == "GET" && allowed.contains(&uri);
                event(
                    &request_events,
                    json!({"kind":"native-resource", "allowed":permitted,
                "method":method,"uri":classify(&uri,&origin,&token)}),
                );
                if permitted {
                    args.SetResponse(None::<&ICoreWebView2WebResourceResponse>)?;
                }
                Ok(())
            })),
            &mut registration,
        )?;
        // Older ALL resource-context filters do not cover every request source kind.
        // An old runtime that lacks _22 aborts before untrusted navigation.
        core.cast::<ICoreWebView2_22>()?
            .AddWebResourceRequestedFilterWithRequestSourceKinds(
                w!("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?;
        let navigation_events = events.clone();
        let expected_shell = shell_url.clone();
        let mut shell_started = false;
        core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    fail_closed(args.SetCancel(true), &navigation_events);
                    let uri = read_string(|value| args.Uri(value))?;
                    let allow = !shell_started && uri == expected_shell;
                    shell_started |= allow;
                    args.SetCancel(!allow)?;
                    event(
                        &navigation_events,
                        json!({"kind":"top-navigation","allowed":allow}),
                    );
                }
                Ok(())
            })),
            &mut registration,
        )?;
        let frame_events = events.clone();
        let mut frame_started = false;
        core.add_FrameNavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    fail_closed(args.SetCancel(true), &frame_events);
                    let uri = read_string(|value| args.Uri(value))?;
                    let allow = !frame_started && uri == frame_url;
                    frame_started |= allow;
                    args.SetCancel(!allow)?;
                    event(
                        &frame_events,
                        json!({"kind":"frame-navigation","allowed":allow}),
                    );
                }
                Ok(())
            })),
            &mut registration,
        )?;
        let popup_events = events.clone();
        core.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    fail_closed(args.SetHandled(true), &popup_events);
                }
                event(&popup_events, json!({"kind":"new-window-blocked"}));
                Ok(())
            })),
            &mut registration,
        )?;
        let download_events = events.clone();
        core.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    fail_closed(args.SetCancel(true), &download_events);
                    args.SetHandled(true)?;
                }
                event(&download_events, json!({"kind":"download-blocked"}));
                Ok(())
            })),
            &mut registration,
        )?;
        let message_events = events.clone();
        core.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let message = read_string(|value| args.TryGetWebMessageAsString(value))?;
                    if message.len() <= 32768 {
                        event(
                            &message_events,
                            json!({"kind":"untrusted-page-report","json":message}),
                        );
                    }
                }
                Ok(())
            })),
            &mut registration,
        )?;
        event(
            &events,
            json!({"kind":"native-installed-before-navigation"}),
        );
        let url = pcw(&shell_url);
        core.Navigate(PCWSTR(url.as_ptr()))?;
        Ok(())
    }
}

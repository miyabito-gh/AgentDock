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
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tauri::{webview::WebviewBuilder, LogicalPosition, LogicalSize, WebviewUrl, WindowBuilder};
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DownloadStartingEventHandler,
        Microsoft::Web::WebView2::Win32::*, NavigationStartingEventHandler,
        NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
        WebResourceRequestedEventHandler,
    };
    use windows::{
        core::{w, Interface, PCWSTR, PWSTR},
        Win32::System::Com::{
            CoCreateGuid, CoTaskMemFree, IStream, StructuredStorage::CreateStreamOnHGlobal,
            STREAM_SEEK_SET,
        },
    };
    type Events = Arc<Mutex<Vec<Value>>>;
    fn event(events: &Events, value: Value) {
        let mut log = events.lock().unwrap();
        if log.len() < 256 {
            log.push(value);
        }
    }
    const CANARY_LABELS: [&str; 5] = ["fetch", "image", "nested-frame", "popup", "redirect"];
    const CANARY_CONNECTION_LIMIT: usize = 32;
    fn unix_millis() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    }
    fn monitor_canary(
        label: &'static str,
        listener: TcpListener,
        stop: Arc<AtomicBool>,
        observations: Events,
        events: Events,
    ) -> thread::JoinHandle<bool> {
        listener
            .set_nonblocking(true)
            .expect("nonblocking canary listener");
        thread::spawn(move || {
            let mut accepted = 0;
            while !stop.load(Ordering::Acquire) && accepted < CANARY_CONNECTION_LIMIT {
                let (mut stream, peer) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => {
                        event(
                            &events,
                            json!({"kind":"canary-listener-error", "label":label,
                            "atUnixMs":unix_millis(), "error":error.to_string()}),
                        );
                        return false;
                    }
                };
                accepted += 1;
                let accepted_at = unix_millis();
                let connection = json!({"label":label, "connection":accepted, "peer":peer.to_string(),
                    "acceptedAtUnixMs":accepted_at, "readCompletedAtUnixMs":null,
                    "readBytes":null, "readOutcome":"pending", "firstHttpLine":null});
                // Dedicated bounded records cannot be displaced by noisy page/native events.
                // Publish acceptance before I/O: a pending read is already a boundary failure.
                let index = {
                    let mut log = observations.lock().unwrap();
                    let index = log.len();
                    log.push(connection);
                    index
                };
                event(
                    &events,
                    json!({"kind":"CANARY_REACHED", "label":label,
                    "connection":accepted, "acceptedAtUnixMs":accepted_at}),
                );
                let mut buffer = [0; 512];
                let (count, read_outcome) = match stream
                    .set_nonblocking(false)
                    .and_then(|()| stream.set_read_timeout(Some(Duration::from_millis(150))))
                {
                    Err(_) => (0, "timeout-configuration-error"),
                    Ok(()) => match stream.read(&mut buffer) {
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
                    },
                };
                let completed_at = unix_millis();
                let request = String::from_utf8_lossy(&buffer[..count]);
                let first_line = request.lines().next().unwrap_or("");
                {
                    let mut log = observations.lock().unwrap();
                    let record = &mut log[index];
                    record["readCompletedAtUnixMs"] = json!(completed_at);
                    record["readBytes"] = json!(count);
                    record["readOutcome"] = json!(read_outcome);
                    record["firstHttpLine"] = json!(first_line);
                }
                event(
                    &events,
                    json!({"kind":"canary-read-completed", "label":label,
                    "connection":accepted, "acceptedAtUnixMs":accepted_at,
                    "readCompletedAtUnixMs":completed_at, "readBytes":count,
                    "readOutcome":read_outcome, "firstHttpLine":first_line}),
                );
                // No log mutex is held during either bounded socket operation.
                if stream
                    .set_write_timeout(Some(Duration::from_millis(150)))
                    .is_ok()
                {
                    let _ = stream.write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
            if accepted == CANARY_CONNECTION_LIMIT {
                event(
                    &events,
                    json!({"kind":"canary-observation-limit", "label":label,
                    "limit":CANARY_CONNECTION_LIMIT, "atUnixMs":unix_millis()}),
                );
            }
            true
        })
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
    type OfflineAssets = BTreeMap<String, (&'static str, String)>;
    unsafe fn memory_response(
        environment: &ICoreWebView2Environment,
        status: i32,
        reason: &str,
        headers: &str,
        body: &[u8],
    ) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
        let stream = CreateStreamOnHGlobal(Default::default(), true)?;
        let mut written = 0;
        stream
            .Write(body.as_ptr().cast(), body.len() as u32, Some(&mut written))
            .ok()?;
        if written as usize != body.len() {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x80004005u32 as i32,
            )));
        }
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let reason = pcw(reason);
        let headers = pcw(headers);
        environment.CreateWebResourceResponse(
            &stream,
            status,
            PCWSTR(reason.as_ptr()),
            PCWSTR(headers.as_ptr()),
        )
    }
    // Experimental per-WebView CDP emulation, never an OS/network security boundary.
    // https://github.com/ChromeDevTools/devtools-protocol/blob/master/json/browser_protocol.json
    // Calls are chained through completion handlers; unsupported/error means no navigation.
    unsafe fn configure_offline(
        core: ICoreWebView2,
        shell_url: String,
        events: Events,
        step: usize,
    ) -> windows::core::Result<()> {
        const COMMANDS: [(&str, &str); 2] = [
            (
                "Network.emulateNetworkConditionsByRule",
                r#"{"offline":true,"matchedNetworkConditions":[{"urlPattern":"","offline":true,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1}]}"#,
            ),
            (
                "Network.overrideNetworkState",
                r#"{"offline":true,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1,"connectionType":"none"}"#,
            ),
        ];
        if step == COMMANDS.len() {
            event(
                &events,
                json!({"kind":"offline-configured-before-navigation", "atUnixMs":unix_millis(), "limitation":"CDP emulation is experimental and is not OS isolation."}),
            );
            let url = pcw(&shell_url);
            return core.Navigate(PCWSTR(url.as_ptr()));
        }
        let (command, parameters) = COMMANDS[step];
        event(
            &events,
            json!({"kind":"offline-cdp-started", "command":command, "parameters":parameters, "atUnixMs":unix_millis()}),
        );
        let next_core = core.clone();
        let dispatch_events = events.clone();
        let callback = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
            move |result, response| {
                let parsed = serde_json::from_str::<Value>(&response).ok();
                let valid_response = parsed.as_ref().is_some_and(|value| {
                    value.is_object()
                        && value.get("error").is_none()
                        && (step != 0
                            || value["ruleIds"]
                                .as_array()
                                .is_some_and(|ids| ids.len() == 1 && ids[0].is_string()))
                });
                let completed = result.is_ok() && valid_response;
                event(
                    &events,
                    json!({"kind":"offline-cdp-completed", "command":command, "atUnixMs":unix_millis(), "success":completed, "response":response, "error":result.err().map(|error| error.to_string())}),
                );
                if !completed {
                    event(
                        &events,
                        json!({"kind":"installation-failed", "stage":"offline-cdp", "command":command}),
                    );
                    return Ok(());
                }
                if let Err(error) =
                    configure_offline(next_core, shell_url, events.clone(), step + 1)
                {
                    event(
                        &events,
                        json!({"kind":"installation-failed", "stage":"offline-cdp-or-navigation", "error":error.to_string()}),
                    );
                }
                Ok(())
            },
        ));
        let command_wide = pcw(command);
        let parameters_wide = pcw(parameters);
        let result = core.CallDevToolsProtocolMethod(
            PCWSTR(command_wide.as_ptr()),
            PCWSTR(parameters_wide.as_ptr()),
            &callback,
        );
        if let Err(error) = &result {
            event(
                &dispatch_events,
                json!({"kind":"offline-cdp-dispatch-error", "command":command, "atUnixMs":unix_millis(), "error":error.to_string()}),
            );
        }
        result
    }
    fn relative_assets_ready(observations: &[Value]) -> bool {
        observations
            .iter()
            .filter(|value| value["kind"] == "untrusted-page-report")
            .any(|value| {
                let Some(message) = value["json"]
                    .as_str()
                    .and_then(|text| serde_json::from_str::<Value>(text).ok())
                else {
                    return false;
                };
                let Some(report) = message["report"]
                    .as_str()
                    .and_then(|text| serde_json::from_str::<Value>(text).ok())
                else {
                    return false;
                };
                message["kind"] == "fixture-observations"
                    && report["kind"] == "relative-assets"
                    && report["module"] == true
                    && report["image"] == true
                    && report["css"] == "rgb(23, 71, 125)"
            })
    }
    pub fn run() {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let offline_preview = arguments.as_slice() == ["--offline-preview"];
        let layered_csp = match arguments.as_slice() {
            [] => false,
            [argument] if argument == "--layered-csp" || argument == "--offline-preview" => true,
            _ => {
                eprintln!("usage: visual_preview_probe [--layered-csp | --offline-preview]");
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
        let origin = format!("http://{}", server.local_addr().unwrap());
        let canary_observations: Events = Arc::default();
        let canary_stop = Arc::new(AtomicBool::new(false));
        let mut canary_endpoints = BTreeMap::new();
        let mut canary_threads = Vec::new();
        for label in CANARY_LABELS {
            let listener = TcpListener::bind("127.0.0.1:0").expect("canary loopback bind");
            canary_endpoints.insert(label, format!("http://{}", listener.local_addr().unwrap()));
            canary_threads.push(monitor_canary(
                label,
                listener,
                canary_stop.clone(),
                canary_observations.clone(),
                events.clone(),
            ));
        }
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
                        .replace("@@FETCH_CANARY@@", &canary_endpoints["fetch"])
                        .replace("@@IMAGE_CANARY@@", &canary_endpoints["image"])
                        .replace("@@FRAME_CANARY@@", &canary_endpoints["nested-frame"])
                        .replace("@@POPUP_CANARY@@", &canary_endpoints["popup"]),
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
        let redirect_target = format!("{}/redirect", canary_endpoints["redirect"]);
        let offline_assets: Option<OfflineAssets> = offline_preview.then(|| {
            fixtures
                .iter()
                .map(|(name, (mime, body))| {
                    (format!("{origin}{prefix}{name}"), (*mime, body.clone()))
                })
                .collect()
        });
        let offline_csp = csp_header.clone();
        let offline_redirect = redirect_target.clone();
        let server_events = events.clone();
        thread::spawn(move || {
            for incoming in server.incoming() {
                let Ok(mut stream) = incoming else { break };
                if offline_preview {
                    // Reserve the origin, but never serve a network fallback in Offline.
                    event(
                        &server_events,
                        json!({"kind":"offline-origin-tcp-accepted", "atUnixMs":unix_millis()}),
                    );
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..count]);
                let mut fields = request.lines().next().unwrap_or("").split_whitespace();
                let method = fields.next().unwrap_or("");
                let path = fields.next().unwrap_or("");
                if method == "GET" && path.strip_prefix(&prefix) == Some("redirect.json") {
                    let response = format!("HTTP/1.1 302 Found\r\n{csp_header}Location: {redirect_target}\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n");
                    let write_result = stream.write_all(response.as_bytes());
                    event(
                        &server_events,
                        json!({"kind":"redirect-response", "targetLabel":"redirect",
                        "atUnixMs":unix_millis(), "writeSucceeded":write_result.is_ok()}),
                    );
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
        let output = root.join("report.json");
        println!("V0 report: {}", output.display());
        // Also bound WebView/environment startup, before an AppHandle is available.
        let watchdog_output = output.clone();
        let watchdog_events = events.clone();
        let watchdog_canaries = canary_observations.clone();
        let watchdog_endpoints = canary_endpoints.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(20));
            let connections = watchdog_canaries.lock().unwrap().clone();
            let report = json!({"schema":2,"status":"watchdog-timeout",
                "canaryEndpoints":watchdog_endpoints, "canaryReached":!connections.is_empty(),
                "canaryConnections":connections, "canaryMonitoringStopped":false,
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
                            offline_assets,
                            offline_csp,
                            offline_redirect,
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
                    canary_stop.store(true, Ordering::Release);
                    let mut monitoring_complete = true;
                    for monitor in canary_threads {
                        monitoring_complete &= monitor.join().unwrap_or(false);
                    }
                    // Snapshot only after bounded reads from every accepted socket complete.
                    let connections = canary_observations.lock().unwrap().clone();
                    let observations = report_events.lock().unwrap().clone();
                    let canary_reached = !connections.is_empty();
                    let runtime_version = observations.iter()
                        .find(|value| value["kind"] == "webview2-runtime")
                        .map(|value| value["browserVersionString"].clone())
                        .unwrap_or(Value::Null);
                    let installed = observations
                        .iter()
                        .any(|value| value["kind"] == "native-installed-before-navigation")
                        && !observations.iter().any(|value| value["kind"] == "installation-failed");
                    let offline_configured = observations.iter().any(|value| value["kind"] == "offline-configured-before-navigation");
                    let offline_fixture_ready = relative_assets_ready(&observations)
                        && ["shell.html", "fixture.html", "fixture.css", "fixture.mjs", "fixture.svg", "redirect.json"].iter().all(|asset|
                            observations.iter().any(|value| value["kind"] == "offline-synthetic-response" && value["uri"] == format!("fixture/{asset}")));
                    let offline_valid = !offline_preview || (offline_configured && offline_fixture_ready
                        && !observations.iter().any(|value| value["kind"] == "offline-origin-tcp-accepted"));
                    let report = json!({"schema":2,"probe":"isolated-child-webview","csp":layered_csp,
                    "policy":if offline_preview {"Offline"} else if layered_csp {"Layered"} else {"NativeOnly"},
                    "offlineConfigured":offline_configured, "offlineFixtureReady":offline_fixture_ready, "offlineValid":offline_valid,
                    "offlineLimitation":"Experimental CDP emulation; no OS isolation or complete protocol coverage guarantee.",
                    "status":"observations-only-not-V0-acceptance", "canaryReached":canary_reached,
                    "canaryEndpoints":canary_endpoints, "canaryConnections":connections,
                    "canaryConnectionLimitPerListener":CANARY_CONNECTION_LIMIT,
                    "canaryMonitoringStopped":monitoring_complete, "runtimeVersion":runtime_version,
                    "causalLimit":"Endpoint labels identify the attempted attack, not the browser mechanism. No HTTP line does not establish preconnect or absence of transmitted bytes outside the bounded read.",
                    "productionIpcTested":false,"centralEmbedTested":false,
                    "profile":report_root.join("webview-profile"),"events":observations});
                    match fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()) {
                        Ok(()) => {
                            println!("{}", report);
                            handle.exit(if canary_reached || !installed || !monitoring_complete || !offline_valid { 2 } else { 0 });
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
        offline_assets: Option<OfflineAssets>,
        offline_csp: String,
        offline_redirect: String,
        shell_url: String,
        frame_url: String,
        origin: String,
        token: String,
        events: Events,
    ) -> windows::core::Result<()> {
        let core = core?;
        match read_string(|value| environment.BrowserVersionString(value)) {
            Ok(version) => event(
                &events,
                json!({"kind":"webview2-runtime", "browserVersionString":version}),
            ),
            Err(error) => event(
                &events,
                json!({"kind":"webview2-runtime-unavailable", "error":error.to_string()}),
            ),
        }
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
        let offline_preview = offline_assets.is_some();
        let resource_environment = environment.clone();
        let redirect_url = format!("{origin}/{token}/redirect.json");
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
                    if let Some(assets) = offline_assets.as_ref() {
                        let response = if uri == redirect_url {
                            let headers = format!("{offline_csp}Location: {offline_redirect}\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\n");
                            memory_response(&resource_environment, 302, "Found", &headers, b"")?
                        } else if let Some((mime, body)) = assets.get(&uri) {
                            let headers = format!("{offline_csp}Content-Type: {mime}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\n", body.len());
                            memory_response(&resource_environment, 200, "OK", &headers, body.as_bytes())?
                        } else {
                            // Default 403 remains; never clear it for a missing asset.
                            event(&request_events, json!({"kind":"offline-synthetic-missing", "uri":classify(&uri,&origin,&token)}));
                            return Ok(());
                        };
                        args.SetResponse(&response)?;
                        event(&request_events, json!({"kind":"offline-synthetic-response", "uri":classify(&uri,&origin,&token), "status":if uri == redirect_url {302} else {200}, "atUnixMs":unix_millis()}));
                        if uri == redirect_url {
                            event(&request_events, json!({"kind":"redirect-response", "synthetic":true, "targetLabel":"redirect", "atUnixMs":unix_millis(), "writeSucceeded":true}));
                        }
                    } else {
                        args.SetResponse(None::<&ICoreWebView2WebResourceResponse>)?;
                    }
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
        if offline_preview {
            configure_offline(core, shell_url, events, 0)?;
        } else {
            let url = pcw(&shell_url);
            core.Navigate(PCWSTR(url.as_ptr()))?;
        }
        Ok(())
    }
}

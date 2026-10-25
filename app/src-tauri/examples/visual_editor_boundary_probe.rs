//! Isolated, opt-in acceptance harness for the production read-only preview boundary.
//! Never starts AgentDock's Host, plugins, store, authentication or normal setup.
#[cfg(not(windows))]
fn main() {
    panic!("Windows WebView2 is required");
}
#[cfg(windows)]
fn main() {
    probe::run();
}
#[cfg(windows)]
mod probe {
    use agentdock_lib::visual_edit::preview::{guarded_invoke_handler, PreviewManager};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{
        fs::{self, OpenOptions},
        io::{Read, Write},
        net::TcpListener,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        thread,
        time::Duration,
    };
    use tauri::{
        webview::WebviewBuilder, LogicalPosition, LogicalSize, Manager, WebviewUrl, WindowBuilder,
    };
    use webview2_com::{Microsoft::Web::WebView2::Win32::*, WebMessageReceivedEventHandler};
    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::System::Com::{CoCreateGuid, CoTaskMemFree},
    };

    struct ProbeState {
        root: PathBuf,
        run: PathBuf,
        run_id: String,
        api: String,
        api_stop: Arc<AtomicBool>,
        api_worker: Mutex<Option<thread::JoinHandle<()>>>,
        api_requests: Arc<Mutex<Vec<String>>>,
        diagnostics: Arc<Mutex<Vec<Value>>>,
        sentinel: AtomicUsize,
        done: AtomicBool,
        profile: Mutex<Option<PathBuf>>,
    }
    fn create(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(bytes)
    }
    fn log(log: &Arc<Mutex<Vec<Value>>>, value: Value) {
        if let Ok(mut log) = log.lock() {
            if log.len() < 128 {
                log.push(value);
            }
        }
    }
    unsafe fn read_string(
        get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>,
    ) -> windows::core::Result<String> {
        let mut raw = PWSTR::null();
        get(&mut raw)?;
        if raw.is_null() {
            return Ok(String::new());
        }
        let value = raw.to_string();
        CoTaskMemFree(Some(raw.as_ptr().cast()));
        Ok(value?)
    }
    #[tauri::command]
    fn probe_privileged_sentinel(state: tauri::State<'_, Arc<ProbeState>>) -> usize {
        state.sentinel.fetch_add(1, Ordering::SeqCst) + 1
    }
    #[tauri::command]
    fn probe_fixture(state: tauri::State<'_, Arc<ProbeState>>) -> Value {
        json!({"root":state.root})
    }
    #[tauri::command]
    async fn probe_attach(
        app: tauri::AppHandle,
        state: tauri::State<'_, Arc<ProbeState>>,
        session_id: String,
    ) -> Result<(), String> {
        let view = app
            .get_webview(&format!("visual-preview-{session_id}"))
            .ok_or("preview missing")?;
        let diagnostics = state.diagnostics.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |platform| {
            let result = unsafe {
                platform.controller().CoreWebView2().and_then(|core| {
                    let mut token = 0;
                    core.add_WebMessageReceived(
                        &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                            if let Some(args) = args {
                                // Diagnostic observer only. Production validation/selection handler remains installed.
                                if let Ok(raw) = read_string(|p| args.TryGetWebMessageAsString(p)) {
                                    if raw.len() <= 16384 {
                                        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                                            if value.get("probe").is_some() {
                                                log(&diagnostics, value);
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(())
                        })),
                        &mut token,
                    )
                })
            }
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())?
    }
    #[tauri::command]
    async fn probe_step(
        app: tauri::AppHandle,
        state: tauri::State<'_, Arc<ProbeState>>,
        session_id: String,
        step: String,
    ) -> Result<(), String> {
        let view = app
            .get_webview(&format!("visual-preview-{session_id}"))
            .ok_or("preview missing")?;
        if matches!(step.as_str(), "shown" | "hidden" | "reshown") {
            let diagnostics = state.diagnostics.clone();
            let scale = view.window().scale_factor().map_err(|e| e.to_string())?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            view.with_webview(move |platform| {
                let result = unsafe {
                    let controller = platform.controller();
                    let mut visible = Default::default();
                    let mut rect = Default::default();
                    controller.IsVisible(&mut visible).and_then(|()|controller.Bounds(&mut rect)).map(|()| {
                        log(&diagnostics,json!({"probe":"lifecycle","stage":step,"visible":visible.as_bool(),
                            "physicalWidth":rect.right-rect.left,"physicalHeight":rect.bottom-rect.top,"scaleFactor":scale}));
                    })
                }.map_err(|e|e.to_string());
                let _=tx.send(result);
            }).map_err(|e|e.to_string())?;
            return rx.await.map_err(|e| e.to_string())?;
        }
        let script=match step.as_str(){
            "click-one"=>"document.getElementById('one').dispatchEvent(new MouseEvent('click',{bubbles:true,composed:true}));".to_string(),
            "ctrl-click-two"=>"document.getElementById('two').dispatchEvent(new MouseEvent('click',{bubbles:true,composed:true,ctrlKey:true}));".to_string(),
            "escape"=>"window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));".to_string(),
            "malicious"=>r#"window.chrome.webview.postMessage(JSON.stringify({selection:[{tag:'div',id:'injected',className:'',text:'injected',selector:'#injected',rect:{x:0,y:0,width:1,height:1}}],rootPath:'C:/',command:'probe_privileged_sentinel'}));window.chrome.webview.postMessage(JSON.stringify({probe:'malicious-sent'}));"#.to_string(),
            "resources"=>format!(r#"(async()=>{{
                const post=v=>window.chrome.webview.postMessage(JSON.stringify(v));
                const timeout=p=>Promise.race([p,new Promise((_,r)=>setTimeout(()=>r('timeout'),1800))]);
                const invokeSurface=typeof window.__TAURI_INTERNALS__?.invoke==='function'&&typeof window.__TAURI_INTERNALS__?.transformCallback==='function';
                const progress={{href:location.href,title:document.title,body:(document.body?.innerText||'').slice(0,200),groupPresent:!!document.getElementById('group'),module:window.probeModule===true,invokeSurface}};
                post({{probe:'resources-progress',stage:'before-api',...progress}});
                const rejected=async(cmd,args)=>{{if(!invokeSurface)return {{kind:'missing-invoke-surface'}};try{{await timeout(window.__TAURI_INTERNALS__.invoke(cmd,args));return {{kind:'resolved'}};}}catch(e){{return {{kind:'rejected',error:String(e)}};}}}};
                const status=async url=>{{try{{return (await timeout(fetch(url))).status;}}catch(e){{return String(e);}}}};
                const image=async src=>{{const i=new Image();const p=new Promise(r=>{{i.onload=()=>r(i.naturalWidth>0);i.onerror=()=>r(false);}});i.src=src;return timeout(p);}};
                try{{
                    const api=await timeout(fetch({api}).then(r=>r.json()));
                    const sentinelDenied=await rejected('probe_privileged_sentinel',{{}});
                    const pluginDenied=await rejected('plugin:event|listen',{{event:'editor-probe-event',target:{{kind:'Any'}},handler:invokeSurface?window.__TAURI_INTERNALS__.transformCallback(()=>{{}}):0}});
                    Object.assign(progress,{{api,sentinelDenied,pluginDenied}});
                    post({{probe:'resources-progress',stage:'ipc-attempts-completed',...progress}});
                    const missing=await status(new URL('../../outside.txt',location.href));
                    const secret=await status(new URL('../.env',location.href));
                    const prefixless=await status(location.origin+'/outside.txt');
                    const dataImage=await image('data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"/>'));
                    const blob=URL.createObjectURL(new Blob(['<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"/>'],{{type:'image/svg+xml'}}));
                    const blobImage=await image(blob);URL.revokeObjectURL(blob);
                    post({{probe:'resources',module:window.probeModule===true,css:getComputedStyle(document.getElementById('group')).color,
                        relativeImage:document.getElementById('relative').naturalWidth>0,rootImage:document.getElementById('root').naturalWidth>0,
                        api,invokeSurface,sentinelDenied,pluginDenied,missing,secret,prefixless,dataImage,blobImage}});
                }}catch(e){{post({{probe:'resources-error',error:String(e),...progress}});}}
            }})();"#,api=serde_json::to_string(&state.api).unwrap()),
            _=>return Err("unknown controlled test step".into())
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |platform| {
            let wide: Vec<u16> = script.encode_utf16().chain([0]).collect();
            let result = unsafe {
                platform.controller().CoreWebView2().and_then(|core| {
                    core.ExecuteScript(
                        PCWSTR(wide.as_ptr()),
                        None::<&ICoreWebView2ExecuteScriptCompletedHandler>,
                    )
                })
            }
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())?
    }
    fn explicit_denial(value: &Value, command: &str, guard: bool) -> bool {
        if value["kind"] != "rejected" {
            return false;
        }
        let Some(error) = value["error"].as_str() else {
            return false;
        };
        (guard && error == "application commands are unavailable to this webview")
            // Tauri 2.12.1 authority.rs reports absent app ACL manifests this way for remote custom commands.
            // The separate mainSentinel==1/sentinelEntries==1 gates prove the command exists and was not entered from preview.
            || (guard && error == format!("{command} not allowed. Plugin not found"))
            || (error.starts_with(command)
                && (error.contains(" not allowed on ")
                    || error.contains(" explicitly denied on ")
                    || error.contains(
                        " not allowed. command not allowed on any window/webview/URL context",
                    )))
    }
    #[test]
    fn rejection_classification_requires_explicit_authority_evidence() {
        let rejected = |error: &str| json!({"kind":"rejected","error":error});
        assert!(explicit_denial(
            &rejected("application commands are unavailable to this webview"),
            "probe_privileged_sentinel",
            true
        ));
        assert!(explicit_denial(
            &rejected("probe_privileged_sentinel not allowed. Plugin not found"),
            "probe_privileged_sentinel",
            true
        ));
        assert!(!explicit_denial(
            &rejected("other not allowed. Plugin not found"),
            "probe_privileged_sentinel",
            true
        ));
        for error in [
            "timeout",
            "ReferenceError: missing",
            "Plugin not found",
            "probe_privileged_sentinel not found",
            "probe_privileged_sentinel not allowed. Command not found",
        ] {
            assert!(
                !explicit_denial(&rejected(error), "probe_privileged_sentinel", true),
                "{error}"
            );
        }
        assert!(!explicit_denial(
            &json!({"kind":"resolved"}),
            "probe_privileged_sentinel",
            true
        ));
        assert!(!explicit_denial(
            &rejected("event.listen not allowed. Plugin not found"),
            "event.listen",
            false
        ));
    }
    fn stop_api(state: &ProbeState) {
        state.api_stop.store(true, Ordering::SeqCst);
        if let Some(worker) = state.api_worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
    fn write_report(
        state: &ProbeState,
        status: &str,
        observations: Value,
        passed: bool,
    ) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let hash = format!(
            "{:x}",
            Sha256::digest(fs::read(&exe).map_err(|e| e.to_string())?)
        );
        let report = json!({"schema":1,"probe":"production-preview-boundary","runId":state.run_id,"status":status,"passed":passed,
            "executable":exe,"executableSha256":hash,"fixtureRoot":state.root,"mainProfile":state.run.join("profile"),
            "previewProfile":*state.profile.lock().unwrap(),"sentinelEntries":state.sentinel.load(Ordering::SeqCst),
            "apiRequests":*state.api_requests.lock().unwrap(),"diagnostics":*state.diagnostics.lock().unwrap(),"observations":observations,
            "limitations":["Controlled DOM events through native ExecuteScript; no real mouse, DPI or full UI test", "No AI/Codex calls, editing, adoption, undo, normal Host or production data", "Nonexistent outside-file HTTP requests only; no claim of exercised native file-scheme denial", "Loopback API proves HTTP allowed; HTTPS/CDN/LAN and every plugin are not exhaustively tested"]});
        let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
        create(&state.run.join("report.json"), &bytes).map_err(|e| e.to_string())?;
        let output_hash = format!("{:x}", Sha256::digest(&bytes));
        println!(
            "report={} sha256={output_hash}",
            state.run.join("report.json").display()
        );
        Ok(())
    }
    #[tauri::command]
    fn probe_finish(
        app: tauri::AppHandle,
        state: tauri::State<'_, Arc<ProbeState>>,
        observations: Value,
    ) -> Result<(), String> {
        if state.done.swap(true, Ordering::SeqCst) {
            return Err("report already finalized".into());
        }
        stop_api(&state);
        let diagnostics = state.diagnostics.lock().unwrap().clone();
        let r = diagnostics.iter().find(|v| v["probe"] == "resources");
        let resources = r
            .map(|r| {
                r["module"] == true
                    && r["relativeImage"] == true
                    && r["rootImage"] == true
                    && r["css"] == "rgb(23, 71, 125)"
                    && r["api"]["allowed"] == true
                    && r["invokeSurface"] == true
                    && explicit_denial(&r["sentinelDenied"], "probe_privileged_sentinel", true)
                    && explicit_denial(&r["pluginDenied"], "event.listen", false)
                    && r["missing"] == 404
                    && r["secret"] == 404
                    && r["prefixless"] == 404
                    && r["dataImage"] == true
                    && r["blobImage"] == true
            })
            .unwrap_or(false);
        let selections = observations["selections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let ids: Vec<Vec<&str>> = selections
            .iter()
            .map(|e| {
                e["selection"]
                    .as_array()
                    .map(|s| s.iter().filter_map(|v| v["id"].as_str()).collect())
                    .unwrap_or_default()
            })
            .collect();
        let sequence = ids.windows(4).any(|w| {
            w == [
                vec!["one"],
                vec!["one", "two"],
                vec!["one", "group"],
                vec![],
            ]
        });
        let lifecycle =
            observations["lifecycle"] == json!(["opened", "shown", "hidden", "reshown", "closed"]);
        let native_lifecycle = [
            ("shown", true, 600.0),
            ("hidden", false, 580.0),
            ("reshown", true, 600.0),
        ]
        .iter()
        .all(|(stage, visible, width)| {
            diagnostics.iter().any(|v| {
                v["probe"] == "lifecycle"
                    && v["stage"] == *stage
                    && v["visible"] == *visible
                    && v["physicalWidth"]
                        .as_f64()
                        .zip(v["scaleFactor"].as_f64())
                        .map(|(actual, scale)| (actual / scale - width).abs() <= 1.0)
                        .unwrap_or(false)
            })
        });
        let no_child = app
            .webviews()
            .keys()
            .all(|k| !k.starts_with("visual-preview-"));
        let api_reached = state
            .api_requests
            .lock()
            .unwrap()
            .iter()
            .any(|p| p == &format!("GET /api/{} HTTP/1.1", state.run_id));
        let passed = resources
            && sequence
            && lifecycle
            && native_lifecycle
            && no_child
            && api_reached
            && state.sentinel.load(Ordering::SeqCst) == 1
            && observations["rootChange"] == json!([true, true])
            && observations["mainSentinel"] == 1
            && observations["beforeMalicious"] == observations["afterMalicious"]
            && diagnostics.iter().any(|d| d["probe"] == "malicious-sent")
            && observations["errors"] == json!([]);
        let written = write_report(
            &state,
            if passed { "passed" } else { "failed" },
            json!({"controller":observations,"checks":{"resources":resources,"selectionSequence":sequence,"lifecycle":lifecycle,"nativeVisibilityBounds":native_lifecycle,"childClosed":no_child,"apiReached":api_reached}}),
            passed,
        );
        app.exit(if passed && written.is_ok() { 0 } else { 2 });
        written
    }
    pub fn run() {
        let guid = format!("{:?}", unsafe { CoCreateGuid().expect("run GUID") })
            .replace(['{', '}', '-'], "")
            .to_lowercase();
        let run = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/visual-editor-probe")
            .join(format!("run-{guid}"));
        fs::create_dir_all(run.parent().unwrap()).expect("fixed evidence parent");
        fs::create_dir(&run).expect("unique run directory");
        let root = run.join("fixture");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("pages")).unwrap();
        fs::create_dir(root.join("assets")).unwrap();
        create(&root.join("pages/index.html"),br#"<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="../assets/style.css"><section id="group"><button id="one">One</button><button id="two">Two</button></section><img id="relative" src="../assets/image.svg"><img id="root" src="/assets/image.svg"><script type="module" src="../assets/module.mjs"></script>"#).unwrap();
        create(
            &root.join("assets/style.css"),
            b"#group{color:rgb(23,71,125)}button{margin:20px}",
        )
        .unwrap();
        create(&root.join("assets/module.mjs"), b"window.probeModule=true;").unwrap();
        create(&root.join("assets/image.svg"),br#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12"><rect width="12" height="12" fill="blue"/></svg>"#).unwrap();
        // Only a synthetic secret inside this new fixture. No real local secrets are accessed.
        create(
            &root.join(".env"),
            b"SYNTHETIC_EDITOR_PROBE_SECRET=not-real\n",
        )
        .unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("owned API listener");
        let api = format!("http://{}/api/{guid}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let api_stop = Arc::new(AtomicBool::new(false));
        let flag = api_stop.clone();
        let api_requests = Arc::new(Mutex::new(Vec::new()));
        let requests = api_requests.clone();
        let worker = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                        let mut bytes = [0u8; 2048];
                        if let Ok(n) = stream.read(&mut bytes) {
                            let first = String::from_utf8_lossy(&bytes[..n])
                                .lines()
                                .next()
                                .unwrap_or("")
                                .to_string();
                            let mut log = requests.lock().unwrap();
                            if log.len() < 32 {
                                log.push(first);
                            }
                            drop(log);
                            let body = b"{\"allowed\":true}";
                            let headers=format!("HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                            let _ = stream.write_all(headers.as_bytes());
                            let _ = stream.write_all(body);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        let state = Arc::new(ProbeState {
            root,
            run: run.clone(),
            run_id: guid,
            api,
            api_stop,
            api_worker: Mutex::new(Some(worker)),
            api_requests,
            diagnostics: Arc::new(Mutex::new(Vec::new())),
            sentinel: AtomicUsize::new(0),
            done: AtomicBool::new(false),
            profile: Mutex::new(None),
        });
        let watchdog_state = state.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(20));
            if watchdog_state
                .done
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                stop_api(&watchdog_state);
                let _ = write_report(&watchdog_state, "watchdog-timeout", json!({}), false);
                std::process::exit(5);
            }
        });
        let context =
            tauri::generate_context!("../verification/visual-editor/editor-probe/tauri.conf.json");
        if context.config().identifier != "com.agentdock.visual-editor-boundary-probe"
            || !context.config().app.with_global_tauri
        {
            state.done.store(true, Ordering::SeqCst);
            stop_api(&state);
            let _ = write_report(
                &state,
                "configuration-error",
                json!({"identifier":context.config().identifier,"withGlobalTauri":context.config().app.with_global_tauri}),
                false,
            );
            std::process::exit(2);
        }
        log(
            &state.diagnostics,
            json!({"probe":"startup-stage","stage":"configuration-validated","identifier":context.config().identifier}),
        );
        let app = tauri::Builder::default()
            .manage(PreviewManager::default())
            .manage(state.clone())
            .invoke_handler(guarded_invoke_handler(tauri::generate_handler![
                agentdock_lib::visual_edit::preview::visual_preview_open,
                agentdock_lib::visual_edit::preview::visual_preview_bounds,
                agentdock_lib::visual_edit::preview::visual_preview_action,
                agentdock_lib::visual_edit::preview::visual_preview_close,
                probe_fixture,
                probe_attach,
                probe_step,
                probe_finish,
                probe_privileged_sentinel
            ]))
            .setup(move |app| {
                let state = app.state::<Arc<ProbeState>>();
                *state.profile.lock().unwrap() = Some(
                    app.path()
                        .app_local_data_dir()?
                        .join("visual-preview/profile"),
                );
                let window = WindowBuilder::new(app, "main")
                    .title("Isolated preview boundary probe")
                    .visible(false)
                    .inner_size(660., 470.)
                    .build()?;
                log(&state.diagnostics,json!({"probe":"startup-stage","stage":"creating-main-webview"}));
                let main = window.add_child(
                    WebviewBuilder::new("main", WebviewUrl::App("editor-probe.html".into()))
                        .data_directory(state.run.join("profile")),
                    LogicalPosition::new(0., 0.),
                    LogicalSize::new(660., 470.),
                )?;
                let diagnostics = state.diagnostics.clone();
                main.with_webview(move |platform| {
                    let observed = unsafe {
                        platform.controller().CoreWebView2().and_then(|core| {
                            let mut token = 0;
                            let messages=diagnostics.clone();
                            core.add_WebMessageReceived(&WebMessageReceivedEventHandler::create(Box::new(move |_,args| {
                                if let Some(args)=args { if let Ok(raw)=read_string(|p|args.TryGetWebMessageAsString(p)) {
                                    if raw.len()<=16384 { if let Ok(value)=serde_json::from_str::<Value>(&raw) {
                                        if value.get("probe").is_some() {log(&messages,value);}
                                    }}
                                }}
                                Ok(())
                            })),&mut token)
                        })
                    };
                    log(&diagnostics,json!({"probe":"startup-stage","stage":"main-observer-installed","success":observed.is_ok(),"error":observed.err().map(|e|e.to_string())}));
                })?;
                log(&state.diagnostics,json!({"probe":"startup-stage","stage":"main-webview-created"}));
                Ok(())
            })
            .build(context);
        let app = match app {
            Ok(app) => app,
            Err(error) => {
                if !state.done.swap(true, Ordering::SeqCst) {
                    stop_api(&state);
                    let _ = write_report(
                        &state,
                        "startup-error",
                        json!({"error":error.to_string()}),
                        false,
                    );
                }
                std::process::exit(2);
            }
        };
        app.run(|_, _| {});
    }
}

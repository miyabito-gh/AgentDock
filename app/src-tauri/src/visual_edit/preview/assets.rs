//! Read-only, bounded preview assets. No request resolves a filesystem path.
use super::super::{ProjectRoot, RelativePath, MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
pub(super) struct Asset {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}
pub(super) type Assets = BTreeMap<String, Asset>;
fn mime(path: &str) -> Option<&'static str> {
    Some(
        match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
            "html" | "htm" => "text/html; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "js" | "mjs" => "text/javascript; charset=utf-8",
            "json" | "map" | "webmanifest" => "application/json",
            "txt" => "text/plain; charset=utf-8",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            "ico" => "image/x-icon",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "ttf" => "font/ttf",
            "otf" => "font/otf",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "ogg" => "audio/ogg",
            "wasm" => "application/wasm",
            _ => return None,
        },
    )
}
pub(super) fn capture(root: &ProjectRoot) -> Result<Assets, String> {
    fn walk(
        root: &ProjectRoot,
        dir: &Path,
        prefix: &str,
        assets: &mut Assets,
        total: &mut u64,
        visited: &mut usize,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            *visited += 1;
            if *visited > 4096 {
                return Err("preview directory entry limit (4096) exceeded".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let relative = match RelativePath::parse(&path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    continue;
                }
            }
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                // Keep directory handles pinned while enumerating, including ancestors.
                let _pins = super::super::scope::pin_directories(&entry.path())
                    .map_err(|e| e.to_string())?;
                walk(root, &entry.path(), &path, assets, total, visited)?;
            } else if meta.is_file() {
                let Some(mime) = mime(&path) else { continue };
                if assets.len() >= MAX_FILES {
                    return Err("preview asset count limit exceeded".into());
                }
                let held = root
                    .open_relative(&relative)
                    .map_err(|e| e.to_string())?
                    .ok_or("preview asset disappeared")?;
                let before = held.file.metadata().map_err(|e| e.to_string())?;
                if before.len() > MAX_FILE_BYTES {
                    continue;
                }
                let mut bytes = Vec::new();
                (&held.file)
                    .take(MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                let after = held.file.metadata().map_err(|e| e.to_string())?;
                if bytes.len() as u64 > MAX_FILE_BYTES
                    || bytes.len() as u64 != before.len()
                    || before.len() != after.len()
                    || before.modified().ok() != after.modified().ok()
                {
                    return Err("preview asset changed during capture".into());
                }
                *total += bytes.len() as u64;
                if *total > MAX_TOTAL_BYTES {
                    return Err("preview total asset bytes exceeded".into());
                }
                assets.insert(path, Asset { mime, bytes });
            }
        }
        Ok(())
    }
    let mut assets = Assets::new();
    walk(root, root.path(), "", &mut assets, &mut 0, &mut 0)?;
    Ok(assets)
}
fn decode_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(pair).ok()?;
            let b = u8::from_str_radix(hex, 16).ok()?;
            if matches!(b, b'/' | b'\\' | b'%') {
                return None;
            }
            decoded.push(b);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}
fn request_path<'a>(target: &str, token: &str, assets: &'a Assets) -> Option<&'a Asset> {
    let target = target.split('?').next()?;
    let suffix = target.strip_prefix(&format!("/{token}/"))?;
    let path = decode_path(suffix)?;
    RelativePath::parse(&path).ok()?;
    assets.get(&path)
}
pub(super) struct Server {
    pub origin: String,
    pub token: String,
    pub assets: Arc<RwLock<Assets>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn start(assets: Assets, token: String) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(RwLock::new(assets));
        let (flag, data, key, host) = (
            stop.clone(),
            shared.clone(),
            token.clone(),
            addr.to_string(),
        );
        let worker = thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &host, &key, &data),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(15))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            origin: format!("http://{addr}"),
            token,
            assets: shared,
            stop,
            worker: Some(worker),
        })
    }
    pub fn url(&self, html: &str) -> String {
        let mut url = tauri::Url::parse(&format!("{}/{}/", self.origin, self.token)).unwrap();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .extend(html.split('/'));
        url.to_string()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn serve(mut stream: TcpStream, host: &str, token: &str, assets: &RwLock<Assets>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    let deadline = std::time::Instant::now() + Duration::from_millis(350);
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    while std::time::Instant::now() < deadline
        && request.len() < 16384
        && !request.windows(4).any(|w| w == b"\r\n\r\n")
    {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&chunk[..n]),
        }
    }
    let Ok(text) = std::str::from_utf8(&request) else {
        return;
    };
    let mut lines = text.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    let headers: Vec<_> = lines
        .take_while(|s| !s.is_empty())
        .filter_map(|s| s.split_once(':'))
        .collect();
    let host_ok = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .count()
        == 1
        && headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("host") && v.trim() == host);
    let valid = parts.len() == 3
        && matches!(parts[0], "GET" | "HEAD")
        && parts[2] == "HTTP/1.1"
        && host_ok
        && request.windows(4).any(|w| w == b"\r\n\r\n");
    let data = match assets.read() {
        Ok(a) => a,
        Err(_) => return,
    };
    let asset = if valid {
        request_path(parts[1], token, &data)
    } else {
        None
    };
    let (status, mime, body) = match asset {
        Some(a) => ("200 OK", a.mime, a.bytes.as_slice()),
        None => ("404 Not Found", "text/plain", b"Not found".as_slice()),
    };
    // No CORS grant: an unrelated origin cannot read the local snapshot.
    let header=format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",body.len());
    // A slow reader must not keep session close / server Drop waiting indefinitely.
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(350);
    let body = if parts.first() == Some(&"HEAD") {
        &[][..]
    } else {
        body
    };
    'send: for mut bytes in [header.as_bytes(), body] {
        while !bytes.is_empty() && std::time::Instant::now() < deadline {
            match stream.write(bytes) {
                Ok(0) => break 'send,
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => break 'send,
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_asset_urls_resolve_nested_spaces_and_unicode() {
        let mut assets = Assets::new();
        for name in ["pages/example page.html", "pages/日本語.html"] {
            assets.insert(
                name.into(),
                Asset {
                    mime: "text/html",
                    bytes: name.as_bytes().to_vec(),
                },
            );
        }
        let server = Server {
            origin: "http://127.0.0.1:12345".into(),
            token: "test-key".into(),
            assets: Arc::new(RwLock::new(assets)),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        for name in ["pages/example page.html", "pages/日本語.html"] {
            let url = tauri::Url::parse(&server.url(name)).unwrap();
            assert!(
                !url.path().contains("//"),
                "generated URL has an empty path segment: {url}"
            );
            let assets = server.assets.read().unwrap();
            let asset = request_path(url.path(), &server.token, &assets)
                .expect("generated URL must resolve a captured asset");
            assert_eq!(asset.bytes, name.as_bytes());
        }
    }
    #[test]
    fn server_scope_rejects_encoded_traversal_secret_and_foreign_target() {
        let mut a = Assets::new();
        a.insert(
            "nested/a.js".into(),
            Asset {
                mime: "text/javascript",
                bytes: vec![1],
            },
        );
        assert!(request_path("/t/nested/a.js?v=2", "t", &a).is_some());
        for p in [
            "/t/../nested/a.js",
            "/t/%2e%2e/nested/a.js",
            "/t/nested%2fa.js",
            "/t/%252e%252e/a.js",
            "/t/.env",
            "http://127.0.0.1/t/nested/a.js",
            "/wrong/nested/a.js",
        ] {
            assert!(request_path(p, "t", &a).is_none(), "{p}");
        }
    }
    #[test]
    fn root_snapshot_excludes_secrets_and_server_never_reads_new_files() {
        let dir = std::env::temp_dir().join(format!(
            "agentdock-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(dir.join("index.html"), b"<h1>original</h1>").unwrap();
        std::fs::write(dir.join(".env"), b"secret").unwrap();
        std::fs::write(dir.join(".codex/visible.json"), b"secret").unwrap();
        std::fs::write(dir.join("launch.ps1"), b"secret").unwrap();
        let oversized = std::fs::File::create(dir.join("unused.mp4")).unwrap();
        oversized.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(oversized);
        let root = ProjectRoot::open(&dir).unwrap();
        let snapshot = capture(&root).unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot.contains_key("index.html"));
        let server = Server::start(snapshot, "test-key".into()).unwrap();
        std::fs::write(dir.join("index.html"), b"replaced").unwrap();
        std::fs::write(dir.join("late.js"), b"late").unwrap();
        let host = server.origin.strip_prefix("http://").unwrap();
        let request = |method: &str, target: &str, authority: &str| {
            let mut c = TcpStream::connect(host).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            write!(c, "{method} {target} HTTP/1.1\r\nHost: {authority}\r\n\r\n").unwrap();
            let mut response = String::new();
            c.read_to_string(&mut response).unwrap();
            response
        };
        let response = request("GET", "/test-key/index.html", host);
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with("<h1>original</h1>"));
        for (method, target, authority) in [
            ("GET", "/test-key/late.js", host),
            ("POST", "/test-key/index.html", host),
            ("GET", "/test-key/.env", host),
            ("GET", "/test-key/index.html", "attacker.invalid"),
        ] {
            assert!(request(method, target, authority).starts_with("HTTP/1.1 404"));
        }
        drop(server);
        drop(root);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

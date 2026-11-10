//! ペットの読み取り（DESIGN_PET「データ互換」）。`${CODEX_HOME:-~/.codex}/pets/<フォルダ>/pet.json` と画像を読むだけで、書かない・コピーしない。
//!
//! - 受け入れ条件は Codex V2: `spriteVersionNumber == 2`、PNG/WebP、1536×2288。条件外は理由つきの「非対応」で返し、黙って捨てない。
//! - 識別はフォルダ名（`id` は無いことがある）。フォルダ名・画像パスは pets 配下から出ないことを確認する。

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SHEET_WIDTH: u32 = 1536;
pub const SHEET_HEIGHT: u32 = 2288;
const MANIFEST_MAX_BYTES: u64 = 64 * 1024;
const SPRITE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// 一覧の1件。`supported=false` のとき `unsupported_reason` に理由を入れる（Codex固有の型は含めない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PetEntry {
    /// `pets` 直下のフォルダ名（識別子）。
    pub folder: String,
    pub display_name: String,
    pub description: Option<String>,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    /// 画像のMIME（`image/png`・`image/webp`）。読めなかったとき None。
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub spritesheet_path: String,
}

/// `pet.json` を検証する。V2以外・必須項目なしは理由を返す。
pub fn parse_manifest(json: &str) -> Result<Manifest, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| "pet.json を読めません（JSONの形式が不正）".to_string())?;
    let obj = v.as_object().ok_or_else(|| "pet.json を読めません（オブジェクトではありません）".to_string())?;
    match obj.get("spriteVersionNumber").and_then(|x| x.as_u64()) {
        Some(2) => {}
        Some(n) => return Err(format!("スプライト版 {n} は非対応です（版2のみ）")),
        None => return Err("スプライト版（spriteVersionNumber）がありません。版2のみ対応です".to_string()),
    }
    let path = obj.get("spritesheetPath").and_then(|x| x.as_str()).map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| "画像の指定（spritesheetPath）がありません".to_string())?;
    let text = |k: &str| obj.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(Manifest { display_name: text("displayName"), description: text("description"), spritesheet_path: path.to_string() })
}

/// 画像の先頭バイトから（MIME, 幅, 高さ）を読む。PNG と WebP（VP8 / VP8L / VP8X）だけ。
pub fn sheet_info(head: &[u8]) -> Result<(&'static str, u32, u32), String> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if head.starts_with(PNG) {
        if head.len() < 24 || &head[12..16] != b"IHDR" {
            return Err("PNG のヘッダーを読めません".into());
        }
        let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
        let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
        return Ok(("image/png", w, h));
    }
    if head.len() >= 16 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        let d = &head[20..];
        let bad = || "WebP のヘッダーを読めません".to_string();
        let (w, h) = match &head[12..16] {
            b"VP8X" => {
                if d.len() < 10 {
                    return Err(bad());
                }
                let w = 1 + (d[4] as u32 | (d[5] as u32) << 8 | (d[6] as u32) << 16);
                let h = 1 + (d[7] as u32 | (d[8] as u32) << 8 | (d[9] as u32) << 16);
                (w, h)
            }
            b"VP8L" => {
                if d.len() < 5 || d[0] != 0x2f {
                    return Err(bad());
                }
                let bits = u32::from_le_bytes([d[1], d[2], d[3], d[4]]);
                (1 + (bits & 0x3fff), 1 + ((bits >> 14) & 0x3fff))
            }
            b"VP8 " => {
                if d.len() < 10 || d[3..6] != [0x9d, 0x01, 0x2a] {
                    return Err(bad());
                }
                (u16::from_le_bytes([d[6], d[7]]) as u32 & 0x3fff, u16::from_le_bytes([d[8], d[9]]) as u32 & 0x3fff)
            }
            _ => return Err(bad()),
        };
        return Ok(("image/webp", w, h));
    }
    Err("画像は PNG か WebP のみ対応です".into())
}

/// 画像の条件（V2: 1536×2288）。満たさなければ理由。
pub fn check_sheet(head: &[u8]) -> Result<&'static str, String> {
    let (mime, w, h) = sheet_info(head)?;
    if (w, h) != (SHEET_WIDTH, SHEET_HEIGHT) {
        return Err(format!("画像の大きさが {w}×{h} です（{SHEET_WIDTH}×{SHEET_HEIGHT} のみ対応）"));
    }
    Ok(mime)
}

/// `pets` のフォルダ。`CODEX_HOME` があればそれ、なければ `%USERPROFILE%\.codex`。読むだけ。
pub fn pets_root() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| {
        std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".codex"))
    })?;
    Some(home.join("pets"))
}

fn valid_folder_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':']) && !name.contains('\0')
}

/// 画像パス（pet.json 内）が、ペットのフォルダ配下の相対パスか。絶対パス・`..` は不可。
fn relative_inside(rel: &str) -> bool {
    let p = Path::new(rel);
    !p.is_absolute() && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("読めません（{e}）"))?;
    let mut buf = Vec::new();
    f.take(limit + 1).read_to_end(&mut buf).map_err(|e| format!("読めません（{e}）"))?;
    if buf.len() as u64 > limit {
        return Err("ファイルが大きすぎます".into());
    }
    Ok(buf)
}

/// 画像ファイルの場所。pets 配下（リンクで外へ出るものを含む）から出ていれば拒否する。
fn sprite_path(root: &Path, dir: &Path, rel: &str) -> Result<PathBuf, String> {
    if !relative_inside(rel) {
        return Err("画像の指定がフォルダの外を指しています".into());
    }
    let path = dir.join(rel);
    let canon = path.canonicalize().map_err(|_| "画像ファイルが見つかりません".to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !canon.starts_with(&root) {
        return Err("画像の指定がペットのフォルダの外を指しています".into());
    }
    Ok(canon)
}

fn read_head(path: &Path) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("画像を読めません（{e}）"))?;
    let mut buf = Vec::new();
    f.take(64).read_to_end(&mut buf).map_err(|e| format!("画像を読めません（{e}）"))?;
    Ok(buf)
}

/// 1フォルダを検査する。Ok は（マニフェスト, MIME, 画像の場所）。
fn inspect(root: &Path, folder: &str) -> Result<(Manifest, &'static str, PathBuf), (Option<Manifest>, String)> {
    let dir = root.join(folder);
    let json = read_limited(&dir.join("pet.json"), MANIFEST_MAX_BYTES).map_err(|e| (None, format!("pet.json: {e}")))?;
    let json = String::from_utf8(json).map_err(|_| (None, "pet.json を読めません（文字コードが不正）".to_string()))?;
    let m = parse_manifest(&json).map_err(|e| (None, e))?;
    let fail = |m: &Manifest, e: String| (Some(m.clone()), e);
    let path = sprite_path(root, &dir, &m.spritesheet_path).map_err(|e| fail(&m, e))?;
    let head = read_head(&path).map_err(|e| fail(&m, e))?;
    let mime = check_sheet(&head).map_err(|e| fail(&m, e))?;
    Ok((m, mime, path))
}

/// `pets` 直下のフォルダを列挙する（フォルダ名順）。`pets` がなければ空。非対応も理由つきで含める。
pub fn list_pets(root: &Path) -> Vec<PetEntry> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut names: Vec<String> = rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| valid_folder_name(n)).collect();
    names.sort();
    names
        .into_iter()
        .map(|folder| match inspect(root, &folder) {
            Ok((m, mime, _)) => PetEntry { display_name: m.display_name.unwrap_or_else(|| folder.clone()), description: m.description, supported: true, unsupported_reason: None, mime: Some(mime.to_string()), folder },
            Err((m, reason)) => PetEntry {
                display_name: m.as_ref().and_then(|m| m.display_name.clone()).unwrap_or_else(|| folder.clone()),
                description: m.and_then(|m| m.description),
                supported: false,
                unsupported_reason: Some(reason),
                mime: None,
                folder,
            },
        })
        .collect()
}

/// 選んだペットの画像（バイト列とMIME）。再検査して、条件外なら理由を返す。
pub fn load_sprite(root: &Path, folder: &str) -> Result<(Vec<u8>, &'static str), String> {
    if !valid_folder_name(folder) {
        return Err("ペットのフォルダ名が不正です".into());
    }
    let (_, mime, path) = inspect(root, folder).map_err(|(_, e)| e)?;
    Ok((read_limited(&path, SPRITE_MAX_BYTES)?, mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v
    }
    fn webp_lossless(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0".to_vec();
        v.push(0x2f);
        v.extend(((w - 1) | ((h - 1) << 14)).to_le_bytes());
        v
    }
    fn webp_ext(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        v.extend(&(w - 1).to_le_bytes()[..3]);
        v.extend(&(h - 1).to_le_bytes()[..3]);
        v
    }
    fn webp_lossy(w: u16, h: u16) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9d\x01\x2a".to_vec();
        v.extend(w.to_le_bytes());
        v.extend(h.to_le_bytes());
        v
    }

    #[test]
    fn manifest_requires_v2_and_a_sheet_path() {
        let m = parse_manifest(r#"{"displayName":"Neko","spriteVersionNumber":2,"spritesheetPath":"s.webp"}"#).unwrap();
        assert_eq!(m.display_name.as_deref(), Some("Neko"));
        assert_eq!(m.spritesheet_path, "s.webp");
        assert!(parse_manifest(r#"{"spriteVersionNumber":1,"spritesheetPath":"s.png"}"#).unwrap_err().contains("版 1"));
        assert!(parse_manifest(r#"{"spritesheetPath":"s.png"}"#).unwrap_err().contains("spriteVersionNumber"));
        assert!(parse_manifest(r#"{"spriteVersionNumber":2}"#).unwrap_err().contains("spritesheetPath"));
        assert!(parse_manifest("not json").is_err());
    }

    #[test]
    fn sheet_dimensions_are_read_for_png_and_all_webp_kinds() {
        assert_eq!(check_sheet(&png(1536, 2288)), Ok("image/png"));
        assert_eq!(check_sheet(&webp_lossless(1536, 2288)), Ok("image/webp"));
        assert_eq!(check_sheet(&webp_ext(1536, 2288)), Ok("image/webp"));
        assert_eq!(check_sheet(&webp_lossy(1536, 2288)), Ok("image/webp"));
        assert!(check_sheet(&png(1536, 1872)).unwrap_err().contains("1536×1872"));
        assert!(check_sheet(&webp_lossless(1536, 1872)).is_err());
        assert!(check_sheet(b"GIF89a....").unwrap_err().contains("PNG か WebP"));
    }

    #[test]
    fn sprite_path_must_stay_inside_the_pet_folder() {
        assert!(relative_inside("spritesheet.webp"));
        assert!(relative_inside("./a/b.png"));
        assert!(!relative_inside("../x.png"));
        assert!(!relative_inside("a/../../x.png"));
        assert!(!relative_inside("C:\\x.png"));
        assert!(!valid_folder_name("..") && !valid_folder_name("a/b") && !valid_folder_name("a\\b") && !valid_folder_name(""));
    }

    #[test]
    fn listing_reports_unsupported_pets_with_reasons() {
        let root = std::env::temp_dir().join(format!("agentdock-pets-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ok = root.join("good");
        let old = root.join("old");
        std::fs::create_dir_all(&ok).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(ok.join("pet.json"), r#"{"displayName":"Good","spriteVersionNumber":2,"spritesheetPath":"s.png"}"#).unwrap();
        std::fs::write(ok.join("s.png"), png(1536, 2288)).unwrap();
        std::fs::write(old.join("pet.json"), r#"{"spriteVersionNumber":2,"spritesheetPath":"s.png"}"#).unwrap();
        std::fs::write(old.join("s.png"), png(1536, 1872)).unwrap();
        let list = list_pets(&root);
        assert_eq!(list.len(), 2);
        let good = list.iter().find(|e| e.folder == "good").unwrap();
        assert!(good.supported && good.mime.as_deref() == Some("image/png") && good.display_name == "Good");
        let old = list.iter().find(|e| e.folder == "old").unwrap();
        assert!(!old.supported && old.unsupported_reason.as_deref().unwrap().contains("1536×1872") && old.display_name == "old");
        assert!(load_sprite(&root, "good").is_ok());
        assert!(load_sprite(&root, "old").is_err());
        assert!(load_sprite(&root, "..").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

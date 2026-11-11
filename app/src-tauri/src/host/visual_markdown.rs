//! Markdown 文書の読み書き（Visual Editor の編集欄用）。文字コード・改行の判定と復元は `visual_edit::markdown`、
//! 書き込みは `direct::safe_write`（リンク・外部への脱出を拒否、一時ファイル＋置換）。自分の保存はターンとして記録しない。
use super::{err, Host};
use crate::backend::ipc::{IpcError, IpcErrorCode};
use crate::visual_edit::{self as ve, direct::{self, DirectError}, markdown::{self as md, Eol}, ProjectRoot, RelativePath};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MdTargetArgs { pub root_path:String, pub path:String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MdSaveArgs { pub root_path:String, pub path:String, pub text:String, pub bom:bool, pub eol:String, #[serde(default)] pub base_stamp:Option<String>, #[serde(default)] pub force:bool }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MdImageArgs { pub root_path:String, pub path:String, pub href:String }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdDoc { pub text:String, pub bom:bool, pub eol:Eol, pub mixed:bool, pub stamp:String, pub bytes:u64 }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdSaved { pub stamp:String, pub bytes:u64 }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MdImage { pub mime:&'static str, pub data:String }

fn blocked(m:impl Into<String>)->IpcError { err(IpcErrorCode::Blocked,m) }
fn derr(e:DirectError)->IpcError { blocked(e.to_string()) }
fn internal_err(e:impl std::fmt::Display)->IpcError { blocked(format!("内部エラーで処理を完了できませんでした（{e}）。もう一度お試しください。")) }
const MISSING:&str="Markdownファイルが見つかりません（移動または削除された可能性があります）。";

struct Doc { root:ProjectRoot, rel:RelativePath }
fn open_doc(root_path:&str,path:&str)->Result<Doc,IpcError> {
    let root=ProjectRoot::open(Path::new(root_path)).map_err(|e|blocked(format!("フォルダを開けません。場所を確認してください。（{}）",direct::jp_visual(&e))))?;
    let rel=ve::preview::relative_entry(root.path(),path).map_err(|e|blocked(format!("Markdownファイルのパスが正しくありません。フォルダの中のファイルを選び直してください。（{e}）")))?;
    let ext=rel.as_str().rsplit('.').next().unwrap_or("");
    if !(ext.eq_ignore_ascii_case("md")||ext.eq_ignore_ascii_case("markdown")) { return Err(blocked("Markdown（.md / .markdown）のファイルだけを編集できます。")); }
    Ok(Doc{root,rel})
}
/// ファイルの変化を見る印（サイズと更新時刻）。無ければ None。
fn file_stamp(doc:&Doc)->Option<String> {
    let meta=std::fs::symlink_metadata(doc.rel.join(doc.root.path())).ok()?;
    if !meta.is_file() { return None; }
    let modified=meta.modified().ok().and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_nanos()).unwrap_or(0);
    Some(format!("{}:{modified}",meta.len()))
}
impl Host {
    /// 文書を読む。壊れた UTF-8 は開かない。印は読む前に取る（読む間に変わっても、次の確認で検出される）。
    pub async fn visual_markdown_read(self:&Arc<Self>,args:MdTargetArgs)->Result<MdDoc,IpcError> {
        tokio::task::spawn_blocking(move||{
            let d=open_doc(&args.root_path,&args.path)?;
            let stamp=file_stamp(&d).ok_or_else(||blocked(MISSING))?;
            let bytes=direct::read_current(&d.root,&d.rel).map_err(derr)?.ok_or_else(||blocked(MISSING))?;
            let dec=md::decode(&bytes).map_err(blocked)?;
            Ok(MdDoc{text:dec.text,bom:dec.bom,eol:dec.eol,mixed:dec.mixed,stamp,bytes:bytes.len() as u64})
        }).await.map_err(internal_err)?
    }
    pub async fn visual_markdown_stamp(self:&Arc<Self>,args:MdTargetArgs)->Result<String,IpcError> {
        tokio::task::spawn_blocking(move||{
            let d=open_doc(&args.root_path,&args.path)?;
            file_stamp(&d).ok_or_else(||blocked(MISSING))
        }).await.map_err(internal_err)?
    }
    /// 保存。読んだときの印と違えば `conflict:` で拒否（`force` のときだけ上書き）。ターンの記録とは直列化する。
    pub async fn visual_markdown_save(self:&Arc<Self>,args:MdSaveArgs)->Result<MdSaved,IpcError> {
        let eol=Eol::parse(&args.eol).ok_or_else(||blocked("改行の種類を確認できません。"))?;
        if args.text.len()>md::MAX_DOC_BYTES { return Err(blocked("本文が大きすぎるため保存できません（4MBまで）。")); }
        let _guard=self.visual_rt.lock.lock().await;
        tokio::task::spawn_blocking(move||{
            let d=open_doc(&args.root_path,&args.path)?;
            if !args.force && file_stamp(&d)!=args.base_stamp { return Err(blocked(format!("conflict:\n{}",d.rel.as_str()))); }
            let bytes=md::encode(&args.text,args.bom,eol);
            direct::safe_write(&d.root,&d.rel,Some(&bytes)).map_err(derr)?;
            let stamp=file_stamp(&d).ok_or_else(||blocked("保存後のファイルを確認できませんでした。"))?;
            Ok(MdSaved{stamp,bytes:bytes.len() as u64})
        }).await.map_err(internal_err)?
    }
    /// 文書から相対参照された画像（フォルダの中のものだけ）。プレビューへ base64 で渡す。外部URLは扱わない。
    pub async fn visual_markdown_image(self:&Arc<Self>,args:MdImageArgs)->Result<MdImage,IpcError> {
        tokio::task::spawn_blocking(move||{
            let d=open_doc(&args.root_path,&args.path)?;
            let rel=md::resolve_ref(d.rel.as_str(),&args.href).ok_or_else(||blocked("この画像はフォルダの外または外部にあるため表示しません。"))?;
            let mime=md::image_mime(&rel).ok_or_else(||blocked("画像ではないファイルです。"))?;
            let parsed=RelativePath::parse(&rel).map_err(|_|blocked("画像のパスが正しくありません。"))?;
            let bytes=direct::read_current(&d.root,&parsed).map_err(derr)?.ok_or_else(||blocked("画像が見つかりません。"))?;
            if bytes.len()>md::MAX_IMAGE_BYTES { return Err(blocked("画像が大きすぎるため表示しません（5MBまで）。")); }
            Ok(MdImage{mime,data:md::base64(&bytes)})
        }).await.map_err(internal_err)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_refuses_a_changed_file_unless_forced_and_keeps_bom_and_crlf() {
        let base=std::env::temp_dir().join(format!("agentdock-mdsave-{}",std::process::id()));
        let _=std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.md"),b"\xEF\xBB\xBF# t\r\nx\r\n").unwrap();
        let d=open_doc(base.to_str().unwrap(),"a.md").unwrap();
        let stamp=file_stamp(&d);
        let bytes=direct::read_current(&d.root,&d.rel).unwrap().unwrap();
        let dec=md::decode(&bytes).unwrap();
        assert!(dec.bom && dec.eol==Eol::Crlf);
        // 外部で書き換わった（印が違う）
        std::fs::write(base.join("a.md"),b"changed elsewhere, longer").unwrap();
        assert_ne!(file_stamp(&d),stamp);
        let out=md::encode("# t\nnew\n",dec.bom,dec.eol);
        direct::safe_write(&d.root,&d.rel,Some(&out)).unwrap();
        let got=std::fs::read(base.join("a.md")).unwrap();
        drop(d);
        let _=std::fs::remove_dir_all(&base);
        assert_eq!(got,b"\xEF\xBB\xBF# t\r\nnew\r\n");
    }
    #[test]
    fn non_markdown_paths_are_refused() {
        let base=std::env::temp_dir().join(format!("agentdock-mdext-{}",std::process::id()));
        let _=std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.txt"),"x").unwrap();
        let r=open_doc(base.to_str().unwrap(),"a.txt");
        let _=std::fs::remove_dir_all(&base);
        assert!(r.is_err());
    }
}

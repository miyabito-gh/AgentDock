//! Visual Editor の Host 側の共有部品: 編集対象ファイルの探索（`discover`）。
use crate::visual_edit::{self as ve, FileAccess, FileScope, ProjectRoot, RelativePath, SelectedFile};
use std::fs;
#[derive(Default)]
pub(super) struct VisualRuntime { pub lock: tokio::sync::Mutex<()> }
fn known_access(ext:&str)->Option<FileAccess> {
    match ext { "html"|"htm"|"css"|"js"|"mjs"|"svg"=>Some(FileAccess::Edit), "png"|"jpg"|"jpeg"|"gif"|"webp"|"avif"|"ico"|"woff"|"woff2"|"ttf"|"otf"|"json"=>Some(FileAccess::ReadOnly), _=>None }
}
pub(super) fn discover(root:&ProjectRoot, html:&RelativePath)->ve::VisualResult<FileScope> {
    let mut chosen=Vec::new();
    let mut stack=vec![root.path().to_path_buf()];
    while let Some(dir)=stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry=entry?;
            let relative=entry.path().strip_prefix(root.path()).map_err(|_| ve::VisualError::Conflict("scope path escaped root"))?.to_string_lossy().replace('\\',"/");
            let Ok(rel)=RelativePath::parse(&relative) else { continue };
            let metadata=fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() { return Err(ve::VisualError::Conflict("link in discovered scope")); }
            if metadata.is_dir() { ProjectRoot::from_verified_path(&entry.path())?; stack.push(entry.path()); continue; }
            if !metadata.is_file() { continue; }
            let ext=rel.as_str().rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if let Some(access)=known_access(&ext) {
                chosen.push(SelectedFile {path:relative,access});
                if chosen.len()>ve::MAX_FILES { return Err(ve::VisualError::Limit("discovered file count")); }
            }
        }
    }
    if !chosen.iter().any(|f| f.path==html.as_str() && f.access==FileAccess::Edit) { return Err(ve::VisualError::Conflict("selected HTML is not an editable file")); }
    FileScope::new(chosen)
}
/// Markdown 文書の編集対象: 文書そのもの（編集可）と、文書から相対参照されるルート内の画像（読み取り専用）。
pub(super) fn discover_markdown(root:&ProjectRoot, doc:&RelativePath)->ve::VisualResult<FileScope> {
    use std::io::Read;
    let mut bytes=Vec::new();
    {
        let held=root.open_relative(doc)?.ok_or(ve::VisualError::Conflict("markdown file not found"))?;
        (&held.file).take(ve::MAX_FILE_BYTES+1).read_to_end(&mut bytes)?;
    }
    if bytes.len() as u64>ve::MAX_FILE_BYTES { return Err(ve::VisualError::Limit("single file bytes")); }
    let mut chosen=vec![SelectedFile {path:doc.as_str().to_string(),access:FileAccess::Edit}];
    let mut seen=std::collections::BTreeSet::from([doc.as_str().to_uppercase()]);
    for href in ve::markdown::extract_refs(&String::from_utf8_lossy(&bytes)) {
        let Some(rel)=ve::markdown::resolve_ref(doc.as_str(),&href) else { continue };
        if ve::markdown::image_mime(&rel).is_none() || !seen.insert(rel.to_uppercase()) { continue; }
        let Ok(parsed)=RelativePath::parse(&rel) else { continue };
        if !matches!(root.open_relative(&parsed),Ok(Some(_))) { continue; }
        chosen.push(SelectedFile {path:rel,access:FileAccess::ReadOnly});
    }
    FileScope::new(chosen)
}
#[cfg(test)]
mod markdown_scope_tests {
    use super::*;
    #[test]
    fn markdown_scope_is_the_document_plus_referenced_images_read_only() {
        let base=std::env::temp_dir().join(format!("agentdock-mdscope-{}",std::process::id()));
        let _=fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("img")).unwrap();
        fs::write(base.join("a.md"),"![x](img/p.png) ![y](missing.png) ![z](../out.png) [l](other.md)").unwrap();
        fs::write(base.join("img").join("p.png"),[1u8,2,3]).unwrap();
        fs::write(base.join("other.md"),"x").unwrap();
        let root=ProjectRoot::open(&base).unwrap();
        let scope=discover_markdown(&root,&RelativePath::parse("a.md").unwrap()).unwrap();
        let got:Vec<_>=scope.selected().iter().map(|f|(f.path.clone(),f.access)).collect();
        drop(root);
        let _=fs::remove_dir_all(&base);
        assert_eq!(got,vec![("a.md".to_string(),FileAccess::Edit),("img/p.png".to_string(),FileAccess::ReadOnly)]);
    }
}
#[cfg(test)]
mod discover_tests {
    use super::*;
    #[test]
    fn discovers_files_in_subdirectories_of_a_canonical_root() {
        let base=std::env::temp_dir().join(format!("agentdock-discover-{}",std::process::id()));
        let _=fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("assets")).unwrap();
        fs::write(base.join("index.html"),"<p>x</p>").unwrap();
        fs::write(base.join("assets").join("mark.svg"),"<svg/>").unwrap();
        let root=ProjectRoot::open(&base).unwrap();
        let html=RelativePath::parse("index.html").unwrap();
        let scope=discover(&root,&html).expect("subdirectory must not be rejected");
        let paths:Vec<_>=scope.selected().iter().map(|f|f.path.clone()).collect();
        let _=fs::remove_dir_all(&base);
        assert!(paths.contains(&"index.html".to_string()) && paths.contains(&"assets/mark.svg".to_string()), "{paths:?}");
        assert!(scope.selected().iter().all(|f| f.access==FileAccess::Edit), "svg and html must be editable");
    }
}
#[cfg(test)]
mod pin_save_tests {
    use super::*;
    #[test]
    fn pinned_root_blocks_directory_swap_but_not_saves_in_ancestors() {
        let base=std::env::temp_dir().join(format!("agentdock-pin-{}",std::process::id()));
        let _=fs::remove_dir_all(&base);
        let child=base.join("a").join("b"); fs::create_dir_all(&child).unwrap();
        let state=base.join("a").join("state.json");
        crate::store::atomic::write_atomic(&state,b"1").unwrap();
        let pins=ProjectRoot::open(&child).unwrap();
        let ancestor=crate::store::atomic::write_atomic(&state,b"2");
        let inside=crate::store::atomic::write_atomic(&child.join("x.json"),b"2");
        let swap=fs::rename(&child,base.join("a").join("moved"));
        drop(pins);
        let _=fs::remove_dir_all(&base);
        assert!(ancestor.is_ok(), "save in pinned ancestor failed: {ancestor:?}");
        assert!(inside.is_ok(), "save inside pinned dir failed: {inside:?}");
        assert!(swap.is_err(), "pinned directory must not be renamable");
    }
}

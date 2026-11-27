use super::*;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    title: String,
    url: String,
    text: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct ChatLinks {
    current: Option<String>,
    seen: BTreeSet<String>,
}

pub fn chat_protected(store: &Meetings, chat: Option<&str>) -> bool {
    let data = store.data.lock().unwrap();
    !may_disclose(&data.items)
        && data.items.iter().any(|item| {
            let dir = store.root.join(&item.id);
            read::<ChatLinks>(&dir, "chat-link.json").map_or(true, |links| match chat {
                Some(id) => links.seen.contains(id),
                None => !links.seen.is_empty(),
            })
        })
}

#[tauri::command]
pub fn meeting_chat_privacy(store: State<'_, Meetings>) -> bool {
    chat_protected(&store, None)
}

#[derive(Deserialize, Serialize)]
pub struct DraftRequest {
    marker: String,
    minutes: bool,
}

#[derive(Default, Serialize)]
pub struct MeetingFiles {
    chat: Option<String>,
    materials: Vec<Material>,
    notes: String,
    transcript: String,
    minutes: String,
    request: Option<DraftRequest>,
    draft: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum FileEdit {
    Chat(Option<String>),
    Materials(Vec<Material>),
    Notes(String),
    Transcript(String),
    Minutes(String),
    Request(Option<DraftRequest>),
    Draft(String),
}

fn read<T: serde::de::DeserializeOwned + Default>(
    dir: &std::path::Path,
    file: &str,
) -> Result<T, String> {
    match std::fs::read(dir.join(file)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| format!("{file} を読み込めません。"))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(_) => Err(format!("{file} を読み込めません。")),
    }
}

fn write(dir: &std::path::Path, file: &str, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    crate::store::atomic::write_atomic(&dir.join(file), &bytes)
        .map_err(|_| format!("{file} を保存できません。"))
}

fn meeting_dir(store: &Meetings, data: &Data, id: &str) -> Result<PathBuf, String> {
    if !may_disclose(&data.items) {
        return Err("共有モードを解除してください。".into());
    }
    if !data.items.iter().any(|m| m.id == id) {
        return Err("会議が見つかりません。".into());
    }
    Ok(store.root.join(id))
}

#[tauri::command]
pub fn meeting_files(store: State<'_, Meetings>, id: String) -> Result<MeetingFiles, String> {
    let data = store.data.lock().unwrap();
    let dir = meeting_dir(&store, &data, &id)?;
    Ok(MeetingFiles {
        chat: read::<ChatLinks>(&dir, "chat-link.json")?.current,
        materials: read(&dir, "materials.json")?,
        notes: read(&dir, "notes.json")?,
        transcript: read(&dir, "transcript.json")?,
        minutes: read(&dir, "minutes.json")?,
        request: read(&dir, "minutes-request.json")?,
        draft: read(&dir, "minutes-draft.json")?,
    })
}

#[tauri::command]
pub fn meeting_file_edit(
    store: State<'_, Meetings>,
    id: String,
    edit: FileEdit,
) -> Result<(), String> {
    let data = store.data.lock().unwrap();
    let dir = meeting_dir(&store, &data, &id)?;
    edit_file(&dir, edit)
}

fn edit_file(dir: &std::path::Path, edit: FileEdit) -> Result<(), String> {
    match edit {
        FileEdit::Chat(value) => {
            if value.as_ref().is_some_and(|id| id.trim().is_empty()) {
                return Err("チャットを選択してください。".into());
            }
            let mut links: ChatLinks = read(dir, "chat-link.json")?;
            links.seen.extend(value.iter().cloned());
            links.current = value;
            write(dir, "chat-link.json", &links)
        }
        FileEdit::Materials(value) => {
            if value
                .iter()
                .any(|item| item.title.trim().is_empty() || !web_url(&item.url))
            {
                return Err("資料名と HTTP(S) URL を確認してください。".into());
            }
            write(dir, "materials.json", &value)
        }
        FileEdit::Notes(value) => write(dir, "notes.json", &value),
        FileEdit::Transcript(value) => {
            if value.trim().is_empty() {
                return Err("文字起こしが空です。".into());
            }
            write(dir, "transcript.json", &value)
        }
        FileEdit::Request(value) => write(dir, "minutes-request.json", &value),
        FileEdit::Draft(value) => write(dir, "minutes-draft.json", &value),
        FileEdit::Minutes(value) => {
            if value.trim().is_empty() {
                return Err("確定する議事録を入力してください。".into());
            }
            write(dir, "minutes.json", &value)
        }
    }
}

fn web_url(value: &str) -> bool {
    tauri::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

#[tauri::command]
pub fn meeting_material_open(
    app: tauri::AppHandle,
    store: State<'_, Meetings>,
    id: String,
    index: usize,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let data = store.data.lock().unwrap();
    let dir = meeting_dir(&store, &data, &id)?;
    let materials: Vec<Material> = read(&dir, "materials.json")?;
    let material = materials
        .get(index)
        .filter(|item| web_url(&item.url))
        .ok_or("資料リンクが見つかりません。")?;
    app.opener()
        .open_url(&material.url, None::<&str>)
        .map_err(|_| "資料リンクを開けませんでした。".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_files_keep_links_notes_and_final_minutes_separate() {
        let root = std::env::temp_dir().join(format!(
            "agentdock-meeting-files-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        edit_file(&root, FileEdit::Chat(Some("sample-chat".into()))).unwrap();
        edit_file(&root, FileEdit::Chat(None)).unwrap();
        let links: ChatLinks = read(&root, "chat-link.json").unwrap();
        assert!(links.current.is_none());
        assert!(links.seen.contains("sample-chat"));
        let item = Material {
            title: "Agenda".into(),
            url: "https://example.com/agenda".into(),
            text: None,
        };
        edit_file(&root, FileEdit::Materials(vec![item])).unwrap();
        let materials: Vec<Material> = read(&root, "materials.json").unwrap();
        assert!(materials[0].text.is_none());
        assert!(web_url(&materials[0].url));
        assert!(!web_url("file:///sample"));
        edit_file(&root, FileEdit::Notes("Note".into())).unwrap();
        edit_file(
            &root,
            FileEdit::Transcript("WEBVTT\n\n00:00.000 --> 00:04.000\nRelease approved.".into()),
        )
        .unwrap();
        edit_file(
            &root,
            FileEdit::Draft("Mock draft: release approved".into()),
        )
        .unwrap();
        assert_eq!(read::<String>(&root, "minutes.json").unwrap(), "");
        edit_file(
            &root,
            FileEdit::Minutes("Reviewed decision: release approved".into()),
        )
        .unwrap();
        assert_eq!(
            read::<String>(&root, "minutes.json").unwrap(),
            "Reviewed decision: release approved"
        );
        assert_eq!(read::<String>(&root, "notes.json").unwrap(), "Note");
        assert!(!root.join("meeting.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! thread 単位の設定上書き（`thread/start`・`thread/resume` の `config`・`developerInstructions`）。
//! 利用者の `~/.codex` は書き換えない。上書きはその thread にだけ効く。

use serde_json::{json, Map, Value};

/// `config` のキーの書き方。App Server がネスト形式を受け付けるかドット形式かは実機で確かめる（未確認）。
/// 効かなかったときはここだけを切り替える。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStyle {
    /// `{"agents": {"enabled": false}}`
    Nested,
    /// `{"agents.enabled": false}`
    Dotted,
}

/// 現在使う形式（1か所）。
pub const KEY_STYLE: KeyStyle = KeyStyle::Nested;

/// 子エージェント（multi-agent）を使わせない設定。`agents.enabled=false` と `features.multi_agent=false`。
pub fn no_subagents_config() -> Value {
    no_subagents_config_in(KEY_STYLE)
}

pub fn no_subagents_config_in(style: KeyStyle) -> Value {
    match style {
        KeyStyle::Nested => json!({"agents": {"enabled": false}, "features": {"multi_agent": false}}),
        KeyStyle::Dotted => json!({"agents.enabled": false, "features.multi_agent": false}),
    }
}

/// 開始・再開の要求へ上書きを足す（None は何も足さない）。
pub fn apply_overrides(p: &mut Value, config: Option<&Value>, developer_instructions: Option<&str>) {
    let Some(obj) = p.as_object_mut() else { return };
    insert_overrides(obj, config, developer_instructions);
}

fn insert_overrides(obj: &mut Map<String, Value>, config: Option<&Value>, developer_instructions: Option<&str>) {
    if let Some(c) = config {
        obj.insert("config".into(), c.clone());
    }
    if let Some(d) = developer_instructions {
        obj.insert("developerInstructions".into(), json!(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_style_is_one_constant_and_both_forms_disable_subagents() {
        assert_eq!(KEY_STYLE, KeyStyle::Nested);
        assert_eq!(no_subagents_config(), json!({"agents": {"enabled": false}, "features": {"multi_agent": false}}));
        assert_eq!(no_subagents_config_in(KeyStyle::Dotted), json!({"agents.enabled": false, "features.multi_agent": false}));
    }

    #[test]
    fn overrides_are_added_only_when_given() {
        let mut p = json!({"cwd": "C:/w"});
        apply_overrides(&mut p, None, None);
        assert_eq!(p, json!({"cwd": "C:/w"}));
        apply_overrides(&mut p, Some(&no_subagents_config()), Some("相談だけ"));
        assert_eq!(p["config"]["agents"]["enabled"], json!(false));
        assert_eq!(p["developerInstructions"], json!("相談だけ"));
    }
}

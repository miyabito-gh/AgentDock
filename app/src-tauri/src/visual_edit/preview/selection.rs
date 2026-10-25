use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub tag: String,
    pub id: String,
    pub class_name: String,
    pub text: String,
    pub selector: String,
    pub rect: Rect,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    selection: Vec<Selection>,
    #[serde(default)]
    truncated: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionEvent {
    pub session_id: String,
    pub selection: Vec<Selection>,
    pub truncated: bool,
}
pub(super) fn validate(raw: &str) -> Option<(Vec<Selection>, bool)> {
    if raw.len() > 65536 {
        return None;
    }
    let m: Message = serde_json::from_str(raw).ok()?;
    if m.selection.len() > 64 {
        return None;
    }
    for s in &m.selection {
        if s.tag.is_empty()
            || s.tag.chars().count() > 64
            || [&s.id, &s.class_name, &s.text, &s.selector]
                .iter()
                .any(|s| s.chars().count() > 1024)
        {
            return None;
        }
        let r = &s.rect;
        if [r.x, r.y, r.width, r.height]
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1e7)
            || r.width < 0.0
            || r.height < 0.0
        {
            return None;
        }
    }
    Some((m.selection, m.truncated))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_rejects_commands_extra_fields_and_bad_geometry() {
        assert!(validate(r#"{"selection":[]}"#).is_some());
        for raw in [
            r#"{"command":"respond_request","selection":[]}"#,
            r#"{"selection":[{"tag":"div","id":"","className":"","text":"","selector":"div","rect":{"x":0,"y":0,"width":-1,"height":1}}]}"#,
            r#"{"selection":[],"rootPath":"C:/"}"#,
        ] {
            assert!(validate(raw).is_none());
        }
        assert!(validate(&" ".repeat(65537)).is_none());
    }
}

/// Fixed window rate limit, applied before COM strings and JSON are copied/parsed.
pub(super) struct MessageBudget {
    start: std::time::Instant,
    count: u8,
}
impl MessageBudget {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
            count: 0,
        }
    }
    pub fn admit(&mut self, now: std::time::Instant) -> bool {
        if now.duration_since(self.start) >= std::time::Duration::from_secs(1) {
            self.start = now;
            self.count = 0;
        }
        if self.count >= 20 {
            return false;
        }
        self.count += 1;
        true
    }
}
#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn observer_drops_flood_then_recovers_next_window() {
        let mut b = MessageBudget::new();
        let now = b.start;
        for _ in 0..20 {
            assert!(b.admit(now));
        }
        for _ in 0..100 {
            assert!(!b.admit(now));
        }
        assert!(b.admit(now + std::time::Duration::from_secs(1)));
    }
}

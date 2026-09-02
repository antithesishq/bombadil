use crate::{Point, schema::TraceEntry};
use serde::{Deserialize, Serialize};
use serde_json as json;

pub type BrowserTraceEntry = TraceEntry<BrowserAction, BrowserStateSummary>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserStateSummary {
    pub url: String,
    pub hash_previous: Option<u64>,
    pub hash_current: Option<u64>,
    pub screenshot: String,
    pub resources: Resources,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resources {
    pub js_heap_used: u64,
    pub js_heap_total: u64,
    pub dom_nodes: u64,
    pub documents: u64,
    pub js_event_listeners: u64,
    pub layout_objects: u64,
    pub timestamp: f64,
    pub thread_time: f64,
    pub task_duration: f64,
    pub script_duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fingerprint {
    // Universal strong identifiers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessible_name: Option<String>,
    pub tag: String,

    // Type-specific weak identifiers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_attr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,

    // Fallbacks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_content: Option<String>, // truncated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structural_path: Option<String>, // only when no strong identifier
}

impl Fingerprint {
    pub fn matches(&self, other: &Fingerprint) -> bool {
        // test-ids
        if let (Some(test_id_self), Some(test_id_other)) =
            (&self.test_id, &other.test_id)
        {
            return test_id_self == test_id_other;
        }

        // ids
        if let (Some(id_self), Some(id_other)) = (&self.id, &other.id) {
            return id_self == id_other;
        }

        // (role, accessible_name) pair
        if let (Some(role_self), Some(role_other)) = (&self.role, &other.role)
            && let (Some(accessible_name_self), Some(accessible_name_other)) =
                (&self.accessible_name, &other.accessible_name)
        {
            if role_self == role_other
                && accessible_name_self == accessible_name_other
            {
                return true;
            }
            if role_self == role_other {
                return false;
            }
        }

        // tag-specific
        match self.tag.as_str() {
            "a" => {
                if let (Some(name_self), Some(name_other)) =
                    (&self.href, &other.href)
                    && name_self == name_other
                {
                    return match (&self.accessible_name, &other.accessible_name)
                    {
                        (
                            Some(accessible_name_self),
                            Some(accessible_name_other),
                        ) => accessible_name_self == accessible_name_other,
                        _ => true,
                    };
                }
            }
            "button" => {
                match (&self.accessible_name, &other.accessible_name) {
                    (
                        Some(accessible_name_self),
                        Some(accessible_name_other),
                    ) => {
                        return accessible_name_self == accessible_name_other
                            && self.tag == other.tag;
                    }
                    (Some(_), None) | (None, Some(_)) => return false,
                    (None, None) => {}
                }

                return self.tag == other.tag
                    && matches!(
                        (
                            &self.input_type,
                            &other.input_type,
                            &self.text_content,
                            &other.text_content,
                        ),
                        (
                            Some(input_type_self),
                            Some(input_type_other),
                            Some(text_content_self),
                            Some(text_content_other),
                        ) if input_type_self == input_type_other
                            && !text_content_self.is_empty()
                            && text_content_self == text_content_other
                    );
            }
            "input" | "textarea" | "select" => {
                if let (Some(name_attr_self), Some(name_attr_other)) =
                    (&self.name_attr, &other.name_attr)
                    && name_attr_self == name_attr_other
                    && self.input_type == other.input_type
                {
                    return true;
                }
                if let (Some(placeholder_self), Some(placeholder_other)) =
                    (&self.placeholder, &other.placeholder)
                    && placeholder_self == placeholder_other
                    && self.input_type == other.input_type
                {
                    return true;
                }
            }
            _ => {}
        }

        // last resort, only populated when strong identifiers absent
        if let (Some(structural_path_self), Some(structural_path_other)) =
            (&self.structural_path, &other.structural_path)
        {
            return structural_path_self == structural_path_other;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::Fingerprint;

    fn button(
        input_type: Option<&str>,
        text_content: Option<&str>,
    ) -> Fingerprint {
        Fingerprint {
            test_id: None,
            id: None,
            role: None,
            accessible_name: None,
            tag: "button".to_owned(),
            href: None,
            name_attr: None,
            placeholder: None,
            input_type: input_type.map(str::to_owned),
            text_content: text_content.map(str::to_owned),
            structural_path: None,
        }
    }

    #[test]
    fn button_matches_same_type_and_nonempty_text() {
        let candidate = button(Some("button"), Some("Draw"));
        let original = button(Some("button"), Some("Draw"));

        assert!(candidate.matches(&original));
        assert!(original.matches(&candidate));
    }

    #[test]
    fn button_rejects_different_text() {
        let candidate = button(Some("button"), Some("Erase"));
        let original = button(Some("button"), Some("Draw"));

        assert!(!candidate.matches(&original));
        assert!(!original.matches(&candidate));
    }

    #[test]
    fn button_rejects_different_type() {
        let candidate = button(Some("submit"), Some("Draw"));
        let original = button(Some("button"), Some("Draw"));

        assert!(!candidate.matches(&original));
        assert!(!original.matches(&candidate));
    }

    #[test]
    fn button_rejects_missing_or_empty_fallback_fields() {
        let complete = button(Some("button"), Some("Draw"));
        let incomplete = [
            button(None, Some("Draw")),
            button(Some("button"), None),
            button(Some("button"), Some("")),
        ];

        for candidate in incomplete {
            assert!(!candidate.matches(&complete));
            assert!(!complete.matches(&candidate));
        }

        let mut missing_type = button(None, Some("Draw"));
        missing_type.structural_path = Some("html/body/button[1]".to_owned());
        assert!(!missing_type.matches(&missing_type));

        let mut missing_text = button(Some("button"), None);
        missing_text.structural_path = Some("html/body/button[1]".to_owned());
        assert!(!missing_text.matches(&missing_text));
    }

    #[test]
    fn button_rejects_one_sided_accessible_name() {
        let mut candidate = button(Some("button"), Some("Draw"));
        candidate.accessible_name = Some("Draw tool".to_owned());
        let original = button(Some("button"), Some("Draw"));

        assert!(!candidate.matches(&original));
        assert!(!original.matches(&candidate));
    }

    #[test]
    fn button_prefers_paired_accessible_name_over_fallback_fields() {
        let mut candidate = button(Some("submit"), Some("Candidate text"));
        candidate.accessible_name = Some("Draw tool".to_owned());
        let mut original = button(Some("button"), Some("Original text"));
        original.accessible_name = Some("Draw tool".to_owned());

        assert!(candidate.matches(&original));
        assert!(original.matches(&candidate));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BrowserAction {
    Back,
    Forward,
    Click {
        fingerprint: Fingerprint,
        point: Point,
    },
    DoubleClick {
        fingerprint: Fingerprint,
        point: Point,
        delay_millis: u64,
    },
    TypeText {
        text: String,
        delay_millis: u64,
    },
    PressKey {
        code: u8,
    },
    ScrollUp {
        origin: Point,
        distance: f64,
    },
    ScrollDown {
        origin: Point,
        distance: f64,
    },
    Reload,
    Wait,
    SetFileInputFiles {
        selector: String,
        files: Vec<String>,
    },
    MouseDrag {
        from: Point,
        to: Point,
        steps: u8,
        delay_millis: u64,
    },
    SetViewport {
        width: u16,
        height: u16,
    },
    Custom {
        name: String,
        arguments: Vec<json::Value>,
    },
}

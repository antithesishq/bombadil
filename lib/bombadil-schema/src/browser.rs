use crate::{Point, schema::TraceEntry};
use serde::{Deserialize, Serialize};

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
                if let (Some(accessible_name_self), Some(accessible_name_other)) =
                    (&self.accessible_name, &other.accessible_name)
                    && accessible_name_self == accessible_name_other
                    && self.tag == other.tag
                {
                    return true;
                }
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
        state: serde_json::Value,
    },
}

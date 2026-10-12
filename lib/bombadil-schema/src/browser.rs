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

/// DOM element fingerprint. Only non-blank strings are allowed
///  in `Some` values.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
)]
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
    pub structural_path: Option<String>, // only when no other identifier is present
}

impl Fingerprint {
    pub fn validate(&self) -> Result<(), String> {
        fn validate_non_blank_option(
            field_name: &str,
            value: &Option<String>,
        ) -> Result<(), String> {
            if let Some(value) = value
                && value.trim().is_empty()
            {
                Err(format!("{field_name} is set but is blank"))
            } else {
                Ok(())
            }
        }

        validate_non_blank_option("test_id", &self.test_id)?;
        validate_non_blank_option("id", &self.id)?;
        validate_non_blank_option("role", &self.role)?;
        validate_non_blank_option("accessible_name", &self.accessible_name)?;
        if self.tag.trim().is_empty() {
            return Err("tag is blank".to_string());
        }
        validate_non_blank_option("href", &self.href)?;
        validate_non_blank_option("name_attr", &self.name_attr)?;
        validate_non_blank_option("placeholder", &self.placeholder)?;
        validate_non_blank_option("input_type", &self.input_type)?;
        validate_non_blank_option("text_content", &self.text_content)?;
        validate_non_blank_option("structural_path", &self.structural_path)?;

        let has_identifier = self.test_id.is_some()
            || self.id.is_some()
            || self.role.is_some()
            || self.accessible_name.is_some()
            || self.href.is_some()
            || self.name_attr.is_some()
            || self.placeholder.is_some()
            || self.input_type.is_some()
            || self.text_content.is_some();
        match (has_identifier, self.structural_path.is_some()) {
            (true, true) => {
                Err("structural_path is set together with other identifiers"
                    .to_string())
            }
            (false, false) => {
                Err("neither structural_path nor any other identifier is set"
                    .to_string())
            }
            _ => Ok(()),
        }
    }

    pub fn matches(&self, other: &Fingerprint) -> bool {
        assert_eq!(self.validate(), Ok(()));
        assert_eq!(other.validate(), Ok(()));

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

        // tag-specific, only between elements of the same tag
        if self.tag == other.tag {
            match self.tag.as_str() {
                "a" => {
                    if let (Some(name_self), Some(name_other)) =
                        (&self.href, &other.href)
                        && name_self == name_other
                    {
                        return match (
                            &self.accessible_name,
                            &other.accessible_name,
                        ) {
                            (
                                Some(accessible_name_self),
                                Some(accessible_name_other),
                            ) => accessible_name_self == accessible_name_other,
                            _ => true,
                        };
                    }
                }
                "button" => {
                    if let (
                        Some(accessible_name_self),
                        Some(accessible_name_other),
                    ) = (&self.accessible_name, &other.accessible_name)
                        && accessible_name_self == accessible_name_other
                    {
                        return true;
                    }
                    if let (
                        Some(input_type_self),
                        Some(input_type_other),
                        Some(text_content_self),
                        Some(text_content_other),
                    ) = (
                        &self.input_type,
                        &other.input_type,
                        &self.text_content,
                        &other.text_content,
                    ) && input_type_self == input_type_other
                        && text_content_self == text_content_other
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
        }

        // weak identifiers only (no rule above applies), require all equal
        if self == other {
            return true;
        }

        // last resort, only populated when all other identifiers are absent
        // (guaranteed by `validate`)
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
        #[deprecated]
        delay_millis: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::Fingerprint;
    use hegel::{
        Generator, PrintableGenerator, TestCase,
        generators::{integers, just, optional, text, vecs},
        one_of,
    };

    fn tags() -> impl PrintableGenerator<String> {
        one_of!(
            just("a"),
            just("button"),
            just("input"),
            just("textarea"),
            just("select"),
            just("div"),
        )
        .map(str::to_string)
    }

    #[hegel::composite]
    fn structural_path_segment(tc: &TestCase) -> String {
        let tag = tc.draw(text().alphabet("ab"));
        let index = tc.draw(integers().min_value(0).max_value(5));
        format!("{tag}[{index}]")
    }

    #[hegel::composite]
    fn fingerprints_structural_path(tc: &TestCase) -> Fingerprint {
        let structural_path = tc.draw(
            vecs(structural_path_segment())
                .min_size(1)
                .map(|segments| segments.join(" > ")),
        );
        let tag = tc.draw(tags());
        let fingerprint = Fingerprint {
            structural_path: Some(structural_path),
            tag,
            ..Default::default()
        };
        if fingerprint.validate().is_err() {
            tc.reject()
        } else {
            fingerprint
        }
    }

    #[hegel::composite]
    fn fingerprints_strong_identifiers(tc: &TestCase) -> Fingerprint {
        let test_id =
            tc.draw(optional(text().alphabet("ab-").min_size(1).max_size(5)));
        let id =
            tc.draw(optional(text().alphabet("ab-").min_size(1).max_size(5)));
        let role =
            tc.draw(optional(text().alphabet("ab").min_size(1).max_size(5)));
        let accessible_name =
            tc.draw(optional(text().alphabet("ab-").min_size(1).max_size(5)));
        let tag = tc.draw(tags());
        let href = tc.draw(optional(
            one_of!(just("/"), just("/a"), just("https://example.com/"))
                .map(str::to_string),
        ));
        let name_attr =
            tc.draw(optional(text().alphabet("ab-").min_size(1).max_size(5)));
        let placeholder = tc.draw(optional(
            text()
                .alphabet("ab ")
                .max_size(3)
                .filter(|text| !text.trim().is_empty()),
        ));
        let input_type = tc.draw(optional(
            one_of!(just("button"), just("submit"), just("number"))
                .map(str::to_string),
        ));
        let text_content = tc.draw(optional(
            text()
                .alphabet("ab ")
                .max_size(3)
                .filter(|text| !text.trim().is_empty()),
        ));
        let fingerprint = Fingerprint {
            test_id,
            id,
            role,
            accessible_name,
            tag,
            href,
            name_attr,
            placeholder,
            input_type,
            text_content,
            structural_path: None,
        };
        if fingerprint.validate().is_err() {
            tc.reject()
        } else {
            fingerprint
        }
    }

    #[hegel::composite]
    fn fingerprints(tc: &TestCase) -> Fingerprint {
        tc.draw_silent(one_of!(
            fingerprints_strong_identifiers(),
            fingerprints_structural_path()
        ))
    }

    #[hegel::test]
    fn test_fingerprint_match_identity(tc: TestCase) {
        let fingerprint = tc.draw(fingerprints().print_as_debug());
        assert!(fingerprint.matches(&fingerprint));
    }

    #[hegel::test]
    fn test_fingerprint_match_commutative(tc: TestCase) {
        let left = tc.draw(fingerprints().print_as_debug());
        let right = tc.draw(fingerprints().print_as_debug());
        assert_eq!(left.matches(&right), right.matches(&left));
    }
}

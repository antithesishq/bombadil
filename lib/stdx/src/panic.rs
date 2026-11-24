use std::any::Any;

/// The message of a caught panic, unless its payload isn't a string.
pub fn message(payload: &(dyn Any + Send)) -> Option<&str> {
    // A `&Box<dyn Any + Send>` coerces to a reference to the box itself.
    let payload = match payload.downcast_ref::<Box<dyn Any + Send>>() {
        Some(inner) => inner.as_ref(),
        None => payload,
    };
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::message;
    use hegel::{TestCase, generators::text};
    use std::panic::{catch_unwind, panic_any};

    #[hegel::test]
    fn message_is_the_panic_text(tc: TestCase) {
        let text = tc.draw(text());
        let payload = catch_unwind(|| panic!("{text}")).unwrap_err();
        assert_eq!(message(payload.as_ref()), Some(text.as_str()));
        assert_eq!(message(&payload), Some(text.as_str()));
    }

    #[test]
    fn message_of_literal_panic() {
        let payload = catch_unwind(|| panic!("literal")).unwrap_err();
        assert_eq!(message(payload.as_ref()), Some("literal"));
    }

    #[test]
    fn no_message_for_other_payloads() {
        let payload = catch_unwind(|| panic_any(1u8)).unwrap_err();
        assert_eq!(message(payload.as_ref()), None);
    }
}

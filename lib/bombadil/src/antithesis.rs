use std::sync::LazyLock;

static IS_IN_GUEST: LazyLock<bool> =
    LazyLock::new(|| std::env::var("ANTITHESIS_OUTPUT_DIR").is_ok());

pub fn is_in_guest() -> bool {
    *IS_IN_GUEST
}

pub fn is_in_guest() -> bool {
    std::env::var("ANTITHESIS_OUTPUT_DIR").is_ok()
}

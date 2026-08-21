use std::ffi::{CString, c_char, c_int};

#[link(name = "libvoidstar_shim", kind = "static")]
unsafe extern "C" {
    fn antithesis_load_libvoidstar();
    fn antithesis_init_coverage_module(
        edge_count: usize,
        symbol_file_name: *const c_char,
    ) -> usize;
    fn antithesis_notify_coverage(edge_index: usize);
    fn antithesis_fuzz_getchar() -> c_int;
}

#[used]
// https://refspecs.linuxbase.org/LSB_3.0.0/LSB-PDA/LSB-PDA/specialsections.html
#[cfg_attr(target_os = "linux", unsafe(link_section = ".init_array"))]
// https://github.com/aidansteele/osx-abi-macho-file-format-reference#table-2-the-sections-of-a__datasegment
#[cfg_attr(
    target_os = "macos",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
static _ANTITHESIS_INIT: unsafe extern "C" fn() = antithesis_load_libvoidstar;

pub fn init_coverage_module(
    edge_count: usize,
    symbol_file_name: &str,
) -> usize {
    let c_str = CString::new(symbol_file_name)
        .expect("NUL terminator in symbol_file_name");
    let offset =
        unsafe { antithesis_init_coverage_module(edge_count, c_str.as_ptr()) };
    log::debug!(
        "init_coverage_module({edge_count:?}, {symbol_file_name:?}) -> {offset:?}"
    );
    offset
}

pub fn notify_coverage(edge_index: usize) {
    log::debug!("notify_coverage({edge_index:?})");
    unsafe { antithesis_notify_coverage(edge_index) }
}

pub fn mark_state_boundary() {
    // We call this as a way of telling the Antithesis
    // fuzzer that this is the boundary of a state,
    // from where it may fork off and try other entropy
    // input.
    let _ = unsafe { antithesis_fuzz_getchar() };
}

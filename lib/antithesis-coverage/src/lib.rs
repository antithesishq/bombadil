use std::ffi::{CString, c_char};

#[link(name = "antithesis_coverage_shim", kind = "static")]
unsafe extern "C" {
    fn antithesis_load_libvoidstar();
    fn antithesis_init_coverage_module(
        edge_count: usize,
        symbol_file_name: *const c_char,
    ) -> usize;
    fn antithesis_notify_coverage(edge_index: usize);
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

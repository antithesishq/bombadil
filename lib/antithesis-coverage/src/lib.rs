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
#[unsafe(link_section = ".init_array")]
static _ANTITHESIS_INIT: unsafe extern "C" fn() = antithesis_load_libvoidstar;

pub fn init_coverage_module(
    edge_count: usize,
    symbol_file_name: &str,
) -> usize {
    let c_str = CString::new(symbol_file_name)
        .expect("NUL terminator in symbol_file_name");
    unsafe { antithesis_init_coverage_module(edge_count, c_str.as_ptr()) }
}

pub fn notify_coverage(edge_index: usize) {
    unsafe { antithesis_notify_coverage(edge_index) }
}

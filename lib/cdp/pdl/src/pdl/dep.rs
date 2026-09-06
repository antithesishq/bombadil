use std::collections::HashSet;

use once_cell::sync::Lazy;

// circularDeps is the list of types that can cause circular dependency
// issues.
static CIRCULAR_DEPS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut m = HashSet::new();
    m.insert("browser.browsercontextid");
    m.insert("dom.backendnodeid");
    m.insert("dom.backendnode");
    m.insert("dom.nodeid");
    m.insert("dom.node");
    m.insert("dom.nodetype");
    m.insert("dom.pseudotype");
    m.insert("dom.rgba");
    m.insert("dom.shadowroottype");
    m.insert("network.loaderid");
    m.insert("network.monotonictime");
    m.insert("network.timesinceepoch");
    m.insert("page.frameid");
    m.insert("page.frame");
    m
});

// returns whether or not a type will cause circular dependency
// issues.
pub(crate) fn is_circular_dep(dty: &str, ty_str: &str) -> bool {
    CIRCULAR_DEPS
        .get(
            format!("{}.{}", dty.to_lowercase(), ty_str.to_lowercase())
                .as_str(),
        )
        .is_some()
}

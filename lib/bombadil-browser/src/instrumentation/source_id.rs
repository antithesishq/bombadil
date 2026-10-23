/// Identifies an instrumented script by where it comes from rather than by
/// its content, so that coverage IDs stay the same across loads and versions
/// of the script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceId(pub String);

impl SourceId {
    /// The path of `url`, with runs of 8 or more hex digits (as in
    /// cache-busting file names like `main.3f2a9c1b.js`) replaced by `*`.
    pub fn from_url(url: &str) -> Self {
        let path = url::Url::parse(url)
            .map(|url| url.path().to_string())
            .unwrap_or_else(|_| url.to_string());
        let mut key = String::with_capacity(path.len());
        let mut run = String::new();
        for c in path.chars().chain(std::iter::once('/')) {
            if c.is_ascii_hexdigit() {
                run.push(c);
                continue;
            }
            if run.len() >= 8 {
                key.push('*');
            } else {
                key.push_str(&run);
            }
            run.clear();
            key.push(c);
        }
        key.pop();
        SourceId(key)
    }

    pub fn inline(&self, index: usize) -> Self {
        SourceId(format!("{}#inline{index}", self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{
        TestCase,
        generators::{integers, optional, sampled_from, text, vecs},
    };

    const HEX: &str = "0123456789abcdefABCDEF";
    /// No `.`, so that the url crate doesn't normalize `.` and `..` segments
    /// away.
    const NOT_HEX: &str = "ghxyzGZ/_-~";

    #[hegel::composite]
    fn origins(tc: &TestCase) -> String {
        let scheme = tc.draw_silent(sampled_from(["http", "https"].as_slice()));
        let host = tc.draw_silent(sampled_from(
            ["localhost", "example.com", "cafe.beef"].as_slice(),
        ));
        let port = tc.draw_silent(optional(
            integers::<u16>().min_value(1024).max_value(65535),
        ));
        match port {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        }
    }

    /// Query and fragment. May hold hex runs, which must not show up in the
    /// key.
    #[hegel::composite]
    fn suffixes(tc: &TestCase) -> String {
        let part = || text().alphabet("abcdef0123456789xyz=&/").max_size(12);
        let mut suffix = String::new();
        if let Some(query) = tc.draw_silent(optional(part())) {
            suffix.push_str(&format!("?{query}"));
        }
        if let Some(fragment) = tc.draw_silent(optional(part())) {
            suffix.push_str(&format!("#{fragment}"));
        }
        suffix
    }

    #[hegel::composite]
    fn hex_runs(tc: &TestCase, min: usize, max: usize) -> String {
        tc.draw_silent(text().alphabet(HEX).min_size(min).max_size(max))
    }

    #[hegel::composite]
    fn not_hex(tc: &TestCase) -> String {
        tc.draw_silent(text().alphabet(NOT_HEX).min_size(1).max_size(4))
    }

    #[hegel::composite]
    fn stretches(tc: &TestCase) -> Vec<String> {
        tc.draw_silent(vecs(not_hex()).min_size(1).max_size(4))
    }

    fn interleave(stretches: &[String], between: &[String]) -> String {
        let mut path = String::from("/");
        for (index, stretch) in stretches.iter().enumerate() {
            if index > 0 {
                path.push_str(&between[index - 1]);
            }
            path.push_str(stretch);
        }
        path
    }

    #[hegel::test]
    fn from_url_depends_on_the_path_only(tc: TestCase) {
        let path = format!(
            "/{}",
            tc.draw(text().alphabet("abcdef0189xyz/_-").max_size(24))
        );
        let first =
            format!("{}{path}{}", tc.draw(origins()), tc.draw(suffixes()));
        let second =
            format!("{}{path}{}", tc.draw(origins()), tc.draw(suffixes()));
        assert_eq!(SourceId::from_url(&first), SourceId::from_url(&second));
    }

    #[hegel::test]
    fn from_url_masks_long_hex_runs(tc: TestCase) {
        let stretches = tc.draw(stretches());
        let runs = |tc: &TestCase| -> Vec<String> {
            (1..stretches.len())
                .map(|_| tc.draw(hex_runs(8, 40)))
                .collect()
        };
        let origin = tc.draw(origins());
        let first = interleave(&stretches, &runs(&tc));
        let second = interleave(&stretches, &runs(&tc));
        let masks = vec!["*".to_string(); stretches.len() - 1];

        let key = SourceId::from_url(&format!("{origin}{first}"));
        assert_eq!(key.0, interleave(&stretches, &masks));
        assert_eq!(key, SourceId::from_url(&format!("{origin}{second}")));
    }

    #[hegel::test]
    fn from_url_keeps_short_hex_runs(tc: TestCase) {
        let stretches = tc.draw(stretches());
        let runs: Vec<String> = (1..stretches.len())
            .map(|_| tc.draw(hex_runs(1, 7)))
            .collect();
        let path = interleave(&stretches, &runs);
        let url = format!("{}{path}", tc.draw(origins()));
        assert_eq!(SourceId::from_url(&url).0, path);
    }

    #[hegel::test]
    fn inline_keys_are_distinct_per_index(tc: TestCase) {
        let page = SourceId(tc.draw(text().alphabet("/ab#0").max_size(6)));
        let first = tc.draw(integers::<usize>().max_value(99));
        let second = tc.draw(integers::<usize>().max_value(99));
        assert_eq!(page.inline(first) == page.inline(second), first == second);
        assert_ne!(page.inline(first), page);
    }

    #[test]
    fn from_url_keeps_the_path_only() {
        assert_eq!(
            SourceId::from_url(
                "http://localhost:8766/examples/react/dist/app.bundle.js?v=3#x"
            )
            .0,
            "/examples/react/dist/app.bundle.js"
        );
    }

    #[test]
    fn from_url_masks_content_hashes() {
        assert_eq!(
            SourceId::from_url(
                "https://example.com/static/main.3f2a9c1b.chunk.js"
            )
            .0,
            "/static/main.*.chunk.js"
        );
        assert_eq!(
            SourceId::from_url("https://example.com/js/abc/deadbee.js").0,
            "/js/abc/deadbee.js"
        );
    }
}

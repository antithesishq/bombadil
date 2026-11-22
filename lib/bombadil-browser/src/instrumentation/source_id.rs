use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write;
use std::sync::Arc;

use url::Url;

/// A script's identity by origin and path, not content, so coverage IDs
/// survive reloads and new builds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(pub Arc<str>);

const HASH_LENGTH_MIN: usize = 8;
const TOKEN_DELIMITERS: [char; 3] = ['.', '-', '_'];

impl SourceId {
    /// Path and query of `url` with build hashes masked:
    /// `/assets/main.3f2a9c1b.js?v=9e8d7c6b` becomes `/assets/main.*.js?v=*`.
    /// The first token of a file name is kept, as `3f2a9c1b.js` has no other
    /// name. Scripts from another origin than `origin` get a `host[:port]`
    /// prefix: `cdn.example.com/lib/x.js`.
    pub fn from_url(url: &Url, origin: &Url) -> Self {
        let path = url.path();
        let (directory, file) =
            path.split_at(path.rfind('/').map_or(0, |slash| slash + 1));
        let mut key = key_with_host(url, origin);
        key.push_str(directory);
        push_masked(&mut key, file, false);
        if let Some(query) = url.query() {
            let mut separator = '?';
            for parameter in query.split('&') {
                key.push(separator);
                separator = '&';
                match parameter.split_once('=') {
                    Some((name, value)) if !name.is_empty() => {
                        key.push_str(name);
                        key.push('=');
                        push_masked(&mut key, value, true);
                    }
                    _ => key.push_str(parameter),
                }
            }
        }
        SourceId(key.into())
    }

    /// Like [`SourceId::from_url`], without masking.
    pub(crate) fn exact(url: &Url, origin: &Url) -> Self {
        let mut key = key_with_host(url, origin);
        key.push_str(url.path());
        if let Some(query) = url.query() {
            key.push('?');
            key.push_str(query);
        }
        SourceId(key.into())
    }

    pub fn inline(&self, index: usize) -> Self {
        SourceId(format!("{}#inline{index}", self.0).into())
    }
}

/// `host[:port]` for other origins, empty for `origin`. Paths start with
/// `/`, so the two never collide.
fn key_with_host(url: &Url, origin: &Url) -> String {
    let mut key = String::with_capacity(url.as_str().len());
    let same_origin = url.scheme() == origin.scheme()
        && url.host() == origin.host()
        && url.port() == origin.port();
    if let Some(host) = url.host_str()
        && !same_origin
    {
        key.push_str(host);
        if let Some(port) = url.port() {
            write!(key, ":{port}").expect("writing to a String");
        }
    }
    key
}

/// Appends `text` with build hashes replaced by `*`. Unless `named`, the
/// first non-empty token is kept as the name.
fn push_masked(key: &mut String, text: &str, mut named: bool) {
    for piece in text.split_inclusive(TOKEN_DELIMITERS) {
        let token = piece.strip_suffix(TOKEN_DELIMITERS).unwrap_or(piece);
        if named && is_build_hash(token) {
            key.push('*');
            key.push_str(&piece[token.len()..]);
        } else {
            key.push_str(piece);
            named |= !token.is_empty();
        }
    }
}

/// At least 8 hex characters with both a digit and a letter (webpack,
/// Parcel). Excludes dates (`20240115`) and words (`deadbeef`).
fn is_build_hash(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= HASH_LENGTH_MIN
        && bytes.iter().all(u8::is_ascii_hexdigit)
        && bytes.iter().any(u8::is_ascii_digit)
        && bytes.iter().any(u8::is_ascii_alphabetic)
}

/// Keeps masked IDs unique per URL. esbuild's `chunk-<hash>.js` files
/// would otherwise all share one.
#[derive(Debug, Default)]
pub(crate) struct SourceIds {
    /// Masked ID to the unmasked ID of the URL that owns it.
    urls: HashMap<SourceId, SourceId>,
}

impl SourceIds {
    /// `masked`, or `exact` if another URL already owns `masked`.
    pub(crate) fn claim(
        &mut self,
        masked: SourceId,
        exact: SourceId,
    ) -> SourceId {
        match self.urls.entry(masked) {
            Entry::Vacant(entry) => {
                let masked = entry.key().clone();
                entry.insert(exact);
                masked
            }
            Entry::Occupied(entry) if *entry.get() == exact => {
                entry.key().clone()
            }
            Entry::Occupied(_) => exact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrumentation::BlockRegistry;
    use hegel::{
        TestCase,
        generators::{booleans, integers, optional, sampled_from, text, vecs},
    };
    use std::collections::BTreeMap;

    const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
    const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const LETTERS: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DIGITS: &str = "0123456789";
    const ALPHANUMERIC: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    const HEX: &str = "0123456789abcdefABCDEF";
    const HEX_LETTERS: &str = "abcdefABCDEF";
    const NOT_HEX: &str = "ghijklmnopqrstuvwxyzGHIJKLMNOPQRSTUVWXYZ~";

    const SCHEMES: [(&str, u16); 2] = [("http", 80), ("https", 443)];
    const HOSTS: [&str; 4] = ["localhost", "example.com", "cafe.beef", "[::1]"];

    /// Host and optional non-default port, as in a key.
    #[derive(Clone, Debug, PartialEq, hegel::PrettyPrintable)]
    struct Authority {
        host: usize,
        port: Option<u16>,
    }

    impl std::fmt::Display for Authority {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str(HOSTS[self.host])?;
            match self.port {
                Some(port) => write!(f, ":{port}"),
                None => Ok(()),
            }
        }
    }

    /// A non-default port.
    #[hegel::composite]
    fn ports(tc: &TestCase) -> u16 {
        let defaults = SCHEMES.map(|(_, port)| port);
        let count = u16::MAX - defaults.len() as u16;
        let mut port =
            tc.draw_silent(integers::<u16>().min_value(1).max_value(count));
        for default in defaults {
            port += u16::from(port >= default);
        }
        port
    }

    #[hegel::composite]
    fn authorities(tc: &TestCase) -> Authority {
        Authority {
            host: tc
                .draw_silent(integers::<usize>().max_value(HOSTS.len() - 1)),
            port: tc.draw_silent(optional(ports())),
        }
    }

    /// Differs from `other` in host, port or both.
    fn authority_other_than(tc: &TestCase, other: &Authority) -> Authority {
        let host_offset =
            tc.draw_silent(integers::<usize>().max_value(HOSTS.len() - 1));
        let host = (other.host + host_offset) % HOSTS.len();
        let port = tc.draw_silent(optional(ports()));
        if host != other.host || port != other.port {
            return Authority { host, port };
        }
        let port = match port {
            Some(_) => None,
            None => Some(tc.draw_silent(ports())),
        };
        Authority { host, port }
    }

    #[hegel::composite]
    fn schemes(tc: &TestCase) -> &'static str {
        tc.draw_silent(sampled_from(SCHEMES.as_slice())).0
    }

    #[hegel::composite]
    fn origins(tc: &TestCase) -> String {
        format!(
            "{}://{}",
            tc.draw_silent(schemes()),
            tc.draw_silent(authorities())
        )
    }

    #[hegel::composite]
    fn fragments(tc: &TestCase) -> String {
        let fragment = tc.draw_silent(optional(
            text().alphabet("abcdef0123456789xyz=&/").max_size(12),
        ));
        fragment.map_or(String::new(), |fragment| format!("#{fragment}"))
    }

    /// `length` characters of `alphabet`, with one from each of `required`.
    fn token_with(
        tc: &TestCase,
        alphabet: &str,
        length: usize,
        required: &[&str],
    ) -> String {
        let free = length - required.len();
        let mut token: Vec<char> = tc
            .draw_silent(
                text().alphabet(alphabet).min_size(free).max_size(free),
            )
            .chars()
            .collect();
        for alphabet in required {
            let character = tc.draw_silent(sampled_from(
                alphabet.chars().collect::<Vec<char>>(),
            ));
            let position =
                tc.draw_silent(integers::<usize>().max_value(token.len()));
            token.insert(position, character);
        }
        token.into_iter().collect()
    }

    /// A build hash, per `is_build_hash`.
    #[hegel::composite]
    fn hashes(tc: &TestCase) -> String {
        let length =
            tc.draw_silent(integers::<usize>().min_value(8).max_value(40));
        token_with(tc, HEX, length, &[HEX_LETTERS, DIGITS])
    }

    /// A non-empty token that is not a build hash: words, digits, hex letters
    /// only, short hex, or non-hex alphanumerics (`BX3kd9aZ`).
    #[hegel::composite]
    fn words(tc: &TestCase) -> String {
        let length = |min: usize, max: usize| {
            tc.draw_silent(integers::<usize>().min_value(min).max_value(max))
        };
        match tc.draw_silent(integers::<u8>().max_value(7)) {
            0 => token_with(tc, LOWER, length(1, 40), &[]),
            1 => token_with(tc, UPPER, length(1, 40), &[]),
            2 => token_with(tc, LETTERS, length(2, 40), &[LOWER, UPPER]),
            3 => token_with(tc, DIGITS, length(1, 40), &[]),
            4 => token_with(tc, HEX_LETTERS, length(1, 40), &[]),
            5 => token_with(tc, HEX, length(2, 7), &[HEX_LETTERS, DIGITS]),
            6 => token_with(tc, ALPHANUMERIC, 8, &[NOT_HEX]),
            _ => token_with(tc, ALPHANUMERIC, length(1, 40), &[NOT_HEX]),
        }
    }

    #[hegel::composite]
    fn delimiters(tc: &TestCase) -> char {
        tc.draw_silent(sampled_from(TOKEN_DELIMITERS.as_slice()))
    }

    /// A name followed by delimited tokens. `None` stands for a build hash;
    /// an empty word gives two delimiters in a row.
    #[derive(Clone, Debug, hegel::PrettyPrintable)]
    struct Name {
        name: String,
        tokens: Vec<(char, Option<String>)>,
    }

    #[hegel::composite]
    fn tokens(tc: &TestCase) -> Vec<(char, Option<String>)> {
        let count = tc.draw_silent(integers::<usize>().max_value(4));
        (0..count)
            .map(|_| {
                let token = match tc.draw_silent(integers::<u8>().max_value(4))
                {
                    0 => Some(String::new()),
                    1 | 2 => Some(tc.draw_silent(words())),
                    _ => None,
                };
                (tc.draw_silent(delimiters()), token)
            })
            .collect()
    }

    /// Like `main.<hash>.chunk.js`.
    #[hegel::composite]
    fn files(tc: &TestCase) -> Name {
        Name {
            name: tc.draw_silent(words()),
            tokens: tc.draw_silent(tokens()),
        }
    }

    /// Like `v=<hash>`.
    #[hegel::composite]
    fn parameters(tc: &TestCase) -> Name {
        let value = match tc.draw_silent(booleans()) {
            true => Some(tc.draw_silent(words())),
            false => None,
        };
        let mut tokens = vec![('=', value)];
        tokens.extend(tc.draw_silent(self::tokens()));
        Name {
            name: tc.draw_silent(words()),
            tokens,
        }
    }

    impl Name {
        fn fill(&self, hash: &mut impl FnMut() -> String) -> String {
            let mut filled = self.name.clone();
            for (delimiter, token) in &self.tokens {
                filled.push(*delimiter);
                match token {
                    Some(word) => filled.push_str(word),
                    None => filled.push_str(&hash()),
                }
            }
            filled
        }

        fn words_only(mut self, tc: &TestCase) -> Self {
            for (_, token) in &mut self.tokens {
                token.get_or_insert_with(|| tc.draw_silent(words()));
            }
            self
        }
    }

    /// A directory path of words and hashes, `/` on both ends.
    #[hegel::composite]
    fn directories(tc: &TestCase) -> String {
        let segments = tc.draw_silent(integers::<usize>().max_value(3));
        let mut directory = String::from("/");
        for _ in 0..segments {
            let hash = &mut || tc.draw_silent(hashes());
            // A non-empty first token keeps the segment from being `.` or `..`.
            directory.push_str(&hash());
            directory.push_str(&tc.draw_silent(files()).fill(hash));
            directory.push('/');
        }
        directory
    }

    #[derive(Clone, Debug, hegel::PrettyPrintable)]
    struct Shape {
        directory: String,
        file: Name,
        query: Option<Vec<Name>>,
    }

    #[hegel::composite]
    fn shapes(tc: &TestCase) -> Shape {
        Shape {
            directory: tc.draw_silent(directories()),
            file: tc.draw_silent(files()),
            query: tc.draw_silent(optional(
                vecs(parameters()).min_size(1).max_size(3),
            )),
        }
    }

    impl Shape {
        /// Path and query, with each hash place filled from `hash`.
        fn fill(&self, mut hash: impl FnMut() -> String) -> String {
            let mut filled =
                format!("{}{}", self.directory, self.file.fill(&mut hash));
            if let Some(query) = &self.query {
                let parameters: Vec<String> = query
                    .iter()
                    .map(|parameter| parameter.fill(&mut hash))
                    .collect();
                filled.push('?');
                filled.push_str(&parameters.join("&"));
            }
            filled
        }

        fn words_only(self, tc: &TestCase) -> Self {
            Shape {
                directory: self.directory,
                file: self.file.words_only(tc),
                query: self.query.map(|query| {
                    query
                        .into_iter()
                        .map(|parameter| parameter.words_only(tc))
                        .collect()
                }),
            }
        }
    }

    fn url(origin: &str, path: &str) -> Url {
        Url::parse(&format!("{origin}{path}")).unwrap()
    }

    /// The ID of a same-origin script.
    fn source_id(origin: &str, path: &str) -> SourceId {
        SourceId::from_url(&url(origin, path), &url(origin, "/"))
    }

    #[hegel::test]
    fn from_url_ignores_fragment_and_the_origin_under_test(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let first = format!("{path}{}", tc.draw(fragments()));
        let second = format!("{path}{}", tc.draw(fragments()));
        assert_eq!(
            source_id(&tc.draw(origins()), &first),
            source_id(&tc.draw(origins()), &second)
        );
    }

    #[hegel::test]
    fn from_url_puts_the_host_of_another_origin_first(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let scheme = tc.draw(schemes());
        let tested = tc.draw(authorities());
        let other = authority_other_than(&tc, &tested);
        tc.note(&format!("other: {other}"));
        let tested = format!("{scheme}://{tested}");

        let script = url(&format!("{}://{other}", tc.draw(schemes())), &path);
        assert_eq!(
            *SourceId::from_url(&script, &url(&tested, "/")).0,
            format!("{other}{}", source_id(&tested, &path).0)
        );
    }

    #[hegel::test]
    fn from_url_puts_the_host_of_another_scheme_first(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let authority = tc.draw(authorities());
        let (tested, other) = match tc.draw(booleans()) {
            true => ("http", "https"),
            false => ("https", "http"),
        };
        let tested = format!("{tested}://{authority}");

        let script = url(&format!("{other}://{authority}"), &path);
        assert_eq!(
            *SourceId::from_url(&script, &url(&tested, "/")).0,
            format!("{authority}{}", source_id(&tested, &path).0)
        );
    }

    #[hegel::test]
    fn from_url_keeps_origins_apart(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let scheme = tc.draw(schemes());
        let first = tc.draw(authorities());
        let second = authority_other_than(&tc, &first);
        tc.note(&format!("second: {second}"));
        // Either of the two, or a third.
        let tested = tc.draw(authorities());
        let tested = url(&format!("{scheme}://{tested}"), "/");

        let key = |authority: &Authority| {
            let script = url(&format!("{scheme}://{authority}"), &path);
            SourceId::from_url(&script, &tested)
        };
        assert_ne!(key(&first), key(&second));
    }

    /// The URL under test is a page: `http://localhost:3000/app/?tab=1`.
    #[hegel::test]
    fn from_url_takes_the_origin_of_the_url_under_test_only(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let script = url(&tc.draw(origins()), &path);
        let scheme = tc.draw(schemes());
        let authority = tc.draw(authorities());
        let page = shape.fill(|| tc.draw(hashes()));
        let page = format!("{page}{}", tc.draw(fragments()));

        assert_eq!(
            SourceId::from_url(
                &script,
                &url(&format!("{scheme}://user@{authority}"), &page)
            ),
            SourceId::from_url(
                &script,
                &url(&format!("{scheme}://{authority}"), "/")
            )
        );
    }

    #[hegel::test]
    fn from_url_takes_a_default_port_for_none(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let (scheme, port) = tc.draw(sampled_from(SCHEMES.as_slice()));
        let authority = Authority {
            host: tc.draw(integers::<usize>().max_value(HOSTS.len() - 1)),
            port: None,
        };
        let without = format!("{scheme}://{authority}");
        let with = format!("{scheme}://{authority}:{port}");
        let key = source_id(&without, &path);

        assert_eq!(
            SourceId::from_url(&url(&with, &path), &url(&without, "/")),
            key
        );
        assert_eq!(
            SourceId::from_url(&url(&without, &path), &url(&with, "/")),
            key
        );

        // Nor in another origin's key.
        let tested = authority_other_than(&tc, &authority);
        let tested = url(&format!("{scheme}://{tested}"), "/");
        assert_eq!(
            SourceId::from_url(&url(&with, &path), &tested),
            SourceId::from_url(&url(&without, &path), &tested)
        );
    }

    #[hegel::test]
    fn from_url_keeps_the_default_port_of_another_scheme(tc: TestCase) {
        let shape = tc.draw(shapes());
        let path = shape.fill(|| tc.draw(hashes()));
        let (scheme, port) = match tc.draw(booleans()) {
            true => ("http", 443),
            false => ("https", 80),
        };
        let host = tc.draw(sampled_from(HOSTS.as_slice()));
        let tested = format!("{scheme}://{host}");

        let script = url(&format!("{scheme}://{host}:{port}"), &path);
        assert_eq!(
            *SourceId::from_url(&script, &url(&tested, "/")).0,
            format!("{host}:{port}{}", source_id(&tested, &path).0)
        );
    }

    #[hegel::test]
    fn from_url_masks_hashes_after_a_name(tc: TestCase) {
        let shape = tc.draw(shapes());
        let origin = tc.draw(origins());
        let first = shape.fill(|| tc.draw(hashes()));
        let second = shape.fill(|| tc.draw(hashes()));
        let masked = shape.fill(|| "*".to_string());

        let key = source_id(&origin, &first);
        assert_eq!(&*key.0, masked);
        assert_eq!(key, source_id(&origin, &second));
    }

    #[hegel::test]
    fn from_url_keeps_what_is_not_a_hash(tc: TestCase) {
        let shape = tc.draw(shapes()).words_only(&tc);
        let path = shape.fill(|| unreachable!());
        assert_eq!(&*source_id(&tc.draw(origins()), &path).0, path);
    }

    /// Hashes included: `0123456789abcdef.js` and `1123456789abcdef.js`
    /// differ.
    #[hegel::test]
    fn from_url_keeps_files_of_different_names_apart(tc: TestCase) {
        let name = |tc: &TestCase| match tc.draw(booleans()) {
            true => tc.draw(hashes()),
            false => tc.draw(words()),
        };
        let (first, second) = (name(&tc), name(&tc));
        tc.assume(first != second);
        let directory = tc.draw(directories());
        let origin = tc.draw(origins());
        let path = |name: String| {
            let file = Name {
                name,
                tokens: tc.draw(tokens()),
            };
            format!("{directory}{}", file.fill(&mut || tc.draw(hashes())))
        };
        let (first, second) = (path(first), path(second));
        assert_ne!(source_id(&origin, &first), source_id(&origin, &second));
    }

    #[hegel::test]
    fn from_url_keeps_a_hash_that_is_the_name(tc: TestCase) {
        let leading: String = tc
            .draw(vecs(delimiters()).max_size(2))
            .into_iter()
            .collect();
        let file = Name {
            name: format!("{leading}{}", tc.draw(hashes())),
            tokens: tc.draw(tokens()),
        }
        .words_only(&tc);
        let mut path = format!(
            "{}{}",
            tc.draw(directories()),
            file.fill(&mut || unreachable!())
        );
        // A query parameter without a name.
        if let Some(delimiter) =
            tc.draw(optional(sampled_from(["", "="].as_slice())))
        {
            path.push_str(&format!("?{delimiter}{}", tc.draw(hashes())));
        }
        assert_eq!(&*source_id(&tc.draw(origins()), &path).0, path);
    }

    /// An origin and URLs on it and on another origin, with hashes from a
    /// small pool so URLs repeat or differ only in hash or origin.
    fn loads(tc: &TestCase) -> (Url, Vec<Url>) {
        let scheme = tc.draw(schemes());
        let tested = tc.draw(authorities());
        let other = authority_other_than(tc, &tested);
        let origins = [tested, other].map(|authority| {
            tc.note(&format!("origin: {scheme}://{authority}"));
            format!("{scheme}://{authority}")
        });
        let shapes = tc.draw(vecs(shapes()).min_size(1).max_size(3));
        let pool = tc.draw(vecs(hashes()).min_size(1).max_size(3));
        let count = tc.draw(integers::<usize>().min_value(1).max_value(8));
        let urls = (0..count)
            .map(|_| {
                let origin = tc.draw_silent(sampled_from(origins.as_slice()));
                let shape = tc.draw_silent(sampled_from(shapes.as_slice()));
                let path =
                    shape.fill(|| tc.draw_silent(sampled_from(pool.clone())));
                url(&origin, &path)
            })
            .collect();
        (url(&origins[0], "/"), urls)
    }

    #[hegel::test]
    fn registry_gives_each_url_one_id_of_its_own(tc: TestCase) {
        let (origin, loads) = loads(&tc);
        let registry = BlockRegistry::default();
        let mut ids: BTreeMap<Url, SourceId> = BTreeMap::new();
        let mut urls: BTreeMap<Arc<str>, Url> = BTreeMap::new();
        for url in loads {
            let id = registry.source_id(&url, &origin);
            tc.note(&format!("{url} -> {}", id.0));
            assert_eq!(urls.entry(id.0.clone()).or_insert(url.clone()), &url);
            assert_eq!(ids.entry(url).or_insert(id.clone()), &id);
        }
    }

    #[hegel::test]
    fn registry_masks_the_first_url_of_each_masked_id(tc: TestCase) {
        let (origin, loads) = loads(&tc);
        let registry = BlockRegistry::default();
        let mut seen: Vec<SourceId> = Vec::new();
        for url in loads {
            let masked = SourceId::from_url(&url, &origin);
            if !seen.contains(&masked) {
                assert_eq!(registry.source_id(&url, &origin), masked);
                seen.push(masked);
            } else {
                registry.source_id(&url, &origin);
            }
        }
    }

    #[hegel::test]
    fn inline_ids_are_not_url_ids(tc: TestCase) {
        let shape = tc.draw(shapes());
        let origin = tc.draw(origins());
        let page = shape.fill(|| tc.draw(hashes()));
        let index = tc.draw(integers::<usize>());
        let inline = source_id(&origin, &page).inline(index);
        // A URL spelled like the inline ID.
        assert_ne!(source_id(&origin, &inline.0), inline);
    }
}

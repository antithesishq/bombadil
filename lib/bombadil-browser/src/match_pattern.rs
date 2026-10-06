//! Exploration boundary patterns, using Chrome extension match pattern syntax
//! (<https://developer.chrome.com/docs/extensions/develop/concepts/match-patterns>).
//!
//! Only `http`, `https`, and `file` patterns are supported. As in Chrome, the
//! pattern is matched against path and query string, not its fragment. Only `*`
//! wildcards are supported.
//!
//! Also supports shorthands for ease of use.

use std::fmt::{Display, Formatter, Result as FmtResult};

use url::Url;

const ALL_URLS: &str = "<all_urls>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchPattern {
    scheme: SchemePattern,
    host: HostPattern,
    /// `None` matches any port.
    port: Option<u16>,
    /// Path pattern, starting with `/`.
    path: PathPattern,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchemePattern {
    Http,
    Https,
    /// `*`
    HttpOrHttps,
    File,
    /// `<all_urls>`
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostPattern {
    /// `*`
    Any,
    /// `*.example.com`
    Subdomains(String),
    Exact(String),
    /// `file://` patterns have no host.
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PathPattern {
    /// Literals between `*` wildcards. These strings may be empty, meaning
    /// they consume nothing of the text when matching.
    Wildcards(Vec<String>),
    /// Exact match on path only (not query or fragment).
    ExactPath(String),
}

impl MatchPattern {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("allow-url must not be empty".to_string());
        }
        if raw == ALL_URLS {
            return Ok(MatchPattern {
                scheme: SchemePattern::Any,
                host: HostPattern::Any,
                port: None,
                path: PathPattern::all_paths(),
            });
        }

        let split = raw.split_once("://");
        let (scheme, rest) = match split {
            Some((scheme, rest)) => (parse_scheme(scheme, raw)?, rest),
            None => (SchemePattern::HttpOrHttps, raw),
        };

        if scheme == SchemePattern::File {
            if !rest.starts_with('/') {
                return Err(invalid(
                    raw,
                    "file patterns have no host, as in file:///path/*",
                ));
            }
            return Ok(MatchPattern {
                scheme,
                host: HostPattern::Empty,
                port: None,
                path: canonical_path(rest),
            });
        }

        let (authority, path) = match rest.find('/') {
            Some(index) => rest.split_at(index),
            None => (rest, "/*"),
        };
        let (host, port) =
            parse_authority(authority).map_err(|error| invalid(raw, &error))?;
        if split.is_none() && host == HostPattern::Any {
            return Err(invalid(
                raw,
                "to allow any host, use <all_urls> or *://*/*",
            ));
        }

        Ok(MatchPattern {
            scheme,
            host,
            port,
            path: canonical_path(path),
        })
    }

    /// The match pattern used when no explicit patterns are given, based on
    /// the origin URL.
    pub fn from_origin(origin: &Url) -> Result<Self, String> {
        match origin.scheme() {
            "file" => Ok(MatchPattern {
                scheme: SchemePattern::File,
                host: HostPattern::Empty,
                port: None,
                path: PathPattern::ExactPath(origin.path().to_string()),
            }),
            scheme @ ("http" | "https") => Ok(MatchPattern {
                scheme: if scheme == "https" {
                    SchemePattern::Https
                } else {
                    SchemePattern::Http
                },
                host: HostPattern::Exact(
                    origin
                        .host_str()
                        .ok_or(format!("{scheme} origin is missing host"))?
                        .to_string(),
                ),
                port: origin.port_or_known_default(),
                path: PathPattern::all_paths(),
            }),
            other => Err(format!("invalid scheme for origin URL: {other:?}")),
        }
    }

    pub fn matches(&self, url: &Url) -> bool {
        self.scheme.matches(url.scheme())
            && self.host.matches(url.host_str())
            && self
                .port
                .is_none_or(|port| url.port_or_known_default() == Some(port))
            && self.path.matches(url)
    }
}

impl SchemePattern {
    fn matches(self, scheme: &str) -> bool {
        match self {
            SchemePattern::Http => scheme == "http",
            SchemePattern::Https => scheme == "https",
            SchemePattern::HttpOrHttps => matches!(scheme, "http" | "https"),
            SchemePattern::File => scheme == "file",
            SchemePattern::Any => matches!(scheme, "http" | "https" | "file"),
        }
    }
}

impl HostPattern {
    fn matches(&self, host: Option<&str>) -> bool {
        match self {
            HostPattern::Any => true,
            HostPattern::Subdomains(domain) => host.is_some_and(|host| {
                host == domain || host.ends_with(&format!(".{domain}"))
            }),
            HostPattern::Exact(expected) => host == Some(expected.as_str()),
            HostPattern::Empty => host.is_none_or(str::is_empty),
        }
    }
}

impl PathPattern {
    fn parse(path: &str) -> Self {
        PathPattern::Wildcards(path.split('*').map(str::to_string).collect())
    }

    fn all_paths() -> Self {
        PathPattern::parse("/*")
    }

    fn matches(&self, url: &Url) -> bool {
        match self {
            PathPattern::Wildcards(literals) => {
                let text = &path_and_query(url);
                let (first, rest) = literals
                    .split_first()
                    .expect("a path pattern must have at least one literal");
                // Match the first literal.
                let Some(text) = text.strip_prefix(first.as_str()) else {
                    return false;
                };
                let Some((last, middle)) = rest.split_last() else {
                    return text.is_empty();
                };
                // Greedily match middle patterns in order.
                let mut text = text;
                for literal in middle {
                    let Some(index) = text.find(literal.as_str()) else {
                        return false;
                    };
                    text = &text[index + literal.len()..];
                }
                // Match last literal.
                text.ends_with(last.as_str())
            }
            PathPattern::ExactPath(path) => url.path() == path,
        }
    }
}

impl Display for PathPattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            PathPattern::Wildcards(literals) => {
                write!(f, "{}", literals.join("*"))
            }
            PathPattern::ExactPath(path) => write!(f, "{path}"),
        }
    }
}

impl Display for MatchPattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let scheme = match self.scheme {
            SchemePattern::Any => return write!(f, "{ALL_URLS}"),
            SchemePattern::Http => "http",
            SchemePattern::Https => "https",
            SchemePattern::HttpOrHttps => "*",
            SchemePattern::File => "file",
        };
        write!(f, "{scheme}://")?;
        match &self.host {
            HostPattern::Any => write!(f, "*")?,
            HostPattern::Subdomains(domain) => write!(f, "*.{domain}")?,
            HostPattern::Exact(host) => write!(f, "{host}")?,
            HostPattern::Empty => {}
        }
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        write!(f, "{}", self.path)
    }
}

fn invalid(raw: &str, error: &str) -> String {
    format!("invalid match pattern {raw:?}: {error}")
}

fn parse_scheme(scheme: &str, raw: &str) -> Result<SchemePattern, String> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Ok(SchemePattern::Http),
        "https" => Ok(SchemePattern::Https),
        "*" => Ok(SchemePattern::HttpOrHttps),
        "file" => Ok(SchemePattern::File),
        other => Err(invalid(raw, &format!("unsupported scheme {other:?}"))),
    }
}

// A missing path or a `/` in a pattern matches all paths.
fn canonical_path(path: &str) -> PathPattern {
    if path == "/" {
        PathPattern::all_paths()
    } else {
        PathPattern::parse(path)
    }
}

fn parse_authority(
    authority: &str,
) -> Result<(HostPattern, Option<u16>), String> {
    // A colon inside brackets belongs to an IPv6 literal, not a port.
    let (host, port) = match authority.rfind(':') {
        Some(index) if !authority[index..].contains(']') => {
            (&authority[..index], Some(&authority[index + 1..]))
        }
        _ => (authority, None),
    };
    let port = match port {
        None | Some("*") => None,
        Some(port) => {
            Some(port.parse().map_err(|_| format!("invalid port {port:?}"))?)
        }
    };
    Ok((parse_host(host)?, port))
}

fn parse_host(host: &str) -> Result<HostPattern, String> {
    // Use the url crate to canonicalize the pattern's host part.
    fn parse_host_as_url(input: &str) -> Result<String, String> {
        url::Host::parse(input)
            .map(|host| host.to_string())
            .map_err(|error| format!("invalid hostname: {error}"))
    }

    if host == "*" {
        return Ok(HostPattern::Any);
    }
    if let Some(domain) = host.strip_prefix("*.") {
        if domain.is_empty() || domain.contains('*') {
            return Err(format!("invalid host {host:?}"));
        }
        return Ok(HostPattern::Subdomains(parse_host_as_url(domain)?));
    }
    if host.is_empty() || host.contains('*') {
        return Err(format!(
            "invalid host {host:?}: a host wildcard must be the whole host, \
             or a leading \"*.\""
        ));
    }
    Ok(HostPattern::Exact(parse_host_as_url(host)?))
}

fn path_and_query(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use hegel::{
        Generator, TestCase,
        generators::{
            booleans, integers, one_of, optional, sampled_from, text, vecs,
        },
    };

    fn url(string: &str) -> Url {
        Url::parse(string).unwrap()
    }

    fn pattern(raw: &str) -> MatchPattern {
        MatchPattern::parse(raw).expect(raw)
    }

    fn allows(raw: &str, url_string: &str) -> bool {
        pattern(raw).matches(&url(url_string))
    }

    // The example patterns from the Chrome documentation.
    #[test]
    fn test_spec_examples_match() {
        assert!(allows("https://*/*", "https://example.com/foo/bar.html"));
        assert!(!allows("https://*/*", "http://example.com/"));

        assert!(allows("https://*/foo*", "https://example.com/foo/bar.html"));
        assert!(allows("https://*/foo*", "https://www.google.com/foo"));
        assert!(!allows("https://*/foo*", "https://example.com/bar"));

        let google = "https://*.google.com/foo*bar";
        assert!(allows(google, "https://www.google.com/foo/baz/bar"));
        assert!(allows(google, "https://docs.google.com/foobar"));
        assert!(!allows(google, "https://www.google.com/foo/baz"));
        assert!(!allows(google, "https://www.googleX.com/foobar"));

        assert!(allows("file:///foo*", "file:///foo/bar.html"));
        assert!(allows("file:///foo*", "file:///foo"));
        assert!(!allows("file:///foo*", "file:///bar"));

        assert!(allows("http://127.0.0.1/*", "http://127.0.0.1/"));
        assert!(allows(
            "http://127.0.0.1/*",
            "http://127.0.0.1/foo/bar.html"
        ));

        assert!(allows("http://localhost/*", "http://localhost:8080/foo"));
        assert!(allows("http://localhost/*", "http://localhost/foo"));

        assert!(allows(
            "*://mail.google.com/*",
            "http://mail.google.com/foo"
        ));
        assert!(allows("*://mail.google.com/*", "https://mail.google.com/"));
        assert!(!allows("*://mail.google.com/*", "ftp://mail.google.com/"));

        assert!(allows("https://*/", "https://example.com/foo/bar.html"));
        assert!(allows(
            "*://mail.google.com/",
            "https://mail.google.com/foo"
        ));

        assert!(allows(ALL_URLS, "http://example.com/x"));
        assert!(allows(ALL_URLS, "https://example.com/x"));
        assert!(allows(ALL_URLS, "file:///tmp/index.html"));
    }

    #[test]
    fn test_explicit_ports_match_the_effective_port() {
        assert!(allows("http://example.com:80/*", "http://example.com/"));
        assert!(allows("https://example.com:443/*", "https://example.com/"));
        assert!(!allows("http://example.com:8080/*", "http://example.com/"));
        assert!(allows("*://example.com:*/*", "http://example.com:9090/"));
    }

    #[test]
    fn test_from_origin_uses_effective_port() {
        let implicit =
            MatchPattern::from_origin(&url("https://example.com/")).unwrap();
        assert_eq!(
            implicit,
            MatchPattern::parse("https://example.com:443/*").unwrap()
        );
        assert!(implicit.matches(&url("https://example.com/app")));
        assert!(!implicit.matches(&url("https://example.com:8443/app")));
        assert!(!implicit.matches(&url("http://example.com/app")));

        let explicit =
            MatchPattern::from_origin(&url("http://localhost:3000/")).unwrap();
        assert!(explicit.matches(&url("http://localhost:3000/app")));
        assert!(!explicit.matches(&url("http://localhost/app")));
    }

    #[test]
    fn test_patterns_print_canonically() {
        assert_eq!(pattern("example.com").to_string(), "*://example.com/*");
        assert_eq!(pattern("file:///").to_string(), "file:///*");
        assert_eq!(pattern(ALL_URLS).to_string(), ALL_URLS);
    }

    #[test]
    fn test_unsupported_schemes_are_out_of_bounds() {
        for url_string in ["about:blank", "about:srcdoc", "data:text/html,x"] {
            assert!(!allows(ALL_URLS, url_string), "{url_string}");
            assert!(!allows("*://*/*", url_string), "{url_string}");
        }
    }

    #[test]
    fn test_parse_rejects_malformed_patterns() {
        for raw in [
            "",
            "*",
            "*/*",
            "https://*foo/bar",
            "https://foo.*.bar/baz",
            "https://*./baz",
            "ftp://example.com/*",
            "file://localhost/foo",
            "https://example.com:abc/*",
            "//example.com/foo",
        ] {
            assert!(
                MatchPattern::parse(raw).is_err(),
                "{raw:?} should not parse"
            );
        }
    }

    #[test]
    fn test_wildcards_at_the_edges() {
        assert!(allows(
            "https://example.com/*foo",
            "https://example.com/a/foo"
        ));
        assert!(!allows(
            "https://example.com/*foo",
            "https://example.com/foot"
        ));

        assert!(allows(
            "https://example.com/foo*",
            "https://example.com/foot"
        ));
        assert!(!allows(
            "https://example.com/foo*",
            "https://example.com/afoo"
        ));

        assert!(allows(
            "https://example.com/*foo*",
            "https://example.com/a/foot"
        ));

        assert!(allows("https://example.com/foo", "https://example.com/foo"));
        assert!(!allows(
            "https://example.com/foo",
            "https://example.com/foot"
        ));
    }

    #[test]
    fn test_wildcards_do_not_reuse_an_overlapping_end() {
        assert!(!allows("https://example.com/a*a", "https://example.com/a"));
        assert!(allows("https://example.com/a*a", "https://example.com/aa"));
        assert!(allows("https://example.com/a*a", "https://example.com/aba"));
    }

    #[test]
    fn test_a_file_origin_keeps_its_own_query_string_in_bounds() {
        let origin = url("file:///app/index.html?route=home");
        let pattern = MatchPattern::from_origin(&origin).unwrap();
        assert!(pattern.matches(&origin));
        assert!(pattern.matches(&url("file:///app/index.html")));
        assert!(pattern.matches(&url("file:///app/index.html?route=away")));
        assert!(!pattern.matches(&url("file:///app/index.html.bak")));
        assert!(!pattern.matches(&url("file:///app/other.html")));
    }

    #[hegel::composite]
    fn hostnames(tc: &TestCase) -> String {
        let label = text().alphabet("ab12-點看").min_size(1).max_size(4);
        tc.draw_silent(vecs(label).min_size(2).max_size(3).filter(|labels| {
            let edges_are_alphanumeric = |label: &String| {
                !label.starts_with('-') && !label.ends_with('-')
            };
            // A trailing all-numeric label makes the url crate read the host
            // as a malformed IPv4 address.
            labels.iter().all(edges_are_alphanumeric)
                && labels.last().is_some_and(|last| {
                    last.starts_with(|first: char| first.is_ascii_alphabetic())
                })
        }))
        .join(".")
    }

    #[hegel::composite]
    fn path_segments(tc: &TestCase) -> Vec<String> {
        let segment = text().alphabet("abc[]*").min_size(1).max_size(3);
        tc.draw_silent(vecs(segment).max_size(3))
    }

    #[hegel::composite]
    fn ports(tc: &TestCase) -> u16 {
        tc.draw_silent(integers::<u16>().min_value(1024).max_value(65535))
    }

    #[hegel::composite]
    fn http_urls(tc: &TestCase) -> Url {
        let scheme = tc.draw_silent(sampled_from(["http", "https"].as_slice()));
        let host = tc.draw(hostnames());
        let port = tc.draw(optional(ports()));
        let path = tc.draw(path_segments()).join("/");
        let query = tc.draw(optional(path_segments()));
        let fragment = tc.draw(optional(path_segments()));
        let authority = match port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        let mut raw = format!("{scheme}://{authority}/{path}");
        if let Some(query) = query {
            raw.push_str(&format!("?q={}", query.join("+")));
        }
        if let Some(fragment) = fragment {
            raw.push_str(&format!("#{}", fragment.join("-")));
        }
        Url::parse(&raw).unwrap_or_else(|_| tc.reject())
    }

    #[hegel::composite]
    fn file_urls(tc: &TestCase) -> Url {
        let mut segments = tc.draw_silent(path_segments());
        if segments.is_empty() {
            segments.push("index.html".to_string());
        }
        let mut url = url(&format!("file:///{}", segments.join("/")));

        let query = tc.draw(optional(path_segments()));
        if let Some(query) = query {
            url.set_query(Some(&format!("q={}", query.join("+"))))
        }
        url
    }

    #[hegel::composite]
    fn all_urls(tc: &TestCase) -> Url {
        tc.draw(
            one_of([http_urls().boxed(), file_urls().boxed()]).print_as_debug(),
        )
    }

    #[hegel::composite]
    fn match_patterns(tc: &TestCase) -> MatchPattern {
        let raw = tc.draw_silent(pattern_strings());
        MatchPattern::parse(&raw).expect("generated pattern should parse")
    }

    #[hegel::composite]
    fn pattern_strings(tc: &TestCase) -> String {
        let schemes = ["http", "https", "*", "file", ALL_URLS];
        let scheme = tc.draw_silent(sampled_from(schemes.as_slice()));
        if scheme == ALL_URLS {
            return ALL_URLS.to_string();
        }

        let mut path =
            format!("/{}", tc.draw_silent(path_segments()).join("/"));
        if tc.draw_silent(booleans()) {
            path.push('*');
        }
        if scheme == "file" {
            return format!("file://{path}");
        }

        let host = tc.draw_silent(hostnames());
        let host = match tc
            .draw_silent(sampled_from(["any", "sub", "exact"].as_slice()))
        {
            "any" => "*".to_string(),
            "sub" => format!("*.{host}"),
            _ => host,
        };
        let port = match tc.draw_silent(optional(ports())) {
            Some(port) => format!(":{port}"),
            None => String::new(),
        };
        if tc.draw_silent(booleans()) {
            format!("{scheme}://{host}{port}")
        } else {
            format!("{scheme}://{host}{port}{path}")
        }
    }

    #[hegel::test]
    fn test_origin_patterns_match_their_origin(tc: TestCase) {
        let origin = tc.draw(all_urls().print_as_debug());
        let from_origin = MatchPattern::from_origin(&origin).unwrap();
        tc.note(&format!("pattern: {from_origin}"));
        assert!(from_origin.matches(&origin));
    }

    // A `file://` origin's pattern does not round trip.
    #[hegel::test]
    fn test_http_origin_patterns_round_trip(tc: TestCase) {
        let origin = tc.draw(http_urls().print_as_debug());
        let from_origin = MatchPattern::from_origin(&origin).unwrap();
        assert_eq!(pattern(&format!("{from_origin}")), from_origin);
    }

    #[hegel::test]
    fn test_roundtrip_display_parse(tc: TestCase) {
        let pattern = tc.draw(match_patterns().print_as_debug());
        assert_eq!(
            MatchPattern::parse(&format!("{pattern}")).unwrap(),
            pattern
        );
    }

    #[hegel::test]
    fn test_pattern_built_from_url_matches_it(tc: TestCase) {
        let url = tc.draw(http_urls().print_as_debug());
        let port = url.port().map_or_else(String::new, |p| format!(":{p}"));
        let raw = format!(
            "{}://{}{port}{}",
            url.scheme(),
            url.host_str().unwrap(),
            path_and_query(&url)
        );
        assert!(
            pattern(&raw).matches(&url),
            "url {url} should be matched by {raw}"
        );
    }

    #[hegel::test]
    fn test_all_urls_matches_every_supported_url(tc: TestCase) {
        let url = tc.draw(all_urls().print_as_debug());
        assert!(pattern(ALL_URLS).matches(&url));
    }

    #[hegel::test]
    fn test_scheme_patterns_are_honoured(tc: TestCase) {
        let url = tc.draw(http_urls().print_as_debug());
        let host = url.host_str().unwrap();
        let mut other = url.clone();
        let other_scheme = match url.scheme() {
            "http" => "https",
            _ => "http",
        };
        other
            .set_scheme(other_scheme)
            .unwrap_or_else(|()| tc.reject());

        let pattern_either_scheme = pattern(&format!("*://{host}/*"));
        assert!(pattern_either_scheme.matches(&url));
        assert!(pattern_either_scheme.matches(&other));

        let pattern_same_scheme =
            pattern(&format!("{}://{host}/*", url.scheme()));
        assert!(pattern_same_scheme.matches(&url));
        assert!(!pattern_same_scheme.matches(&other));
    }

    #[hegel::test]
    fn test_default_boundary_covers_the_origin_host(tc: TestCase) {
        let origin = tc.draw(http_urls().print_as_debug());
        let segments = tc.draw(path_segments());
        let pattern = MatchPattern::from_origin(&origin).unwrap();

        assert!(pattern.matches(&origin));

        let mut elsewhere = origin.clone();
        elsewhere.set_path(&format!("/{}", segments.join("/")));
        elsewhere.set_query(Some("q=1"));
        assert!(pattern.matches(&elsewhere));

        if let url::Host::Domain(domain) = origin.host().unwrap() {
            let with_subdomain = url(&format!("https://sub.{domain}/"));
            assert!(!pattern.matches(&with_subdomain));
        }
    }

    #[hegel::test]
    fn test_file_patterns_match_paths(tc: TestCase) {
        let file_url = tc.draw(file_urls().print_as_debug());
        let suffix = tc.draw(text().alphabet("abc").min_size(1).max_size(3));
        let only_this_file = MatchPattern::from_origin(&file_url).unwrap();
        tc.note(&format!("pattern: {only_this_file}"));
        assert!(only_this_file.matches(&file_url));
        let mut with_suffix = file_url.clone();
        with_suffix.path_segments_mut().unwrap().push(&suffix);
        assert!(!only_this_file.matches(&with_suffix));

        let directory =
            &file_url.path()[..=file_url.path().rfind('/').unwrap()];
        // Written by hand, and the syntax has no escape for a literal `*`.
        if directory.contains('*') {
            tc.reject();
        }
        let whole_directory = pattern(&format!("file://{directory}*"));
        assert!(whole_directory.matches(&file_url));
        assert!(whole_directory.matches(&url(&format!("{file_url}{suffix}"))));
    }

    #[hegel::test]
    fn test_host_wildcard_matches_subdomains(tc: TestCase) {
        let host = tc.draw(hostnames());
        let name = tc.draw(text().alphabet("abc").min_size(1).max_size(3));
        let wildcard = pattern(&format!("*://*.{host}/*"));

        assert!(wildcard.matches(&url(&format!("http://{host}/"))));
        let subdomain = url(&format!("http://{name}.{host}/"));
        assert!(wildcard.matches(&subdomain));

        let sibling = url(&format!("http://{name}{host}/"));
        assert!(!wildcard.matches(&sibling));
    }

    #[hegel::test]
    fn test_path_globs_match_suffixes(tc: TestCase) {
        let http_url = tc.draw(http_urls().print_as_debug());
        let suffix =
            tc.draw(text().alphabet("abc[]*?/").min_size(1).max_size(3));
        let host = http_url.host_str().unwrap();
        let target = path_and_query(&http_url);
        // Written by hand around `target`, so a `*` in it would read as a
        // wildcard.
        if target.contains('*') {
            tc.reject();
        }

        let prefixed = pattern(&format!("*://{host}{target}*"));
        let longer =
            url(&format!("{}://{host}{target}{suffix}", http_url.scheme()));
        assert!(prefixed.matches(&http_url));
        assert!(prefixed.matches(&longer));

        if target != "/" {
            let exact = pattern(&format!("*://{host}{target}"));
            assert!(exact.matches(&http_url));
            assert!(!exact.matches(&longer));
        }
    }

    #[hegel::test]
    fn test_port_patterns(tc: TestCase) {
        let host = tc.draw(hostnames());
        let port = tc.draw(ports());
        let other_port = tc.draw(ports());
        let http_url = url(&format!("http://{host}:{port}/app"));

        assert!(pattern(&format!("*://{host}/*")).matches(&http_url,));

        let pinned = pattern(&format!("*://{host}:{port}/*"));
        assert!(pinned.matches(&http_url));

        let elsewhere = url(&format!("http://{host}:{other_port}/app"));
        assert_eq!(pinned.matches(&elsewhere), other_port == port);
    }

    #[hegel::test]
    fn test_shorthand_spellings_agree(tc: TestCase) {
        let url = tc.draw(http_urls().print_as_debug());
        let port = url.port().map_or_else(String::new, |p| format!(":{p}"));
        let authority = format!("{}{port}", url.host_str().unwrap());
        let canonical = pattern(&format!("*://{authority}/*"));

        assert_eq!(pattern(&authority), canonical);
        assert_eq!(pattern(&format!("*://{authority}")), canonical);
        assert_eq!(pattern(&format!("*://{authority}/")), canonical);
        assert!(canonical.matches(&url));

        let host = url.host_str().unwrap();
        assert_eq!(
            pattern(&format!("*.{host}")),
            pattern(&format!("*://*.{host}/*"))
        );
        assert_eq!(
            pattern(&format!("{authority}/app*")),
            pattern(&format!("*://{authority}/app*"))
        );
        assert_eq!(
            pattern(&format!("https://{authority}")),
            pattern(&format!("https://{authority}/*"))
        );
    }
}

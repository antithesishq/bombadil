use std::fmt::{Display, Formatter, Result as FmtResult};
use std::path::PathBuf;

use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowUrl {
    /// The starting origin host (any path). Used as the default boundary when
    /// no explicit `--allow-url` entries are given.
    ExactHost { host: String, port: Option<u16> },
    /// Exact `file://` path (absolute). Used for inspect / local HTML origins
    /// and explicit `--allow-url file:///…` entries.
    ExactFile { path: PathBuf },
    /// A registrable domain and its subdomains (e.g. `example.com`, `.example.com`).
    Domain { domain: String },
    /// A URL prefix: scheme, host, port, and path prefix must match.
    UrlPrefix {
        scheme: String,
        host: String,
        port: Option<u16>,
        path_prefix: String,
    },
}

impl AllowUrl {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("allow-url must not be empty".into());
        }

        if raw.contains("://") {
            let url = Url::parse(raw)
                .map_err(|err| format!("invalid allow-url {raw:?}: {err}"))?;
            if url.scheme() == "file" {
                return Ok(AllowUrl::ExactFile {
                    path: absolute_file_path(&url)?,
                });
            }
            let host = url
                .host_str()
                .ok_or_else(|| format!("allow-url {raw:?} has no host"))?
                .to_string();
            return Ok(AllowUrl::UrlPrefix {
                scheme: url.scheme().to_string(),
                host,
                port: url.port(),
                path_prefix: normalize_path_prefix(url.path()),
            });
        }

        Ok(AllowUrl::Domain {
            domain: normalize_domain(raw),
        })
    }

    /// Implicit allow rule for the test origin.
    ///
    /// * Network origins → [`AllowUrl::ExactHost`] for that host (and port).
    /// * `file://` origins → [`AllowUrl::ExactFile`] for that absolute path only.
    /// * Other host-less origins → no implicit rule.
    pub fn from_origin(origin: &Url) -> Option<Self> {
        if origin.scheme() == "file" {
            return absolute_file_path(origin)
                .ok()
                .map(|path| AllowUrl::ExactFile { path });
        }
        origin.host_str().map(|host| AllowUrl::ExactHost {
            host: host.to_string(),
            port: origin.port(),
        })
    }

    /// Whether `uri` matches this rule. Rules are self-contained; they do not
    /// consult a separate origin for ports or other fields.
    fn matches(&self, uri: &Url) -> bool {
        match self {
            AllowUrl::ExactHost { host, port } => {
                uri.host_str() == Some(host.as_str())
                    && ports_match(uri.port(), *port)
            }
            AllowUrl::ExactFile { path } => {
                if uri.scheme() != "file" {
                    return false;
                }
                absolute_file_path(uri)
                    .map(|p| p == *path)
                    .unwrap_or(false)
            }
            AllowUrl::Domain { domain } => {
                let Some(uri_host) = uri.host_str() else {
                    return false;
                };
                host_matches_domain(uri_host, domain)
            }
            AllowUrl::UrlPrefix {
                scheme,
                host,
                port,
                path_prefix,
            } => {
                uri.scheme() == scheme
                    && uri.host_str() == Some(host.as_str())
                    && ports_match(uri.port(), *port)
                    && path_matches_prefix(uri.path(), path_prefix)
            }
        }
    }
}

/// CLI / reproduce form. [`AllowUrl::ExactHost`] is origin-only and not displayed.
impl Display for AllowUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            AllowUrl::ExactHost { .. } => {
                panic!("origin allow-url is implicit and not reproduced")
            }
            AllowUrl::ExactFile { path } => {
                let url = Url::from_file_path(path).map_err(|_| std::fmt::Error)?;
                write!(f, "{url}")
            }
            AllowUrl::Domain { domain } => write!(f, "{domain}"),
            AllowUrl::UrlPrefix {
                scheme,
                host,
                port,
                path_prefix,
            } => {
                write!(f, "{scheme}://{host}")?;
                if let Some(port) = port {
                    write!(f, ":{port}")?;
                }
                if path_prefix != "/" {
                    write!(f, "{path_prefix}")?;
                }
                Ok(())
            }
        }
    }
}

/// Build the exploration allow-list.
///
/// * No explicit entries → default to the origin only ([`AllowUrl::from_origin`]).
/// * One or more `--allow-url` entries → those alone (replace, do not widen).
///   Include the origin in `--allow-url` if it should remain in bounds.
pub fn build_allow_list(origin: &Url, extra: &[AllowUrl]) -> Vec<AllowUrl> {
    if extra.is_empty() {
        AllowUrl::from_origin(origin).into_iter().collect()
    } else {
        extra.to_vec()
    }
}

pub fn is_url_allowed(uri: &Url, allow_urls: &[AllowUrl]) -> bool {
    // about: URLs (blank, srcdoc, …) are tab setup or non-app chrome, never
    // exploration targets; out of bounds → Back only.
    if uri.scheme() == "about" {
        return false;
    }
    allow_urls.iter().any(|rule| rule.matches(uri))
}

/// Absolute filesystem path for a `file://` URL.
fn absolute_file_path(url: &Url) -> Result<PathBuf, String> {
    if url.scheme() != "file" {
        return Err(format!("expected file URL, got {}", url.scheme()));
    }
    let path = url
        .to_file_path()
        .map_err(|()| format!("invalid file URL {url}"))?;
    std::path::absolute(path)
        .map_err(|err| format!("could not resolve absolute path for {url}: {err}"))
}

fn normalize_domain(domain: &str) -> String {
    domain.trim_start_matches('.').to_string()
}

fn normalize_path_prefix(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

fn host_matches_domain(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// Port constraint from the allow rule alone (no separate origin port).
fn ports_match(uri_port: Option<u16>, rule_port: Option<u16>) -> bool {
    match (uri_port, rule_port) {
        (Some(uri), Some(expected)) => uri == expected,
        (Some(_), None) => true,
        (None, _) => true,
    }
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{
        TestCase,
        generators::{booleans, urls},
    };

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn allow(origin: &str, extra: &[&str]) -> Vec<AllowUrl> {
        let origin = url(origin);
        build_allow_list(
            &origin,
            &extra
                .iter()
                .map(|raw| AllowUrl::parse(raw).unwrap())
                .collect::<Vec<_>>(),
        )
    }

    #[hegel::composite]
    fn draw_network_url(tc: TestCase) -> Url {
        let u = Url::parse(&tc.draw(urls())).unwrap_or_else(|_| tc.reject());
        if u.host_str().is_none() {
            tc.reject();
        }
        u
    }

    #[hegel::composite]
    fn draw_allow_url(tc: TestCase) -> AllowUrl {
        let seed = tc.draw(draw_network_url());
        let host = seed.host_str().unwrap().to_string();
        if tc.draw(booleans()) {
            AllowUrl::Domain { domain: host }
        } else {
            AllowUrl::UrlPrefix {
                scheme: seed.scheme().to_string(),
                host,
                port: seed.port(),
                path_prefix: normalize_path_prefix(seed.path()),
            }
        }
    }

    #[test]
    fn file_origin_allows_exact_path_only() {
        let origin = url("file:///tmp/index.html");
        let rules = build_allow_list(&origin, &[]);
        assert_eq!(rules.len(), 1);
        assert!(matches!(&rules[0], AllowUrl::ExactFile { .. }));

        let origin_path = absolute_file_path(&origin).unwrap();
        let origin_url =
            Url::from_file_path(&origin_path).expect("file path to url");
        assert!(is_url_allowed(&origin_url, &rules));

        // Sibling file path is not implied; allow only exact origin path.
        let other = url("file:///tmp/other.html");
        assert!(!is_url_allowed(&other, &rules));

        assert!(!is_url_allowed(&url("about:blank"), &rules));
        assert!(!is_url_allowed(&url("https://example.com/"), &rules));
    }

    #[test]
    fn file_allow_url_is_exact_match() {
        let origin = url("https://example.com/");
        let allowed = url("file:///tmp/allowed.html");
        let allowed_abs = Url::from_file_path(absolute_file_path(&allowed).unwrap())
            .unwrap();
        let rules = build_allow_list(
            &origin,
            &[AllowUrl::parse(allowed_abs.as_str()).unwrap()],
        );
        assert!(is_url_allowed(&allowed_abs, &rules));
        assert!(!is_url_allowed(&url("file:///tmp/other.html"), &rules));
    }

    #[test]
    fn about_scheme_is_always_out_of_bounds() {
        let network_rules = allow("http://localhost:1073/", &[]);
        assert!(!is_url_allowed(&url("about:blank"), &network_rules));
        assert!(!is_url_allowed(&url("about:srcdoc"), &network_rules));

        let file = url("file:///tmp/index.html");
        let file_rules = build_allow_list(&file, &[]);
        assert!(!is_url_allowed(&url("about:blank"), &file_rules));
        assert!(!is_url_allowed(&url("about:srcdoc"), &file_rules));
    }

    #[test]
    fn origin_only_allows_same_host() {
        let rules = allow("https://example.com/app", &[]);
        assert!(is_url_allowed(&url("https://example.com/other"), &rules));
        assert!(!is_url_allowed(
            &url("https://app.example.com/other"),
            &rules
        ));
    }

    #[test]
    fn explicit_allow_url_replaces_origin_default() {
        // With --allow-url, origin is not implicitly kept.
        let rules = build_allow_list(
            &url("https://origin.example/"),
            &[AllowUrl::parse("https://allowed.example/app").unwrap()],
        );
        assert_eq!(rules.len(), 1);
        assert!(!is_url_allowed(&url("https://origin.example/"), &rules));
        assert!(is_url_allowed(
            &url("https://allowed.example/app/page"),
            &rules
        ));
    }

    #[test]
    fn empty_allow_url_keeps_origin_default() {
        let rules = build_allow_list(&url("https://origin.example/"), &[]);
        assert_eq!(rules.len(), 1);
        assert!(matches!(&rules[0], AllowUrl::ExactHost { .. }));
        assert!(is_url_allowed(&url("https://origin.example/x"), &rules));
    }

    #[test]
    fn domain_entry_allows_subdomains() {
        let rules = allow("https://example.com/", &[".example.com"]);
        assert!(is_url_allowed(
            &url("https://app.example.com/path"),
            &rules
        ));
        assert!(!is_url_allowed(
            &url("https://notexample.com/path"),
            &rules
        ));
    }

    #[test]
    fn domain_entry_does_not_reuse_origin_port() {
        // Origin port must not constrain a Domain rule (Oskar's example).
        let rules = build_allow_list(
            &url("http://origin.example:8080/"),
            &[AllowUrl::parse("other.example").unwrap()],
        );
        assert!(is_url_allowed(
            &url("http://other.example:9090/"),
            &rules
        ));
        assert!(is_url_allowed(
            &url("http://other.example:8080/"),
            &rules
        ));
    }

    #[test]
    fn url_prefix_port_is_self_contained() {
        // allow-url http://other:9090 must not accept other:8080 via origin port.
        let rules = build_allow_list(
            &url("http://origin.example:8080/"),
            &[AllowUrl::parse("http://other.example:9090/").unwrap()],
        );
        assert!(is_url_allowed(
            &url("http://other.example:9090/path"),
            &rules
        ));
        assert!(!is_url_allowed(
            &url("http://other.example:8080/path"),
            &rules
        ));
    }

    #[test]
    fn url_prefix_entry_allows_scoped_paths() {
        let rules = allow(
            "https://other.example.com/",
            &["https://example.com/my/cool/feature"],
        );
        assert!(is_url_allowed(
            &url("https://example.com/my/cool/feature/extra"),
            &rules
        ));
        assert!(!is_url_allowed(
            &url("https://example.com/my/cool/features"),
            &rules
        ));
    }

    #[test]
    fn parse_domain_and_url_forms() {
        assert_eq!(
            AllowUrl::parse("example.com").unwrap(),
            AllowUrl::Domain {
                domain: "example.com".into()
            }
        );
        assert_eq!(
            AllowUrl::parse("https://example.com/my/cool/feature").unwrap(),
            AllowUrl::UrlPrefix {
                scheme: "https".into(),
                host: "example.com".into(),
                port: None,
                path_prefix: "/my/cool/feature".into(),
            }
        );
    }

    #[hegel::test]
    fn roundtrip_display_parse(tc: TestCase) {
        let allow = tc.draw(draw_allow_url());
        let formatted = format!("{allow}");
        let parsed = AllowUrl::parse(&formatted).unwrap();
        assert_eq!(parsed, allow);
    }

    /// Given Url `u` that parses as AllowUrl `a`, `is_url_allowed(u, &[a])`.
    #[hegel::test]
    fn is_url_allowed_when_parsed_from_url(tc: TestCase) {
        let u = tc.draw(draw_network_url());
        let a = if tc.draw(booleans()) {
            AllowUrl::parse(u.host_str().unwrap()).unwrap()
        } else {
            AllowUrl::parse(u.as_str()).unwrap_or_else(|_| tc.reject())
        };
        assert!(
            is_url_allowed(&u, std::slice::from_ref(&a)),
            "url {u} should be allowed by {a}"
        );
    }
}

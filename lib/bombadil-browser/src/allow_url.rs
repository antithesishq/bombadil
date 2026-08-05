use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllowUrl {
    /// The starting origin host. Added implicitly when the origin has a host;
    /// any path on this host is allowed.
    ExactHost { host: String, port: Option<u16> },
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

    /// Implicit allow rule for the test origin, if it has a network host.
    ///
    /// Origins without a host (e.g. `file://`) produce no rule. Host-less
    /// *targets* such as `about:blank` and other `file://` pages are handled
    /// separately in [`is_url_allowed`], so exploration still works for
    /// inspect reports and mid-navigation blanks without a dedicated allow-url
    /// variant.
    pub fn from_origin(origin: &Url) -> Option<Self> {
        origin.host_str().map(|host| AllowUrl::ExactHost {
            host: host.to_string(),
            port: origin.port(),
        })
    }

    pub fn cli_value(&self) -> String {
        match self {
            AllowUrl::ExactHost { .. } => {
                panic!("origin allow-url is implicit and not reproduced")
            }
            AllowUrl::Domain { domain } => domain.clone(),
            AllowUrl::UrlPrefix {
                scheme,
                host,
                port,
                path_prefix,
            } => {
                let mut url = format!("{scheme}://{host}");
                if let Some(port) = port {
                    url.push(':');
                    url.push_str(&port.to_string());
                }
                if path_prefix != "/" {
                    url.push_str(path_prefix);
                }
                url
            }
        }
    }

    fn matches(&self, uri: &Url, origin: &Url) -> bool {
        match self {
            AllowUrl::ExactHost { host, port } => {
                uri.host_str() == Some(host.as_str())
                    && ports_match(uri.port(), *port, origin.port())
            }
            AllowUrl::Domain { domain } => {
                if uri.port().is_some() && uri.port() != origin.port() {
                    return false;
                }
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
                    && ports_match(uri.port(), *port, *port)
                    && path_matches_prefix(uri.path(), path_prefix)
            }
        }
    }
}

pub fn build_allow_list(origin: &Url, extra: &[AllowUrl]) -> Vec<AllowUrl> {
    let mut allow_urls = Vec::new();
    if let Some(origin_rule) = AllowUrl::from_origin(origin) {
        allow_urls.push(origin_rule);
    }
    allow_urls.extend(extra.iter().cloned());
    allow_urls
}

pub fn is_url_allowed(
    uri: &Url,
    allow_urls: &[AllowUrl],
    origin: &Url,
) -> bool {
    // Host-less targets (about:blank mid-navigation, file:// pages, etc.) are
    // always treated as in-bounds so the action set does not go empty. This is
    // separate from the allow-list rules, which only apply to networked hosts.
    if uri.host().is_none() {
        return uri.port().is_none() || uri.port() == origin.port();
    }
    allow_urls.iter().any(|rule| rule.matches(uri, origin))
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

fn ports_match(
    uri_port: Option<u16>,
    rule_port: Option<u16>,
    origin_port: Option<u16>,
) -> bool {
    let expected = rule_port.or(origin_port);
    match (uri_port, expected) {
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

    #[test]
    fn file_origin_has_no_implicit_rule_but_allows_hostless_targets() {
        let origin = url("file:///tmp/index.html");
        let rules = build_allow_list(&origin, &[]);
        assert!(rules.is_empty());
        assert!(AllowUrl::from_origin(&origin).is_none());
        // Other file:// pages and about:blank stay in-bounds via the host-less
        // target special case, not via an allow-list rule.
        assert!(is_url_allowed(
            &url("file:///tmp/other.html"),
            &rules,
            &origin
        ));
        assert!(is_url_allowed(&url("about:blank"), &rules, &origin));
        assert!(!is_url_allowed(
            &url("https://example.com/"),
            &rules,
            &origin
        ));
    }

    #[test]
    fn hostless_urls_are_allowed() {
        let origin = url("http://localhost:1073/");
        let rules = allow("http://localhost:1073/", &[]);
        assert!(is_url_allowed(&url("about:blank"), &rules, &origin));
    }

    #[test]
    fn origin_only_allows_same_host() {
        let origin = url("https://example.com/app");
        let rules = allow("https://example.com/app", &[]);
        assert!(is_url_allowed(
            &url("https://example.com/other"),
            &rules,
            &origin
        ));
        assert!(!is_url_allowed(
            &url("https://app.example.com/other"),
            &rules,
            &origin
        ));
    }

    #[test]
    fn domain_entry_allows_subdomains() {
        let origin = url("https://example.com/");
        let rules = allow("https://example.com/", &[".example.com"]);
        assert!(is_url_allowed(
            &url("https://app.example.com/path"),
            &rules,
            &origin
        ));
        assert!(!is_url_allowed(
            &url("https://notexample.com/path"),
            &rules,
            &origin
        ));
    }

    #[test]
    fn url_prefix_entry_allows_scoped_paths() {
        let origin = url("https://other.example.com/");
        let rules = allow(
            "https://other.example.com/",
            &["https://example.com/my/cool/feature"],
        );
        assert!(is_url_allowed(
            &url("https://example.com/my/cool/feature/extra"),
            &rules,
            &origin
        ));
        assert!(!is_url_allowed(
            &url("https://example.com/my/cool/features"),
            &rules,
            &origin
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
}

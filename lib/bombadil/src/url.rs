use anyhow::{Result, anyhow};
use url::Url;

pub fn is_within_domain(uri: &Url, domain: &Url) -> bool {
    (uri.host().is_none() || uri.host() == domain.host())
        && (uri.port().is_none() || uri.port() == domain.port())
}

#[allow(unused, reason = "porting this to js scripts")]
pub fn parse_browser_url(string: &str, context: &Url) -> Result<Url> {
    context.join(string).map_err(|err| anyhow!(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_browser_url_file_name() {
        let url = parse_browser_url(
            "foo.html",
            &Url::parse("https://example.com").unwrap(),
        )
        .unwrap();
        assert_eq!(url.to_string(), "https://example.com/foo.html");
    }

    #[test]
    fn test_parse_browser_url_absolute_path() {
        let url = parse_browser_url(
            "/foo/bar.html",
            &Url::parse("https://example.com").unwrap(),
        )
        .unwrap();
        assert_eq!(url.to_string(), "https://example.com/foo/bar.html");
    }

    #[test]
    fn test_parse_browser_url_blank_scheme() {
        let url = parse_browser_url(
            "//other.example.com/foo/bar.html",
            &Url::parse("https://example.com").unwrap(),
        )
        .unwrap();
        assert_eq!(url.to_string(), "https://other.example.com/foo/bar.html");
    }

    #[test]
    fn test_parse_browser_url_query_and_fragment() {
        let url = parse_browser_url(
            "/foo/bar.html?foo=bar#baz",
            &Url::parse("https://example.com").unwrap(),
        )
        .unwrap();
        assert_eq!(
            url.to_string(),
            "https://example.com/foo/bar.html?foo=bar#baz"
        );
    }

    #[test]
    fn test_parse_browser_url_relative() {
        let url = parse_browser_url(
            "../baz.html",
            &Url::parse("https://example.com/foo/bar/baz/").unwrap(),
        )
        .unwrap();
        assert_eq!(url.to_string(), "https://example.com/foo/bar/baz.html");
    }

    #[test]
    fn test_parse_browser_url_mailto() {
        let url = parse_browser_url(
            "mailto:me@example.com",
            &Url::parse("https://example.com").unwrap(),
        )
        .unwrap();
        assert_eq!(url.to_string(), "mailto:me@example.com");
    }
}

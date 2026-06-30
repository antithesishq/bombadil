use anyhow::{Result, anyhow};
use chromiumoxide::cdp::browser_protocol::network;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserCookie {
    raw: String,
    pub name: String,
    pub value: String,
    pub domain: Option<String>,
    pub path: Option<String>,
    pub secure: bool,
    pub http_only: bool,
}

impl BrowserCookie {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("cookie must not be empty".into());
        }

        let mut segments = raw.split(';').map(str::trim);
        let name_value = segments
            .next()
            .ok_or_else(|| "cookie must start with NAME=VALUE".to_string())?;
        let (name, value) = name_value.split_once('=').ok_or_else(|| {
            format!("invalid cookie {name_value:?}, expected NAME=VALUE")
        })?;
        if name.is_empty() {
            return Err("cookie name must not be empty".into());
        }

        let mut cookie = BrowserCookie {
            raw: raw.to_string(),
            name: name.to_string(),
            value: unquote(value),
            domain: None,
            path: None,
            secure: false,
            http_only: false,
        };

        for segment in segments {
            if segment.is_empty() {
                continue;
            }
            if let Some((attribute, attribute_value)) = segment.split_once('=')
            {
                match attribute.trim().to_ascii_lowercase().as_str() {
                    "domain" => {
                        cookie.domain = Some(unquote(attribute_value.trim()));
                    }
                    "path" => {
                        cookie.path = Some(unquote(attribute_value.trim()));
                    }
                    other => {
                        return Err(format!(
                            "unknown cookie attribute {other:?}"
                        ));
                    }
                }
            } else {
                match segment.to_ascii_lowercase().as_str() {
                    "secure" => cookie.secure = true,
                    "httponly" => cookie.http_only = true,
                    other => {
                        return Err(format!(
                            "unknown cookie attribute {other:?}"
                        ));
                    }
                }
            }
        }

        Ok(cookie)
    }

    pub fn cli_value(&self) -> &str {
        &self.raw
    }
}

pub fn build_cookie_param(
    cookie: &BrowserCookie,
    origin: &Url,
) -> Result<network::CookieParam> {
    let mut builder = network::CookieParam::builder()
        .name(&cookie.name)
        .value(&cookie.value);

    if cookie.domain.is_some() || cookie.path.is_some() {
        let domain = cookie
            .domain
            .as_deref()
            .or_else(|| origin.host_str())
            .ok_or_else(|| anyhow!("origin URL has no host"))?;
        builder = builder
            .domain(domain)
            .path(cookie.path.as_deref().unwrap_or("/"))
            .secure(cookie.secure || origin.scheme() == "https");
        if cookie.http_only {
            builder = builder.http_only(true);
        }
    } else {
        builder = builder.url(origin.as_str());
        if cookie.secure {
            builder = builder.secure(true);
        }
        if cookie.http_only {
            builder = builder.http_only(true);
        }
    }

    builder
        .build()
        .map_err(|s| anyhow!("build CookieParam failed: {s}"))
}

fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_value_only() {
        let cookie = BrowserCookie::parse("session=abc").unwrap();
        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc");
        assert_eq!(cookie.domain, None);
        assert_eq!(cookie.path, None);
        assert!(!cookie.secure);
        assert!(!cookie.http_only);
    }

    #[test]
    fn parse_value_with_equals_sign() {
        let cookie = BrowserCookie::parse("token=a=b=c").unwrap();
        assert_eq!(cookie.name, "token");
        assert_eq!(cookie.value, "a=b=c");
    }

    #[test]
    fn parse_set_cookie_attributes() {
        let cookie = BrowserCookie::parse(
            "session=abc; Domain=.example.com; Path=/app; Secure; HttpOnly",
        )
        .unwrap();
        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc");
        assert_eq!(cookie.domain.as_deref(), Some(".example.com"));
        assert_eq!(cookie.path.as_deref(), Some("/app"));
        assert!(cookie.secure);
        assert!(cookie.http_only);
    }

    #[test]
    fn parse_domain_is_case_insensitive() {
        let cookie =
            BrowserCookie::parse("session=abc; domain=localhost").unwrap();
        assert_eq!(cookie.domain.as_deref(), Some("localhost"));
    }

    #[test]
    fn rejects_unknown_attribute() {
        assert!(BrowserCookie::parse("session=abc; Expires=Wed").is_err());
    }
}

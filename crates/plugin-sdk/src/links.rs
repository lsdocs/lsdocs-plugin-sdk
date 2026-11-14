//! Resolving link URLs found in documents.
//!
//! Relative URLs are resolved against the linking document's directory, or
//! against the source directory if they start with `/`, the same way by the
//! host and by every plugin.

/// Where a link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlTarget {
    /// Another site, or a non-file scheme such as `mailto:`.
    External,
    /// Above the source directory.
    Outside,
    /// A file in the site, by its path relative to the source directory
    /// (`None` for the linking document itself), and an optional fragment.
    Internal {
        /// The file's path, or `None` for the linking document.
        path: Option<String>,
        /// The part after `#`, percent-decoded; `None` if absent or empty.
        fragment: Option<String>,
    },
}

/// Resolves `url`, found in the document `from`.
pub fn resolve_url(url: &str, from: &str) -> UrlTarget {
    let has_scheme = url.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    if has_scheme || url.starts_with("//") {
        return UrlTarget::External;
    }
    let (rest, fragment) = match url.split_once('#') {
        Some((rest, fragment)) => (rest, Some(percent_decode(fragment))),
        None => (url, None),
    };
    let fragment = fragment.filter(|f| !f.is_empty());
    let path = rest.split_once('?').map_or(rest, |(path, _)| path);
    if path.is_empty() {
        return UrlTarget::Internal {
            path: None,
            fragment,
        };
    }
    let path = percent_decode(path);
    let (base, path) = match path.strip_prefix('/') {
        Some(rooted) => ("", rooted),
        None => (
            from.rsplit_once('/').map_or("", |(dir, _)| dir),
            path.as_str(),
        ),
    };
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return UrlTarget::Outside;
                }
            }
            part => parts.push(part),
        }
    }
    let mut joined = parts.join("/");
    if path.ends_with('/') && !joined.is_empty() {
        joined.push('/');
    }
    UrlTarget::Internal {
        path: Some(joined),
        fragment,
    }
}

/// Decodes `%XX` escapes; malformed escapes are kept as they are.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal(path: Option<&str>, fragment: Option<&str>) -> UrlTarget {
        UrlTarget::Internal {
            path: path.map(Into::into),
            fragment: fragment.map(Into::into),
        }
    }

    #[test]
    fn resolving() {
        assert_eq!(
            resolve_url("https://x.org/a.md", "a.md"),
            UrlTarget::External
        );
        assert_eq!(resolve_url("//cdn/x", "a.md"), UrlTarget::External);
        assert_eq!(resolve_url("mailto:me@x.org", "a.md"), UrlTarget::External);
        assert_eq!(resolve_url("#top", "g/a.md"), internal(None, Some("top")));
        assert_eq!(resolve_url("?q=1", "g/a.md"), internal(None, None));
        assert_eq!(
            resolve_url("b.md", "g/a.md"),
            internal(Some("g/b.md"), None)
        );
        assert_eq!(
            resolve_url("../c/d.md?x#Sec%20One", "g/a.md"),
            internal(Some("c/d.md"), Some("Sec One"))
        );
        assert_eq!(
            resolve_url("/img/x.svg", "g/h/a.md"),
            internal(Some("img/x.svg"), None)
        );
        assert_eq!(
            resolve_url("./my%20file.md", "a.md"),
            internal(Some("my file.md"), None)
        );
        assert_eq!(resolve_url("sub/", "a.md"), internal(Some("sub/"), None));
        assert_eq!(resolve_url("../x.md", "a.md"), UrlTarget::Outside);
    }
}

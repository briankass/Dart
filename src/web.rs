//! Web search row: "? query" (or "g query") searches the web, and any query
//! with no app matches offers a web search as a fallback.

const SEARCH_URL: &str = "https://www.google.com/search?q=";

/// If the query uses the explicit web prefix, returns the search terms.
pub fn explicit(query: &str) -> Option<&str> {
    let q = query.trim_start();
    let rest = q.strip_prefix("? ").or_else(|| q.strip_prefix("g "))?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

pub fn url(terms: &str) -> String {
    format!("{SEARCH_URL}{}", encode(terms.trim()))
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(explicit("? rust lang"), Some("rust lang"));
        assert_eq!(explicit("g weather"), Some("weather"));
        assert_eq!(explicit("?  "), None);
        assert_eq!(explicit("git"), None);
        assert_eq!(explicit("notepad"), None);
    }

    #[test]
    fn encoding() {
        assert_eq!(url("a b&c"), "https://www.google.com/search?q=a+b%26c");
        assert_eq!(url("café"), "https://www.google.com/search?q=caf%C3%A9");
    }
}

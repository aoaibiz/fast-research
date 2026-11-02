//! Readable text extraction for the optional raw-content results.
//!
//! This is not a full readability implementation: it takes the largest of the
//! article/main/body containers and normalises the whitespace, which is enough
//! for an agent to read the page behind a search result.

use scraper::Html;

/// Longest text returned per page, in characters, unless the caller asks for
/// a different limit.
const MAX_CHARS: usize = 4_000;

/// The readable text of a parsed HTML document.
pub fn main_text(html: &Html) -> String {
    main_text_limited(html, MAX_CHARS)
}

/// The readable text of a parsed HTML document, capped at the given length.
pub fn main_text_limited(html: &Html, max: usize) -> String {
    const CANDIDATES: &[&str] = &["article", "main", "[role=main]", "body"];
    for selector in CANDIDATES {
        let Ok(selector) = scraper::Selector::parse(selector) else {
            continue;
        };
        if let Some(element) = html.select(&selector).next() {
            let text = normalise(&element.text().collect::<Vec<_>>().join(" "));
            if !text.is_empty() {
                return truncate(&text, max);
            }
        }
    }
    String::new()
}

/// Collapse whitespace and drop empty lines.
fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_wins_over_body() {
        let html = Html::parse_document(
            "<html><body><nav>menu</nav><article><p>Hello   world</p></article></body></html>",
        );
        assert_eq!(main_text(&html), "Hello world");
    }

    #[test]
    fn plain_body_is_the_fallback() {
        let html = Html::parse_document("<html><body><p>Just text</p></body></html>");
        assert_eq!(main_text(&html), "Just text");
    }

    #[test]
    fn long_text_is_truncated() {
        let body = "a".repeat(MAX_CHARS + 100);
        let html = Html::parse_document(&format!("<html><body><p>{body}</p></body></html>"));
        let text = main_text(&html);
        assert!(text.chars().count() <= MAX_CHARS + 1);
    }
}

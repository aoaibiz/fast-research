//! Bounded extraction and explicit quality/transport outcomes for agent consumers.
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageEvidence {
    pub status: String,
    pub code: String,
    pub fetched_at: Option<String>,
    pub final_url: Option<String>,
    pub characters: usize,
    pub truncated: bool,
    pub method: String,
    pub cache_hit: bool,
    pub next_action: String,
}

pub fn extract(html: &Html, max: usize) -> (String, PageEvidence) {
    let max = max.clamp(200, 32_000);
    let mut best = String::new();
    let mut method = "none";
    for (label, selector) in [
        ("article", "article"),
        ("main", "main, [role=main]"),
        ("body_fallback", "body"),
    ] {
        let selector = Selector::parse(selector).unwrap();
        let text = html
            .select(&selector)
            .map(|el| {
                el.descendants()
                    .filter_map(|node| {
                        let value = node.value().as_text()?;
                        let excluded = node.ancestors().any(|a| {
                            a.value().as_element().is_some_and(|e| {
                                matches!(
                                    e.name(),
                                    "script"
                                        | "style"
                                        | "nav"
                                        | "footer"
                                        | "header"
                                        | "noscript"
                                        | "svg"
                                        | "form"
                                )
                            })
                        });
                        if excluded {
                            None
                        } else {
                            Some(value.to_string())
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .max_by_key(|s| s.chars().count())
            .unwrap_or_default();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.chars().count() > best.chars().count() {
            best = text;
            method = label;
        }
        // Prefer semantic content when substantial; a tiny article can fall back to main/body.
        if best.chars().count() >= 200 {
            break;
        }
    }
    let total = best.chars().count();
    let lower = best.to_ascii_lowercase();
    let blocked = total < 2000
        && [
            "access denied",
            "verify you are human",
            "just a moment",
            "enable javascript and cookies",
            "captcha",
        ]
        .iter()
        .any(|s| lower.contains(s));
    let (status, code) = if blocked {
        ("failed", "challenge_page")
    } else if total == 0 {
        ("failed", "empty_text")
    } else if total < 200 {
        ("partial", "short_text")
    } else {
        ("ok", "extracted")
    };
    let evidence = PageEvidence {
        status: status.into(),
        code: code.into(),
        characters: total.min(max),
        truncated: total > max,
        method: method.into(),
        next_action: if status == "ok" {
            "verify_relevance"
        } else {
            "try_another_source"
        }
        .into(),
        ..Default::default()
    };
    let text = if blocked {
        String::new()
    } else {
        best.chars().take(max).collect()
    };
    (text, evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_article_recovers_main_and_excludes_scripts() {
        let html = Html::parse_document(&format!(
            "<body><script>secret script</script><article>tiny</article><main><p>{}</p></main></body>",
            "本文です。".repeat(100)
        ));
        let (text, meta) = extract(&html, 300);
        assert_eq!(meta.status, "ok");
        assert!(meta.truncated);
        assert_eq!(meta.method, "main");
        assert_eq!(text.chars().count(), 300);
        assert!(!text.contains("script"));
    }
    #[test]
    fn distinguishes_challenge_empty_and_short() {
        for (body, code) in [
            ("<body>Access denied</body>", "challenge_page"),
            ("<body></body>", "empty_text"),
            ("<body>短い説明</body>", "short_text"),
        ] {
            assert_eq!(extract(&Html::parse_document(body), 8000).1.code, code);
        }
    }
}

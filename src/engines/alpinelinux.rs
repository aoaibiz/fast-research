//! Alpine Linux engine, ported from `searx/engines/alpinelinux.py`.
// engine-id: alpinelinux

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["packages", "it"],
    shortcut: "apk",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pkgs.alpinelinux.org";

const ARCHES: [&str; 8] = [
    "x86_64", "x86", "aarch64", "armhf", "ppc64le", "s390x", "armv7", "riscv64",
];

/// Alpine Linux binary packages search.
pub struct AlpineLinux {
    meta: EngineMeta,
}

impl AlpineLinux {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn arch_in(&self, query: &str) -> Option<String> {
        ARCHES
            .iter()
            .find(|arch| query.contains(**arch))
            .map(|s| s.to_string())
    }
}

impl Engine for AlpineLinux {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let text = query.query_text();
        let query_arch = self.arch_in(&text);
        let query = if let Some(arch) = &query_arch {
            text.replace(arch, "").trim().to_string()
        } else {
            text
        };
        let arch = query_arch.unwrap_or_else(|| "x86_64".to_string());
        // Upstream keeps the "*" wildcards and the arch literal as-is.
        let url = format!("{base}/packages?name=*{query}*&page={pageno}&arch={arch}");
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let mut results = Vec::new();
        for row in select_elements(&html, "table tbody tr")? {
            // Skip non-valid entries (e.g. "No item found" rows) that lack enough cells.
            if children(row, "td").len() < 9 {
                continue;
            }
            let package = text_of(row, "td.package").unwrap_or_default();
            if package.is_empty() {
                continue;
            }
            let href = attr_of(row, "td.package a", "href").unwrap_or_default();
            let url = if href.is_empty() {
                String::new()
            } else {
                absolute_url(&href, base).unwrap_or_default()
            };
            if url.is_empty() || !url.starts_with("http") {
                continue;
            }
            let mut builder = ResultBuilder::new(url, package.clone())
                .template(Template::Packages)
                .metadata("package_name", package);
            if let Some(date) = text_of(row, "td.bdate") {
                builder = builder.published_date(date);
            }
            if let Some(version) = text_of(row, "td.version") {
                builder = builder.metadata("version", version);
            }
            if let Some(homepage) = attr_of(row, "td.url a", "href") {
                builder = builder.metadata("homepage", homepage);
            }
            if let Some(maintainer) = text_of(row, "td.maintainer") {
                builder = builder.metadata("maintainer", maintainer);
            }
            if let Some(license) = text_of(row, "td.license") {
                builder = builder.metadata("license_name", license);
            }
            if let Some(repo) = text_of(row, "td.repo") {
                builder = builder.metadata("tags", repo);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_package_table() {
        let html = r#"
        <table><tbody>
          <tr>
            <td class="package"><a href="/package/x86_64/testing/foo-src">foo</a></td>
            <td class="bdate">2023-01-01</td>
            <td class="version">1.2.3</td>
            <td class="url"><a href="https://example.com">home</a></td>
            <td class="maintainer">alice</td>
            <td class="license">MIT</td>
            <td class="repo">main</td>
            <td>extra</td><td>extra</td>
          </tr>
          <tr><td>No item found</td></tr>
        </tbody></table>"#;
        let response = EngineResponse::new(
            200,
            "https://pkgs.alpinelinux.org/",
            html.as_bytes().to_vec(),
        );
        let engine = AlpineLinux::from_config(&EngineConfig::new(
            "alpinelinux",
            "alpinelinux",
            "apk",
            &["packages", "it"],
        ))
        .unwrap();
        let query = Query::parse("foo x86_64", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        assert!(
            matches!(&items[0], EngineItem::Result(r) if r.title == "foo" && r.metadata.contains_key("version"))
        );
    }

    #[test]
    fn builds_the_url_with_arch_in_query() {
        let engine = AlpineLinux::from_config(&EngineConfig::new(
            "alpinelinux",
            "alpinelinux",
            "apk",
            &["packages", "it"],
        ))
        .unwrap();
        let query = Query::parse(
            "bar aarch64",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let request = engine.build_request(&query, 1).unwrap();
        assert!(request.url.contains("name=*bar*"));
        assert!(request.url.contains("arch=aarch64"));
        assert!(!request.url.contains("aarch64%20"));
    }
}

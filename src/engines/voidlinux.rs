//! Void Linux engine, ported from searx/engines/voidlinux.py.
// engine-id: voidlinux

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
    shortcut: "void",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://xq-api.voidlinux.org";
const PKG_REPO_URL: &str = "https://github.com/void-linux/void-packages";
/// Upstream's ARCH_RE alternatives, matched as whole query tokens.
const ARCHES: &str =
    "aarch64-musl armv6l-musl armv7l-musl x86_64-musl aarch64 armv6l armv7l i686 x86_64";

/// Void Linux binary packages.
pub struct VoidLinux {
    meta: EngineMeta,
}

impl VoidLinux {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }

    /// The architecture of the query and the search terms without it.
    fn split_arch(&self, raw: &str) -> (String, String) {
        let arch = raw
            .split_whitespace()
            .find(|token| ARCHES.split(' ').any(|value| value == *token));
        let terms = raw
            .split_whitespace()
            .filter(|token| Some(*token) != arch)
            .collect::<Vec<_>>()
            .join(" ");
        let arch = arch.map(str::to_string).unwrap_or_else(|| {
            self.meta
                .opt_str("void_arch")
                .unwrap_or("x86_64")
                .to_string()
        });
        (arch, terms)
    }
}

impl Engine for VoidLinux {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let (arch, terms) = self.split_arch(&query.query_text());
        let pairs: Vec<(&str, String)> = vec![("q", terms)];
        Ok(EngineRequest::get(format!(
            "{}/v1/query/{}?{}",
            self.base_url(),
            arch,
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(entries) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut packages: Vec<(String, Package)> = Vec::new();
        for item in entries {
            let name = json_str(item, "name");
            if name.is_empty() {
                continue;
            }
            let pkg_url = format!("{PKG_REPO_URL}/tree/master/srcpkgs/{}", source_slug(&name));
            let size = json_str(item, "filename_size");
            let size = size.parse::<f64>().map(humanize_bytes).unwrap_or_default();
            let revision = json_str(item, "revision");
            let version = format!("v{}_{}", json_str(item, "version"), revision);
            let repository = json_str(item, "repository");
            let content = format!("{} - {}", json_str(item, "short_desc"), size);
            let existing = packages
                .iter_mut()
                .position(|(url, _)| url.as_str() == pkg_url.as_str());
            match existing {
                Some(index) => {
                    let package = &mut packages[index].1;
                    package.title.push_str(" | ");
                    package.title.push_str(&name);
                    package.package_name.push_str(" | ");
                    package.package_name.push_str(&name);
                    package.tags.push_str(", ");
                    package.tags.push_str(&repository);
                }
                None => packages.push((
                    pkg_url,
                    Package {
                        title: name.clone(),
                        content,
                        package_name: name,
                        version,
                        tags: repository,
                    },
                )),
            }
        }
        let mut results = Vec::new();
        for (url, package) in packages {
            results.push(
                ResultBuilder::new(url, package.title)
                    .content(package.content)
                    .template(Template::Packages)
                    .metadata("package_name", package.package_name)
                    .metadata("version", package.version)
                    .metadata("tags", package.tags)
                    .build(),
            );
        }
        Ok(results)
    }
}

struct Package {
    title: String,
    content: String,
    package_name: String,
    version: String,
    tags: String,
}

/// The source package name: the -32bit and -dbg packages have no template.
fn source_slug(name: &str) -> String {
    ["-32bit", "-dbg"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
        .to_string()
}

/// Upstream's humanize_bytes (base 1024, two decimals).
fn humanize_bytes(mut size: f64) -> String {
    const SUFFIX: [&str; 5] = ["B ", "KB", "MB", "GB", "TB"];
    let mut position = 0usize;
    while size > 1024.0 && position < SUFFIX.len() {
        position += 1;
        size /= 1024.0;
    }
    let position = position.min(SUFFIX.len() - 1);
    format!("{size:.2} {}", SUFFIX[position])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"{"data":[{"name":"firefox","short_desc":"Mozilla Firefox","filename_size":73400320,"version":"128.0","revision":"1","repository":"current"},{"name":"firefox-32bit","short_desc":"Mozilla Firefox","filename_size":73000000,"version":"128.0","revision":"1","repository":"multilib"},{"name":"htop","short_desc":"Interactive process viewer","filename_size":2048,"version":"3.3.0","revision":"2","repository":"current"}]}"##;

    fn engine() -> Arc<VoidLinux> {
        let config = EngineConfig::new("voidlinux", "voidlinux", "void", &["packages"]);
        VoidLinux::from_config(&config).unwrap()
    }

    fn items() -> Vec<EngineItem> {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("firefox", &EngineRegistry::default(), &[]);
        engine().parse(&response, &query).unwrap()
    }

    #[test]
    fn merges_packages_sharing_a_source_template() {
        let items = items();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a package")
        };
        assert_eq!(result.title, "firefox | firefox-32bit");
        assert_eq!(result.content, "Mozilla Firefox - 70.00 MB");
        assert_eq!(
            result.metadata.get("tags").map(String::as_str),
            Some("current, multilib")
        );
    }

    #[test]
    fn extracts_the_architecture_from_the_query() {
        let query = Query::parse("htop aarch64-musl", &EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.contains("/aarch64-musl?q=htop"));
    }
}

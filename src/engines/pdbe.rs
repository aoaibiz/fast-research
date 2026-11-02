//! PDBe engine, ported from searx/engines/pdbe.py.
// engine-id: pdbe

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science"],
    shortcut: "pdb",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// Status codes of unpublished entries.
const UNPUBLISHED: &[&str] = &[
    "HPUB", "HOLD", "PROC", "WAIT", "AUTH", "AUCO", "REPL", "POLC", "REFI", "TRSF", "WDRN",
];
const SEARCH_URL: &str = "https://www.ebi.ac.uk/pdbe/search/pdb/select?";
const ENTRY_URL: &str = "https://www.ebi.ac.uk/pdbe/entry/pdb/{pdb_id}";
const PREVIEW_URL: &str =
    "https://www.ebi.ac.uk/pdbe/static/entry/{pdb_id}_deposited_chain_front_image-200x200.png";

/// Protein Data Bank in Europe search.
pub struct Pdbe {
    meta: EngineMeta,
}

impl Pdbe {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| SEARCH_URL.to_string())
    }
}

impl Engine for Pdbe {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let form = vec![
            ("q".to_string(), query.query_text()),
            ("wt".to_string(), "json".to_string()),
        ];
        Ok(EngineRequest::post(self.base_url())
            .form(form)
            .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "response.docs").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let hide_obsolete = self.meta.opt_bool("hide_obsolete").unwrap_or(false);
        let mut results = Vec::new();
        for item in items {
            let status = json_str(item, "status");
            if UNPUBLISHED.contains(&status.as_str()) {
                continue;
            }
            // Upstream skips every obsolete entry when hide_obsolete is set.
            if status == "OBS" && hide_obsolete {
                continue;
            }
            let pdb_id = json_str(item, "pdb_id");
            if pdb_id.is_empty() {
                continue;
            }
            let url = entry_url(&pdb_id);
            let (title, content, thumbnail) = if status == "OBS" {
                let superseded = json_str(item, "superseded_by");
                if superseded.is_empty() {
                    continue;
                }
                (
                    format!("{} (OBSOLETE)", json_str(item, "title")),
                    format!(
                        "This entry has been superseded by: {} ({superseded})",
                        entry_url(&superseded)
                    ),
                    None,
                )
            } else {
                body(item)
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(thumbnail) = thumbnail {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn entry_url(pdb_id: &str) -> String {
    ENTRY_URL.replace("{pdb_id}", pdb_id)
}

fn preview(item: &serde_json::Value) -> Option<String> {
    let pdb_id = json_str(item, "pdb_id");
    if pdb_id.is_empty() {
        None
    } else {
        Some(PREVIEW_URL.replace("{pdb_id}", &pdb_id))
    }
}

fn first_author(item: &serde_json::Value) -> Option<String> {
    let authors = json_path(item, "entry_author_list")?.as_array()?;
    authors.first()?.as_str().map(str::to_string)
}

/// Upstream composes the title and the citation line, empty on a KeyError.
fn body(item: &serde_json::Value) -> (String, String, Option<String>) {
    let title = json_str(item, "title");
    let thumbnail = preview(item);
    let (Some(author), Some(citation_title)) = (
        first_author(item),
        json_path(item, "citation_title").and_then(serde_json::Value::as_str),
    ) else {
        return (title, String::new(), thumbnail);
    };
    let journal = json_str(item, "journal");
    let content = if journal.is_empty() {
        if json_path(item, "release_year").is_none() {
            String::new()
        } else {
            format!(
                "{citation_title} - {author} () ({})",
                json_str(item, "release_year")
            )
        }
    } else {
        let required = ["journal_volume", "journal_page", "citation_year"];
        if required.iter().any(|key| json_path(item, key).is_none()) {
            String::new()
        } else {
            format!(
                "{citation_title} - {author} {journal} ({}) {} ({})",
                json_str(item, "journal_volume"),
                json_str(item, "journal_page"),
                json_str(item, "citation_year")
            )
        }
    };
    (title, content, thumbnail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"response":{"docs":[
      {"pdb_id":"1abc","title":"Crystal structure","status":"REL","citation_title":"Crystal structure","entry_author_list":["Doe, Jane"],"journal":"J Biol Chem","journal_volume":"280","journal_page":"123-130","citation_year":"2005","release_year":"2005"},
      {"pdb_id":"2def","title":"Old entry","status":"OBS","superseded_by":"1abc"},
      {"pdb_id":"3ghi","title":"Unpublished","status":"HPUB"},
      {"pdb_id":"4jkl","title":"NMR structure","status":"REL","citation_title":"NMR structure","entry_author_list":["Roe, Rick"],"release_year":"2019"}
    ]}}"##;

    fn engine() -> Arc<Pdbe> {
        Pdbe::from_config(&EngineConfig::new("pdbe", "pdbe", "pdb", &["science"])).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn response(body: &str) -> EngineResponse {
        EngineResponse::new(200, SEARCH_URL, body.as_bytes().to_vec())
    }

    #[test]
    fn parses_the_entries() {
        let items = engine()
            .parse(&response(FIXTURE), &query("hemoglobin"))
            .unwrap();
        assert_eq!(items.len(), 3);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://www.ebi.ac.uk/pdbe/entry/pdb/1abc");
        assert_eq!(result.title, "Crystal structure");
        assert_eq!(
            result.content,
            "Crystal structure - Doe, Jane J Biol Chem (280) 123-130 (2005)"
        );
        assert_eq!(
            result.thumbnail.as_deref(),
            Some(
                "https://www.ebi.ac.uk/pdbe/static/entry/1abc_deposited_chain_front_image-200x200.png"
            )
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(second.title, "Old entry (OBSOLETE)");
        assert_eq!(
            second.content,
            "This entry has been superseded by: https://www.ebi.ac.uk/pdbe/entry/pdb/1abc (1abc)"
        );
        assert_eq!(second.thumbnail, None);
        let EngineItem::Result(third) = &items[2] else {
            panic!("expected a result")
        };
        assert_eq!(third.content, "NMR structure - Roe, Rick () (2019)");
    }

    #[test]
    fn obsolete_entries_can_be_hidden() {
        let mut config = EngineConfig::new("pdbe", "pdbe", "pdb", &["science"]);
        config
            .extra
            .insert("hide_obsolete".into(), toml::Value::Boolean(true));
        let engine = Pdbe::from_config(&config).unwrap();
        let items = engine
            .parse(&response(FIXTURE), &query("hemoglobin"))
            .unwrap();
        assert_eq!(items.len(), 2);
        assert!(!items.iter().any(
            |item| matches!(item, EngineItem::Result(result) if result.title.contains("OBSOLETE"))
        ));
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("hemoglobin"), 1).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, SEARCH_URL);
        assert!(
            request
                .form
                .contains(&("q".to_string(), "hemoglobin".to_string()))
        );
        assert!(
            request
                .form
                .contains(&("wt".to_string(), "json".to_string()))
        );
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        assert!(
            engine()
                .parse(&response(r##"{"response":{}}"##), &query("hemoglobin"))
                .unwrap()
                .is_empty()
        );
    }
}

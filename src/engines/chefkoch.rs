//! Chefkoch engine, ported from searx/engines/chefkoch.py.
// engine-id: chefkoch

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &[],
    shortcut: "chef",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.chefkoch.de";
const PAGE_SIZE: u32 = 20;
const THUMBNAIL_FORMAT: &str = "crop-240x300";

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Chefkoch, the German recipe database.
pub struct Chefkoch {
    meta: EngineMeta,
}

impl Chefkoch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Chefkoch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let offset = u64::from(pageno.max(1) - 1) * u64::from(PAGE_SIZE);
        let pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("limit", PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{base}/v2/search-gateway/recipes?{}",
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let skip_premium = self.meta.opt_bool("skip_premium").unwrap_or(true);
        let thumbnail_format = self
            .meta
            .opt_str("thumbnail_format")
            .unwrap_or(THUMBNAIL_FORMAT);
        let mut results = Vec::new();
        for item in items {
            let Some(recipe) = json_path(item, "recipe") else {
                continue;
            };
            if skip_premium {
                let premium = json_path(recipe, "isPremium")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let plus = json_path(recipe, "isPlus")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if premium || plus {
                    continue;
                }
            }
            let url = json_str(recipe, "siteUrl");
            let title = json_str(recipe, "title");
            if title.is_empty() || !is_http(&url) {
                continue;
            }
            let mut content = Vec::new();
            let subtitle = json_str(recipe, "subtitle");
            if !subtitle.is_empty() {
                content.push(subtitle);
            }
            content.push(format!(
                "Schwierigkeitsstufe (1-3): {}",
                json_str(recipe, "difficulty")
            ));
            content.push(format!(
                "Zubereitungszeit: {}min",
                json_str(recipe, "preparationTime")
            ));
            content.push(format!(
                "Anzahl der Zutaten: {}",
                json_str(recipe, "ingredientCount")
            ));
            let submitted = json_str(recipe, "submissionDate");
            let published = if submitted.len() >= 19 {
                let prefix: String = submitted.chars().take(19).collect();
                chrono::NaiveDateTime::parse_from_str(&prefix, "%Y-%m-%dT%H:%M:%S")
                    .map(|date| date.format("%Y-%m-%dT%H:%M:%S").to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let template = json_str(recipe, "previewImageUrlTemplate");
            let thumbnail = template.replace("<format>", thumbnail_format);
            let mut builder = ResultBuilder::new(url, title)
                .content(content.join(" | "))
                .published_date(published);
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"results":[
      {"recipe":{"title":"Kartoffelsuppe","siteUrl":"https://www.chefkoch.de/rezepte/1",
       "isPremium":false,"isPlus":false,"submissionDate":"2024-01-02T03:04:05.123456",
       "difficulty":2,"preparationTime":30,"ingredientCount":8,
       "subtitle":"Cremig und einfach",
       "previewImageUrlTemplate":"https://img.chefkoch.de/a.jpg<format>"}},
      {"recipe":{"title":"Premium Rezept","siteUrl":"https://www.chefkoch.de/rezepte/2",
       "isPremium":true,"isPlus":false,"submissionDate":null,"difficulty":1,
       "preparationTime":5,"ingredientCount":3,"subtitle":"",
       "previewImageUrlTemplate":""}},
      {"recipe":{"title":"Ohne URL","siteUrl":"","isPremium":false,"isPlus":false,
       "submissionDate":null,"difficulty":1,"preparationTime":5,
       "ingredientCount":3,"subtitle":"","previewImageUrlTemplate":""}}
    ]}"##;

    fn engine() -> Arc<Chefkoch> {
        Chefkoch::from_config(&EngineConfig::new("chefkoch", "chefkoch", "chef", &[])).unwrap()
    }

    #[test]
    fn parses_and_skips_premium_recipes() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("suppe", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(result.url, "https://www.chefkoch.de/rezepte/1");
        assert_eq!(result.title, "Kartoffelsuppe");
        assert_eq!(
            result.content,
            "Cremig und einfach | Schwierigkeitsstufe (1-3): 2 | Zubereitungszeit: 30min | Anzahl der Zutaten: 8"
        );
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://img.chefkoch.de/a.jpgcrop-240x300")
        );
        assert_eq!(
            result.published_date.as_deref(),
            Some("2024-01-02T03:04:05")
        );
    }

    #[test]
    fn premium_recipes_can_be_kept() {
        let mut config = EngineConfig::new("chefkoch", "chefkoch", "chef", &[]);
        config
            .extra
            .insert("skip_premium".into(), toml::Value::Boolean(false));
        let engine = Chefkoch::from_config(&config).unwrap();
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("suppe", &crate::engine::EngineRegistry::default(), &[]);
        assert_eq!(engine.parse(&response, &query).unwrap().len(), 2);
    }

    #[test]
    fn builds_the_paged_request() {
        let query = Query::parse("suppe", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://api.chefkoch.de/v2/search-gateway/recipes?query=suppe&limit=20&offset=20"
        );
    }
}

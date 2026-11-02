//! Yandex Search API engine, ported from searx/engines/yandex_api.py.
//!
//! The synchronous v2 API returns the XML result page Base64 encoded inside a
//! JSON envelope; an api_key and folder id are required.

use std::sync::Arc;

use base64_decode_bytes as decode_base64;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: yandex_api

const API_URL: &str = "https://searchapi.api.cloud.yandex.net/v2/web/search";
const PAGE_SIZE: u32 = 10;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "yandex",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

/// A Yandex Search API client.
pub struct YandexApi {
    meta: EngineMeta,
    folder_id: String,
    region: String,
}

impl YandexApi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let folder_id = meta.opt_str("folder_id").unwrap_or_default().to_string();
        if folder_id.is_empty() {
            return Err("yandex_api requires the folder_id option".into());
        }
        let region = meta.opt_str("region").unwrap_or_default().to_string();
        Ok(Arc::new(Self {
            meta,
            folder_id,
            region,
        }))
    }

    /// The search type and l10n for a locale (upstream language_map).
    fn language(locale: &str) -> (String, String) {
        let language = locale_language(locale);
        match language.as_str() {
            "ru" | "be" | "kk" | "uk" => ("SEARCH_TYPE_RU".to_string(), "ru-RU".to_string()),
            "tr" => ("SEARCH_TYPE_TR".to_string(), "tr-TR".to_string()),
            _ => ("SEARCH_TYPE_COM".to_string(), "en-US".to_string()),
        }
    }

    fn is_base64(value: &str) -> bool {
        !value.is_empty()
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
            })
    }
}

impl Engine for YandexApi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let api_key = self
            .meta
            .api_key
            .as_deref()
            .ok_or_else(|| EngineError::Message("yandex_api requires an api_key".into()))?;
        if pageno > 25 || query.query_text().chars().count() > 400 {
            return Err(EngineError::Message(
                "yandex_api cannot serve this page or query length".into(),
            ));
        }
        let (search_type, l10n) = Self::language(&query.locale);
        let family_mode = match query.safesearch {
            2 => "FAMILY_MODE_STRICT",
            1 => "FAMILY_MODE_MODERATE",
            _ => "FAMILY_MODE_NONE",
        };
        let mut body = serde_json::json!({
            "query": {
                "searchType": search_type,
                "queryText": query.query_text(),
                "familyMode": family_mode,
                "page": (pageno.saturating_sub(1)).to_string(),
            },
            "groupSpec": {
                "groupMode": "GROUP_MODE_FLAT",
                "groupsOnPage": PAGE_SIZE.to_string(),
                "docsInGroup": "1",
            },
            "l10n": l10n,
            "folderId": self.folder_id,
            "responseFormat": "FORMAT_XML",
        });
        // Yandex accepts a region only together with the Russian search type.
        if !self.region.is_empty() && search_type == "SEARCH_TYPE_RU" {
            body["region"] = serde_json::json!(self.region);
        }
        Ok(EngineRequest::post(API_URL)
            .json_body(body)
            .header("Content-Type", "application/json")
            .header("Authorization", &format!("Api-Key {api_key}")))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let raw = json_str(&json, "rawData");
        if raw.is_empty() || !Self::is_base64(&raw) {
            return Err(EngineError::Parse("yandex_api: missing rawData".into()));
        }
        let Some(xml) = decode_base64(&raw) else {
            return Err(EngineError::Parse(
                "yandex_api: rawData is not base64".into(),
            ));
        };
        let document = String::from_utf8_lossy(&xml).to_string();
        if document.contains("<error code=\"15\"") {
            // code 15 means "nothing found", not an error
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        // The payload is a flat XML document; split it into doc blocks without a
        // full XML dependency.
        let mut rest = document.as_str();
        while let Some(start) = rest.find("<doc") {
            let Some(end) = rest[start..].find("</doc>") else {
                break;
            };
            let block = &rest[start..start + end];
            rest = &rest[start + end + "</doc>".len()..];
            let url = xml_element(block, "url");
            let title = xml_element(block, "title");
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let mut content = xml_element(block, "headline");
            if content.is_empty() {
                content = xml_element(block, "passages");
            }
            results.push(
                ResultBuilder::new(url, strip_tags(&title))
                    .content(strip_tags(&content))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The text of the first child element (namespaced tags included).
fn xml_element(block: &str, name: &str) -> String {
    let open = format!("<{name}");
    let Some(start) = block.find(&open) else {
        return String::new();
    };
    let Some(open_end) = block[start..].find('>') else {
        return String::new();
    };
    let content_start = start + open_end + 1;
    let close = format!("</{name}>");
    let Some(length) = block[content_start..].find(&close) else {
        return String::new();
    };
    decode_entities(&block[content_start..content_start + length])
}

/// Minimal standard base64 decoder (RFC 4648, padding optional).
fn base64_decode_bytes(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for character in value.chars() {
        let digit = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            '=' => break,
            other if other.is_whitespace() => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<YandexApi> {
        let mut config = EngineConfig::new("yandex_api", "yandex_api", "yandex", &["general"]);
        config.api_key = Some("KEY".to_string());
        config
            .extra
            .insert("folder_id".into(), toml::Value::String("folder".into()));
        YandexApi::from_config(&config).expect("engine")
    }

    #[test]
    fn requires_a_folder_id() {
        let config = EngineConfig::new("yandex_api", "yandex_api", "yandex", &["general"]);
        assert!(YandexApi::from_config(&config).is_err());
    }

    #[test]
    fn posts_the_query_to_the_api() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.url, API_URL);
        let body = request.json.clone().unwrap();
        assert_eq!(body["query"]["queryText"], serde_json::json!("rust"));
        assert_eq!(body["folderId"], serde_json::json!("folder"));
    }

    #[test]
    fn parses_the_xml_payload() {
        let xml = "<response><results><grouping><group><doc><url>https://example.test/a</url><title>Title A</title><headline>Snippet A</headline></doc></group></grouping></results></response>";
        let encoded = "PFJlc3BvbnNlPjwvcmVzcG9uc2U+"; // sanity: valid base64
        assert!(base64_decode_bytes(encoded).is_some());
        let body = serde_json::json!({ "rawData": base64_encode(xml.as_bytes()) }).to_string();
        let response = EngineResponse::new(200, API_URL, body.into_bytes());
        let query = Query::parse("a", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/a");
                assert_eq!(result.title, "Title A");
                assert_eq!(result.content, "Snippet A");
            }
            _ => panic!("expected a result"),
        }
    }

    /// The encoder used only by tests (keeps the fixture readable).
    fn base64_encode(input: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let bytes = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let value =
                (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    let shift = 18 - index * 6;
                    out.push(char::from(ALPHABET[((value >> shift) & 0x3f) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

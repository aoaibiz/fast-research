//! eBay engine, ported from searx/engines/ebay.py.
//!
//! Upstream renders its results with the products.html template, which this
//! port does not have; the price, the shipping cost and the item location are
//! kept in the result metadata instead. Upstream also sends the page number in
//! the _sacat parameter (eBay's category id), this port uses _pgn so that
//! paging actually works.
// engine-id: ebay

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["shopping"],
    shortcut: "eb",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

const BASE_URL: &str = "https://www.ebay.com";

/// eBay.
pub struct Ebay {
    meta: EngineMeta,
}

impl Ebay {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Ebay {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("_nkw", query.query_text()),
            ("_pgn", pageno.max(1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/sch/i.html?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, "li.s-item")? {
            let title = text_of(element, "h3.s-item__title")
                .or_else(|| text_of(element, "div.s-item__title"))
                .unwrap_or_default();
            let href = attr_of(element, "a.s-item__link", "href").unwrap_or_default();
            let url = absolute_url(&href, &base).unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let content = text_of(element, "div.s-item__subtitle")
                .or_else(|| text_of(element, "span.SECONDARY_INFO"))
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .metadata(
                    "price",
                    text_of(element, "span.s-item__price").unwrap_or_default(),
                )
                .metadata(
                    "shipping",
                    text_of(element, "span.s-item__shipping").unwrap_or_default(),
                )
                .metadata(
                    "source_country",
                    text_of(element, "span.s-item__location").unwrap_or_default(),
                );
            let thumbnail = attr_of(element, "img.s-item__image-img", "src").unwrap_or_default();
            if !thumbnail.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body><ul>
      <li class="s-item s-item__pl-on-bottom">
        <a class="s-item__link" href="https://www.ebay.com/itm/123456">
          <div class="s-item__image-section">
            <img class="s-item__image-img" src="https://i.ebayimg.com/images/g/abc/s-l225.jpg">
          </div>
          <div class="s-item__info">
            <div class="s-item__title"><span role="heading">Vintage camera lens</span></div>
            <div class="s-item__subtitle"><span class="SECONDARY_INFO">Pre-owned</span></div>
            <div class="s-item__details">
              <span class="s-item__price">$19.99</span>
              <span class="s-item__shipping s-item__logisticsCost">+$5.00 shipping</span>
              <span class="s-item__location s-item__itemLocation">from Japan</span>
            </div>
          </div>
        </a>
      </li>
      <li class="s-item">
        <a class="s-item__link" href="https://www.ebay.com/itm/999">
          <div class="s-item__title"></div>
        </a>
      </li>
    </ul></body></html>"#;

    fn engine() -> Arc<Ebay> {
        Ebay::from_config(&EngineConfig::new("ebay", "ebay", "eb", &["shopping"])).unwrap()
    }

    #[test]
    fn parses_item_rows() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("lens", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Vintage camera lens");
                assert_eq!(result.url, "https://www.ebay.com/itm/123456");
                assert_eq!(result.content, "Pre-owned");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.ebayimg.com/images/g/abc/s-l225.jpg")
                );
                assert_eq!(
                    result.metadata.get("price").map(|value| value.as_str()),
                    Some("$19.99")
                );
                assert_eq!(
                    result.metadata.get("shipping").map(|value| value.as_str()),
                    Some("+$5.00 shipping")
                );
                assert_eq!(
                    result
                        .metadata
                        .get("source_country")
                        .map(|value| value.as_str()),
                    Some("from Japan")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse(
            "camera lens",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.ebay.com/sch/i.html?_nkw=camera%20lens&_pgn=2"
        );
    }
}

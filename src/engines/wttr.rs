//! wttr.in engine, ported from searx/engines/wttr.py.
//!
//! Upstream answers a WeatherAnswer with the current conditions and the hourly
//! forecasts. EngineItem only carries a plain answer text here, so the answer
//! sums up the current conditions and the daily range.
// engine-id: wttr

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["weather"],
    shortcut: "wttr",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 9.0,
};

const BASE_URL: &str = "https://wttr.in";
const COMPASS_POINTS: [&str; 16] = [
    "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW",
    "NNW",
];

/// wttr.in, the weather forecast service.
pub struct Wttr {
    meta: EngineMeta,
}

impl Wttr {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

/// The WWO weather code map of upstream.
fn condition(code: &str) -> &'static str {
    match code {
        "113" => "clear sky",
        "116" => "partly cloudy",
        "119" => "cloudy",
        "122" | "143" => "fair",
        "176" | "263" | "266" | "293" | "353" => "light rain showers",
        "179" | "284" | "323" | "326" | "368" => "light snow showers",
        "182" | "281" | "362" | "374" => "light sleet showers",
        "185" | "311" | "317" | "350" => "light sleet",
        "200" => "rain and thunder",
        "227" => "light snow",
        "230" | "332" | "338" => "heavy snow",
        "248" | "260" => "fog",
        "296" => "light rain",
        "299" => "rain showers",
        "302" => "rain",
        "305" | "356" => "heavy rain showers",
        "308" | "359" => "heavy rain",
        "314" => "sleet",
        "320" | "377" => "heavy sleet",
        "329" | "335" | "371" | "395" => "heavy snow showers",
        "365" => "sleet showers",
        "386" => "rain showers and thunder",
        "389" => "heavy rain showers and thunder",
        "392" => "snow showers and thunder",
        _ => "unknown",
    }
}

/// The compass point of an azimuth in degrees.
fn compass(degrees: &str) -> Option<&'static str> {
    let value: f64 = degrees.trim().parse().ok()?;
    let sector = (value % 360.0 + 360.0) % 360.0 / 22.5;
    Some(COMPASS_POINTS[(sector.round() as usize) % 16])
}

impl Engine for Wttr {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let language = locale_language(&query.engine_language());
        Ok(EngineRequest::get(format!(
            "{base}/{}?format=j1&lang={}",
            encode(&query.query_text()),
            encode(&language)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if response.status == 404 {
            return Ok(Vec::new());
        }
        let json = response.json()?;
        let Some(current) = json_path(&json, "current_condition.0") else {
            return Ok(Vec::new());
        };
        let mut temperature = json_str(current, "temp_C");
        if temperature.is_empty() {
            temperature = json_str(current, "tempC");
        }
        if temperature.is_empty() {
            return Ok(Vec::new());
        }
        let location = self
            .meta
            .opt_str("location")
            .map(str::to_string)
            .unwrap_or_else(|| query.query_text());
        let location = location.trim();
        let location = if location.is_empty() {
            "wttr.in"
        } else {
            location
        };
        let mut parts = vec![
            format!("{location}: {temperature} °C"),
            condition(&json_str(current, "weatherCode")).to_string(),
        ];
        let feels = json_str(current, "FeelsLikeC");
        if !feels.is_empty() {
            parts.push(format!("feels like {feels} °C"));
        }
        let speed = json_str(current, "windspeedKmph");
        if !speed.is_empty() {
            match compass(&json_str(current, "winddirDegree")) {
                Some(direction) => parts.push(format!("wind {speed} km/h from {direction}")),
                None => parts.push(format!("wind {speed} km/h")),
            }
        }
        let pressure = json_str(current, "pressure");
        if !pressure.is_empty() {
            parts.push(format!("pressure {pressure} hPa"));
        }
        let humidity = json_str(current, "humidity");
        if !humidity.is_empty() {
            parts.push(format!("humidity {humidity} %"));
        }
        let cloud_cover = json_str(current, "cloudcover");
        if !cloud_cover.is_empty() {
            parts.push(format!("cloud cover {cloud_cover} %"));
        }
        let low = json_str(&json, "weather.0.mintempC");
        let high = json_str(&json, "weather.0.maxtempC");
        if !low.is_empty() && !high.is_empty() {
            parts.push(format!("today {low} to {high} °C"));
        }
        Ok(vec![EngineItem::Answer(parts.join(", "))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "current_condition": [{
        "temp_C": "12",
        "FeelsLikeC": "10",
        "weatherCode": "113",
        "winddirDegree": "270",
        "windspeedKmph": "15",
        "pressure": "1015",
        "humidity": "60",
        "cloudcover": "0"
      }],
      "weather": [{"mintempC": "7", "maxtempC": "16", "hourly": [{}]}]
    }"##;

    fn engine() -> Arc<Wttr> {
        let mut config = EngineConfig::new("wttr.in", "wttr", "wttr", &["weather"]);
        config.extra.insert(
            "location".to_string(),
            toml::Value::String("Berlin".to_string()),
        );
        Wttr::from_config(&config).expect("wttr engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn answers_the_current_conditions() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("berlin")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Answer(answer) => assert_eq!(
                answer,
                "Berlin: 12 °C, clear sky, feels like 10 °C, wind 15 km/h from W, \
                 pressure 1015 hPa, humidity 60 %, cloud cover 0 %, today 7 to 16 °C"
            ),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn builds_the_json_url() {
        let mut query = query("new york");
        query.locale = "ja-JP".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.url, "https://wttr.in/new%20york?format=j1&lang=ja");
    }

    #[test]
    fn a_missing_container_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(
            engine()
                .parse(&response, &query("berlin"))
                .unwrap()
                .is_empty()
        );
        let response = EngineResponse::new(404, BASE_URL, b"{}".to_vec());
        assert!(
            engine()
                .parse(&response, &query("berlin"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(compass("0"), Some("N"));
        assert_eq!(compass("180"), Some("S"));
        assert_eq!(compass("not a number"), None);
    }
}

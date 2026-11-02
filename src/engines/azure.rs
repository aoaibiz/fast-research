//! Azure engine, ported from searx/engines/azure.py.
// engine-id: azure

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "cloud"],
    shortcut: "az",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BATCH_ENDPOINT: &str = "https://management.azure.com/batch?api-version=2020-06-01";
const PORTAL: &str = "https://portal.azure.com/#@/resource";
const TAKE: u32 = 30;

/// Azure resources and resource groups, like the search bar of the portal.
pub struct Azure {
    meta: EngineMeta,
}

impl Azure {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let mut missing = Vec::new();
        for option in ["azure_tenant_id", "azure_client_id", "azure_client_secret"] {
            if config
                .opt_str(option)
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
            {
                missing.push(option);
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "missing values for options: {}",
                missing.join(", ")
            ));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn token(&self) -> Result<&str, EngineError> {
        self.meta
            .api_key
            .as_deref()
            .or_else(|| self.meta.opt_str("azure_access_token"))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| EngineError::Message("azure requires an access token".into()))
    }
}

fn batch_body(search: &str) -> serde_json::Value {
    let search = search.replace('\'', "\\'");
    let resource_group_query = format!(
        "ResourceContainers | where (name contains ('{search}')) | where (type =~ ('Microsoft.Resources/subscriptions/resourcegroups')) | project id,name,type,kind,subscriptionId,resourceGroup | extend matchscore = name startswith '{search}' | extend normalizedName = tolower(tostring(name)) | sort by matchscore desc, normalizedName asc | take {TAKE}"
    );
    let details = serde_json::json!({"commandName": "Microsoft.ResourceGraph"});
    let url = "/providers/Microsoft.ResourceGraph/resources?api-version=2024-04-01";
    serde_json::json!({
        "requests": [
            {"url": url, "httpMethod": "POST", "name": "resourceGroups",
             "requestHeaderDetails": details, "content": {"query": resource_group_query}},
            {"url": url, "httpMethod": "POST", "name": "resources", "requestHeaderDetails": details,
             "content": {"query": format!("Resources | where name contains '{search}' | take {TAKE}")}},
        ]
    })
}

fn resource_group_item(name: &str, subscription: &str) -> EngineItem {
    let url = format!("{PORTAL}/subscriptions/{subscription}/resourceGroups/{name}/overview");
    ResultBuilder::new(url, name)
        .content(format!("Resource Group in Subscription: {subscription}"))
        .build()
}

fn resource_item(item: &serde_json::Value, name: &str, subscription: &str) -> EngineItem {
    let resource_group = json_str(item, "resourceGroup");
    let kind = json_str(item, "type");
    let url = format!(
        "{PORTAL}/subscriptions/{subscription}/resourceGroups/{resource_group}/providers/{kind}/{name}/overview"
    );
    let content = format!(
        "Resource of type {kind} in Subscription: {subscription}, Resource Group: {resource_group}"
    );
    ResultBuilder::new(url, name).content(content).build()
}

impl Engine for Azure {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let token = self.token()?;
        let endpoint = self
            .meta
            .opt_str("azure_batch_endpoint")
            .unwrap_or(BATCH_ENDPOINT);
        Ok(EngineRequest::post(endpoint)
            .header("Authorization", &format!("Bearer {token}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json_body(batch_body(&query.query_text())))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(entries) = json_path(&json, "responses").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let kind = json_str(entry, "name");
            let Some(items) = json_path(entry, "content.data").and_then(|value| value.as_array())
            else {
                continue;
            };
            for item in items {
                let name = json_str(item, "name");
                let subscription = json_str(item, "subscriptionId");
                if name.is_empty() || subscription.is_empty() {
                    continue;
                }
                if kind == "resourceGroups" {
                    results.push(resource_group_item(&name, &subscription));
                } else if kind == "resources" {
                    results.push(resource_item(item, &name, &subscription));
                }
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_engine(token: Option<&str>) -> Result<Arc<Azure>, String> {
        let mut config = EngineConfig::new("azure", "azure", "az", &["it", "cloud"]);
        for option in ["azure_tenant_id", "azure_client_id", "azure_client_secret"] {
            config
                .extra
                .insert(option.to_string(), toml::Value::String("x".into()));
        }
        if let Some(token) = token {
            config.api_key = Some(token.to_string());
        }
        Azure::from_config(&config)
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    fn result(items: &[EngineItem], index: usize) -> &crate::results::SearchResult {
        match &items[index] {
            EngineItem::Result(result) => result,
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn requires_the_credentials_and_a_token() {
        let config = EngineConfig::new("azure", "azure", "az", &["it", "cloud"]);
        assert!(Azure::from_config(&config).is_err());
        let request = try_engine(Some("TOKEN")).expect("engine");
        let request = request.build_request(&query("my rg"), 1).expect("request");
        assert_eq!(request.url, BATCH_ENDPOINT);
        assert!(matches!(
            try_engine(None)
                .expect("engine")
                .build_request(&query("x"), 1),
            Err(EngineError::Message(_))
        ));
    }

    #[test]
    fn parses_the_batch_answer() {
        let body = r##"{"responses": [{"name": "resourceGroups", "content": {"data": [{"name": "rg-prod", "subscriptionId": "sub-1"}]}}, {"name": "resources", "content": {"data": [{"name": "vm-prod", "subscriptionId": "sub-1", "resourceGroup": "rg-prod", "type": "Microsoft.Compute/virtualMachines"}, {"name": ""}]}}]}"##;
        let response = EngineResponse::new(200, BATCH_ENDPOINT, body.as_bytes().to_vec());
        let engine = try_engine(Some("TOKEN")).expect("engine");
        let items = engine.parse(&response, &query("prod")).expect("items");
        assert_eq!(result(&items, 0).title, "rg-prod");
        assert!(
            result(&items, 1)
                .url
                .contains("/virtualMachines/vm-prod/overview")
        );
        let empty = EngineResponse::new(200, BATCH_ENDPOINT, b"{}".to_vec());
        assert!(
            engine
                .parse(&empty, &query("prod"))
                .expect("items")
                .is_empty()
        );
    }
}

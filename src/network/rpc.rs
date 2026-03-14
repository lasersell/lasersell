use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::{json, Value};

use crate::util::logging::redact_url;

pub async fn rpc_call(client: &Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let endpoint = redact_url(url);
    let resp = client
        .post(url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }))
        .send()
        .await
        .map_err(|err| {
            let kind = if err.is_timeout() {
                "timeout"
            } else if err.is_connect() {
                "connect"
            } else {
                "send"
            };
            anyhow!("rpc request {method} failed ({kind}) to {endpoint}")
        })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|err| {
        let kind = if err.is_timeout() { "timeout" } else { "read" };
        anyhow!("rpc response read failed ({kind}) from {endpoint}")
    })?;
    if !status.is_success() {
        return Err(anyhow!("RPC HTTP {} for {}", status, method));
    }
    let parsed: Value = serde_json::from_str(&body).context("decode rpc response")?;
    if let Some(err) = parsed.get("error") {
        return Err(anyhow!("RPC error: {}", err));
    }
    Ok(parsed)
}

pub async fn rpc_result(client: &Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let parsed = rpc_call(client, url, method, params).await?;
    parsed
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("rpc response missing result"))
}

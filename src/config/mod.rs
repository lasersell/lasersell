use std::env;
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Url;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub account: AccountConfig,
    pub strategy: StrategyConfig,
    #[serde(default)]
    pub sell: SellConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountConfig {
    pub keypair_path: String,
    #[serde(default)]
    pub local: bool,
    #[serde(
        default = "default_secret_string",
        deserialize_with = "deserialize_secret_string",
        serialize_with = "serialize_secret_string"
    )]
    pub rpc_url: SecretString,
    #[serde(
        default = "default_secret_string",
        deserialize_with = "deserialize_secret_string",
        serialize_with = "serialize_secret_string"
    )]
    pub api_key: SecretString,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyConfig {
    pub target_profit: StrategyAmount,
    /// Positive amount; if percent, it's based on detected buy amount.
    pub stop_loss: StrategyAmount,
    #[serde(rename = "deadline_timeout")]
    pub deadline_timeout_sec: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SellConfig {
    #[serde(default = "default_slippage_pad")]
    pub slippage_pad_bps: u16,
    #[serde(default = "default_slippage_retry_bump_first")]
    pub slippage_retry_bump_bps_first: u16,
    #[serde(default = "default_slippage_retry_bump_next")]
    pub slippage_retry_bump_bps_next: u16,
    #[serde(default = "default_slippage_max")]
    pub slippage_max_bps: u16,
    #[serde(default = "default_confirm_timeout_sec")]
    pub confirm_timeout_sec: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
}

impl Default for SellConfig {
    fn default() -> Self {
        Self {
            slippage_pad_bps: default_slippage_pad(),
            slippage_retry_bump_bps_first: default_slippage_retry_bump_first(),
            slippage_retry_bump_bps_next: default_slippage_retry_bump_next(),
            slippage_max_bps: default_slippage_max(),
            confirm_timeout_sec: default_confirm_timeout_sec(),
            max_retries: default_max_retries(),
        }
    }
}

fn default_slippage_pad() -> u16 {
    2_000
}

fn default_slippage_retry_bump_first() -> u16 {
    20
}

fn default_slippage_retry_bump_next() -> u16 {
    40
}

fn default_slippage_max() -> u16 {
    2_500
}

fn default_max_retries() -> usize {
    2
}

pub const STREAM_ENDPOINT: &str = "wss://stream.lasersell.io/v1/ws";
pub const EXIT_API_BASE_URL: &str = "https://api.lasersell.io";
pub const LOCAL_STREAM_ENDPOINT: &str = "ws://localhost:8082/v1/ws";
pub const LOCAL_EXIT_API_BASE_URL: &str = "http://localhost:8080";

#[cfg(feature = "devnet")]
fn default_confirm_timeout_sec() -> u64 {
    25
}

#[cfg(not(feature = "devnet"))]
fn default_confirm_timeout_sec() -> u64 {
    10
}

fn default_secret_string() -> SecretString {
    SecretString::new(String::new())
}

fn env_nonempty(var: &str) -> Option<String> {
    match env::var(var) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(_) => None,
    }
}

fn deserialize_secret_string<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(SecretString::new(value))
}

fn serialize_secret_string<S>(value: &SecretString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(value.expose_secret())
}

impl Config {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config file {}", path.display()))?;
        reject_removed_yaml_fields(&raw)?;
        let mut cfg: Config = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse yaml config {}", path.display()))?;
        cfg.apply_env_overrides();
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        let raw = serde_yaml::to_string(self).context("serialize config yaml")?;
        crate::util::fs_utils::atomic_write(path, raw.as_bytes(), Some(0o600))
            .with_context(|| format!("write config file {}", path.display()))?;
        Ok(())
    }

    fn apply_env_overrides(&mut self) {
        if let Some(path) = env_nonempty("LASERSELL_KEYPAIR_PATH") {
            self.account.keypair_path = path;
        }
        if let Some(value) =
            env_nonempty("LASERSELL_RPC_URL").or_else(|| env_nonempty("LASERSELL_PRIVATE_RPC_URL"))
        {
            self.account.rpc_url = SecretString::new(value);
        }
        if let Some(value) = env_nonempty("LASERSELL_API_KEY") {
            self.account.api_key = SecretString::new(value);
        }
    }

    pub fn wallet_pubkey(&self, keypair: &Keypair) -> Result<Pubkey> {
        Ok(keypair.pubkey())
    }

    pub fn http_rpc_url(&self) -> String {
        self.account.rpc_url.expose_secret().trim().to_string()
    }

    pub fn stream_url(&self) -> String {
        if self.account.local {
            LOCAL_STREAM_ENDPOINT.to_string()
        } else {
            STREAM_ENDPOINT.to_string()
        }
    }

    pub fn exit_api_url(&self) -> String {
        if self.account.local {
            LOCAL_EXIT_API_BASE_URL.to_string()
        } else {
            EXIT_API_BASE_URL.to_string()
        }
    }

    pub fn rpc_connect_timeout(&self) -> Duration {
        Duration::from_millis(200)
    }

    pub fn rpc_request_timeout(&self) -> Duration {
        Duration::from_millis(800)
    }

    pub fn exit_api_connect_timeout(&self) -> Duration {
        Duration::from_millis(200)
    }

    pub fn exit_api_request_timeout(&self) -> Duration {
        Duration::from_millis(900)
    }

    pub fn validate(&self) -> Result<()> {
        if self.account.keypair_path.trim().is_empty() {
            return Err(anyhow!("account.keypair_path must not be empty"));
        }
        let raw = self.account.rpc_url.expose_secret().trim();
        if raw.is_empty() {
            return Err(anyhow!("account.rpc_url must not be empty"));
        }
        let url = Url::parse(raw).map_err(|_| anyhow!("account.rpc_url must be a valid URL"))?;
        match url.scheme() {
            "https" => {}
            "http" => {
                let host = url
                    .host_str()
                    .ok_or_else(|| anyhow!("account.rpc_url host is missing"))?;
                if !is_local_or_private_host(host) {
                    return Err(anyhow!(
                        "account.rpc_url http:// is allowed only for localhost/private endpoints"
                    ));
                }
                if !config_warnings_suppressed() {
                    eprintln!(
                        "Warning: account.rpc_url uses http:// for local/private endpoint ({host}); use https:// in production."
                    );
                }
            }
            _ => {
                return Err(anyhow!(
                    "account.rpc_url must start with https:// (or http:// for local/private endpoints)"
                ));
            }
        }
        let stream_url_value = self.stream_url();
        let stream_url = stream_url_value.trim();
        if stream_url.is_empty() {
            return Err(anyhow!("internal stream endpoint must not be empty"));
        }
        let stream = Url::parse(stream_url)
            .map_err(|_| anyhow!("internal stream endpoint must be a valid URL"))?;
        if self.account.local {
            if stream.scheme() != "ws" {
                return Err(anyhow!(
                    "internal local stream endpoint must start with ws://"
                ));
            }
        } else if stream.scheme() != "wss" {
            return Err(anyhow!(
                "internal production stream endpoint must start with wss://"
            ));
        }
        let exit_api_url_value = self.exit_api_url();
        let exit_api_url = exit_api_url_value.trim();
        if exit_api_url.is_empty() {
            return Err(anyhow!("internal exit-api endpoint must not be empty"));
        }
        let api_key = self.account.api_key.expose_secret().trim();
        if api_key.is_empty() {
            return Err(anyhow!("account.api_key must not be empty"));
        }
        let exit_url = Url::parse(exit_api_url)
            .map_err(|_| anyhow!("internal exit-api endpoint must be a valid URL"))?;
        if self.account.local {
            if exit_url.scheme() != "http" {
                return Err(anyhow!(
                    "internal local exit-api endpoint must start with http://"
                ));
            }
        } else if exit_url.scheme() != "https" {
            return Err(anyhow!(
                "internal production exit-api endpoint must start with https://"
            ));
        }
        let _ = self.strategy.target_profit_units(None)?;
        let _ = self.strategy.stop_loss_units(None)?;
        let target_profit_pct = self.strategy.target_profit.percent_value();
        let stop_loss_pct = self.strategy.stop_loss.percent_value();
        let deadline_timeout_sec = self.strategy.deadline_timeout_sec;
        if target_profit_pct <= 0.0 && stop_loss_pct <= 0.0 && deadline_timeout_sec == 0 {
            return Err(anyhow!(
                "at least one of strategy.target_profit, strategy.stop_loss, or strategy.deadline_timeout must be > 0"
            ));
        }
        Ok(())
    }
}

fn reject_removed_yaml_fields(raw: &str) -> Result<()> {
    let parsed: serde_yaml::Value = serde_yaml::from_str(raw).context("parse yaml config")?;
    let Some(root) = parsed.as_mapping() else {
        return Ok(());
    };
    if root.contains_key(serde_yaml::Value::String("services".to_string())) {
        return Err(anyhow!(
            "services section has been removed; stream and exit-api endpoints are fixed in code (set account.local=true for localhost mode) and account.api_key is required"
        ));
    }
    if root.contains_key(serde_yaml::Value::String("rpc".to_string())) {
        return Err(anyhow!(
            "rpc section has been removed; account.rpc_url is required and RPC timeouts are fixed in code"
        ));
    }
    Ok(())
}

fn is_local_or_private_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }

    let Ok(ip) = host.parse::<IpAddr>() else {
        return false;
    };

    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    }
}

fn config_warnings_suppressed() -> bool {
    env_nonempty("LASERSELL_SUPPRESS_CONFIG_WARNINGS")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

impl StrategyConfig {
    pub fn target_profit_units(&self, buy_quote_amount: Option<u64>) -> Result<Option<u64>> {
        self.target_profit
            .to_base_units(buy_quote_amount, "strategy.target_profit")
    }

    pub fn stop_loss_units(&self, buy_quote_amount: Option<u64>) -> Result<Option<u64>> {
        self.stop_loss
            .to_base_units(buy_quote_amount, "strategy.stop_loss")
    }
}

#[derive(Clone, Debug)]
pub enum StrategyAmount {
    Percent(f64),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StrategyAmountInput {
    Number(f64),
    String(String),
}

impl<'de> Deserialize<'de> for StrategyAmount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = StrategyAmountInput::deserialize(deserializer)?;
        match input {
            StrategyAmountInput::Number(value) => {
                if value == 0.0 {
                    Ok(StrategyAmount::Percent(0.0))
                } else {
                    Err(serde::de::Error::custom(
                        "strategy amount must be a percent string like \"10%\"",
                    ))
                }
            }
            StrategyAmountInput::String(value) => {
                parse_strategy_amount_str(&value).map_err(serde::de::Error::custom)
            }
        }
    }
}

impl Serialize for StrategyAmount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            StrategyAmount::Percent(value) => serializer.serialize_str(&format!("{value}%")),
        }
    }
}

impl StrategyAmount {
    pub fn parse_str(raw: &str) -> Result<Self> {
        parse_strategy_amount_str(raw)
    }

    pub fn percent_value(&self) -> f64 {
        match *self {
            StrategyAmount::Percent(value) => value,
        }
    }

    pub fn to_base_units(&self, buy_quote_amount: Option<u64>, field: &str) -> Result<Option<u64>> {
        self.validate(field)?;
        let Some(buy) = buy_quote_amount else {
            return Ok(None);
        };
        Ok(Some(percent_to_base_units(buy, self.value(), field)?))
    }

    fn value(&self) -> f64 {
        match *self {
            StrategyAmount::Percent(value) => value,
        }
    }

    fn validate(&self, field: &str) -> Result<()> {
        let value = self.value();
        if !value.is_finite() {
            return Err(anyhow!("{field} must be a finite number"));
        }
        if value < 0.0 {
            return Err(anyhow!("{field} must be >= 0"));
        }
        Ok(())
    }
}

fn percent_to_base_units(buy_quote_amount: u64, pct: f64, field: &str) -> Result<u64> {
    if !pct.is_finite() {
        return Err(anyhow!("{field} must be a finite number"));
    }
    if pct < 0.0 {
        return Err(anyhow!("{field} must be >= 0"));
    }
    let units = (buy_quote_amount as f64) * (pct / 100.0);
    if units > (u64::MAX as f64) {
        return Err(anyhow!("{field} is too large"));
    }
    Ok(units.round() as u64)
}

fn parse_strategy_amount_str(raw: &str) -> Result<StrategyAmount> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("strategy amount must not be empty"));
    }

    let lowered = trimmed.to_ascii_lowercase();
    if let Some(percent) = lowered.strip_suffix('%') {
        let val = percent
            .trim()
            .parse::<f64>()
            .map_err(|_| anyhow!("invalid percent strategy amount: {raw}"))?;
        return Ok(StrategyAmount::Percent(val));
    }
    Err(anyhow!(
        "strategy amount must be a percent string like \"10%\""
    ))
}

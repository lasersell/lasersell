use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bip39::{Language, Mnemonic, MnemonicType, Seed};
use reqwest::Url;
use secrecy::SecretString;
use serde::Deserialize;
use solana_derivation_path::DerivationPath;
use solana_sdk::signature::{keypair_from_seed_and_derivation_path, read_keypair_file, Keypair};
use solana_sdk::signer::Signer;
use zeroize::{Zeroize, Zeroizing};

use crate::config::{AccountConfig, Config, SellConfig, StrategyAmount, StrategyConfig};
use crate::ui::format::{format_bps_percent, parse_percent_to_bps};
use crate::util::support;
use crate::wallet;

const SOLANA_DERIVATION_PATH: &str = "m/44'/501'/0'/0'";
const DEFAULT_RPC_URL: &str = "https://api.mainnet-beta.solana.com";

fn default_keystore_path_for_config(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("wallet.keystore.json")
}

#[derive(Deserialize)]
struct PartialConfig {
    account: Option<PartialAccount>,
}

#[derive(Deserialize)]
struct PartialAccount {
    keypair_path: Option<String>,
}

fn resolve_keypair_path_from_config(config_path: &Path, raw: &str) -> PathBuf {
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        candidate
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(candidate)
    }
}

fn find_existing_encrypted_keystore(config_path: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if config_path.is_file() {
        if let Ok(raw) = fs::read_to_string(config_path) {
            if let Ok(cfg) = serde_yaml::from_str::<PartialConfig>(&raw) {
                if let Some(account) = cfg.account {
                    if let Some(keypair_path) = account.keypair_path {
                        let trimmed = keypair_path.trim();
                        if !trimmed.is_empty() {
                            candidates.push(resolve_keypair_path_from_config(config_path, trimmed));
                        }
                    }
                }
            }
        }
    }

    candidates.push(default_keystore_path_for_config(config_path));

    for candidate in candidates {
        if candidate.is_file() {
            if let Ok(kind) = wallet::detect_wallet_file_kind(&candidate) {
                if kind == wallet::WalletFileKind::EncryptedKeystore {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportMode {
    SeedPhrase,
    KeypairJson,
    Base58Secret,
}

#[derive(Debug)]
struct WalletSelection {
    keypair: Keypair,
    delete_after_import: Option<PathBuf>,
}

#[derive(Debug)]
enum WalletPlan {
    ReuseExistingKeystore { path: PathBuf },
    NewWallet { selection: WalletSelection },
}

#[derive(Debug, Clone)]
struct StrategyInputs {
    target_profit: StrategyAmount,
    target_profit_enabled: bool,
    stop_loss: StrategyAmount,
    stop_loss_enabled: bool,
    trailing_stop: StrategyAmount,
    trailing_stop_enabled: bool,
    sell_timeout_sec: u64,
    timeout_enabled: bool,
    slippage_max_bps: u16,
    sell_on_graduation: bool,
}

#[derive(Debug, Clone)]
struct ConfigInputs {
    rpc_url: String,
    api_key: String,
    local: bool,
    target_profit: StrategyAmount,
    target_profit_enabled: bool,
    stop_loss: StrategyAmount,
    stop_loss_enabled: bool,
    trailing_stop: StrategyAmount,
    trailing_stop_enabled: bool,
    sell_timeout_sec: u64,
    timeout_enabled: bool,
    slippage_max_bps: u16,
    sell_on_graduation: bool,
}

pub fn run_onboarding(config_path: &Path) -> Result<(Config, Keypair)> {
    match run_onboarding_inner(config_path) {
        Ok(result) => Ok(result),
        Err(err) => {
            let _ = cliclack::outro_cancel("Setup cancelled.");
            Err(err)
        }
    }
}

fn run_onboarding_inner(config_path: &Path) -> Result<(Config, Keypair)> {
    cliclack::clear_screen()?;
    cliclack::intro("LaserSell Setup")?;

    let use_recommended: bool = cliclack::confirm("Use recommended settings? (skips strategy + file path questions)")
        .initial_value(true)
        .interact()?;

    // ── Credentials & Network ──────────────────────────────────────────
    cliclack::log::step("Credentials & Network")?;

    let rpc_url = prompt_rpc_url()?;
    let api_key = prompt_api_key()?;

    // ── Strategy ───────────────────────────────────────────────────────
    cliclack::log::step("Strategy")?;

    let StrategyInputs {
        target_profit,
        target_profit_enabled,
        stop_loss,
        stop_loss_enabled,
        trailing_stop,
        trailing_stop_enabled,
        sell_timeout_sec,
        timeout_enabled,
        slippage_max_bps,
        sell_on_graduation,
    } = if use_recommended {
        let defaults = StrategyInputs {
            target_profit: StrategyAmount::Percent(6.0),
            target_profit_enabled: true,
            stop_loss: StrategyAmount::Percent(10.0),
            stop_loss_enabled: true,
            trailing_stop: StrategyAmount::Percent(0.0),
            trailing_stop_enabled: false,
            sell_timeout_sec: 120,
            timeout_enabled: true,
            slippage_max_bps: 2000,
            sell_on_graduation: false,
        };
        let slippage_label = format_bps_percent(defaults.slippage_max_bps);
        let target_profit_label = format_strategy_amount(&defaults.target_profit);
        let stop_loss_label = format_strategy_amount(&defaults.stop_loss);
        let deadline_label = format!("{}s", defaults.sell_timeout_sec);
        cliclack::log::success(format!(
            "Applied recommended defaults: Take Profit {target_profit_label} | Stop Loss {stop_loss_label} | Deadline {deadline_label} | Slippage {slippage_label}"
        ))?;
        defaults
    } else {
        prompt_strategy_inputs()?
    };

    // ── Wallet ─────────────────────────────────────────────────────────
    cliclack::log::step("Wallet")?;

    let wallet_plan = if let Some(path) = find_existing_encrypted_keystore(config_path) {
        cliclack::log::success(format!(
            "Existing keystore detected: {}",
            path.display()
        ))?;
        let reuse: bool = cliclack::confirm("Use existing wallet and keep the current keystore file?")
            .initial_value(true)
            .interact()?;
        if reuse {
            WalletPlan::ReuseExistingKeystore { path }
        } else {
            WalletPlan::NewWallet {
                selection: prompt_wallet()?,
            }
        }
    } else {
        WalletPlan::NewWallet {
            selection: prompt_wallet()?,
        }
    };

    let reuse_existing_keystore = matches!(&wallet_plan, WalletPlan::ReuseExistingKeystore { .. });

    // ── Security ───────────────────────────────────────────────────────
    cliclack::log::step("Security")?;

    let (keypair, delete_after_import, mut passphrase, existing_keystore_path) = match wallet_plan {
        WalletPlan::ReuseExistingKeystore { path } => {
            cliclack::log::info("Unlock your existing wallet keystore.")?;
            let keypair = loop {
                let passphrase = Zeroizing::new(
                    cliclack::password("Keystore passphrase")
                        .mask('*')
                        .interact()?,
                );
                if passphrase.trim().is_empty() {
                    cliclack::log::warning("Passphrase cannot be empty.")?;
                    continue;
                }
                let result = wallet::load_keypair_from_path(&path, || {
                    Ok(SecretString::new(passphrase.to_string()))
                });
                match result {
                    Ok(keypair) => break keypair,
                    Err(err) => {
                        let msg = support::with_support_hint(format!("Failed to unlock keystore: {err}"));
                        cliclack::log::error(msg)?;
                    }
                }
            };
            (keypair, None, None, Some(path))
        }
        WalletPlan::NewWallet { selection } => {
            cliclack::log::info("Set a password to encrypt your wallet.")?;
            let passphrase = prompt_passphrase()?;
            (
                selection.keypair,
                selection.delete_after_import,
                Some(passphrase),
                None,
            )
        }
    };
    let pubkey = keypair.pubkey();

    // ── Save Configuration ─────────────────────────────────────────────
    cliclack::log::step("Save Configuration")?;

    let mut config_path = if use_recommended {
        config_path.to_path_buf()
    } else {
        prompt_path("Config file location", config_path)?
    };
    let mut keystore_path = if reuse_existing_keystore {
        let path =
            existing_keystore_path.ok_or_else(|| anyhow!("existing keystore path missing"))?;
        cliclack::log::success(format!(
            "Reusing existing keystore at {} (no changes will be made to the wallet file).",
            path.display()
        ))?;
        path
    } else if use_recommended {
        default_keystore_path_for_config(&config_path)
    } else {
        prompt_path(
            "Keystore file location",
            &default_keystore_path_for_config(&config_path),
        )?
    };

    let inputs = ConfigInputs {
        rpc_url,
        api_key,
        local: false,
        target_profit,
        target_profit_enabled,
        stop_loss,
        stop_loss_enabled,
        trailing_stop,
        trailing_stop_enabled,
        sell_timeout_sec,
        timeout_enabled,
        slippage_max_bps,
        sell_on_graduation,
    };

    // ── Review ─────────────────────────────────────────────────────────
    cliclack::log::step("Review")?;

    loop {
        let mut config_changed = false;
        if config_path.exists() {
            let overwrite: bool = cliclack::confirm(format!(
                "Config file {} exists. Overwrite?",
                config_path.display()
            ))
            .initial_value(false)
            .interact()?;
            if !overwrite {
                if use_recommended {
                    return Err(anyhow!(
                        "config file {} exists; rerun without recommended settings to choose a different path",
                        config_path.display()
                    ));
                }
                config_path = prompt_path("Config file location", &config_path)?;
                if !reuse_existing_keystore {
                    keystore_path = prompt_path(
                        "Keystore file location",
                        &default_keystore_path_for_config(&config_path),
                    )?;
                }
                config_changed = true;
            }
        }
        if config_changed {
            continue;
        }
        if !reuse_existing_keystore && keystore_path.exists() {
            let overwrite: bool = cliclack::confirm(format!(
                "Keystore file {} exists. Overwrite?",
                keystore_path.display()
            ))
            .initial_value(false)
            .interact()?;
            if !overwrite {
                if use_recommended {
                    return Err(anyhow!(
                        "keystore file {} exists; rerun without recommended settings to choose a different path",
                        keystore_path.display()
                    ));
                }
                keystore_path = prompt_path("Keystore file location", &keystore_path)?;
                continue;
            }
        }
        break;
    }

    let config = build_config(&inputs, &keystore_path)?;
    config.validate()?;

    let summary = build_summary_text(&config_path, &keystore_path, &pubkey, &inputs);
    cliclack::note("Summary", summary)?;

    let write_now: bool = cliclack::confirm("Write configuration now?")
        .initial_value(use_recommended)
        .interact()?;
    if !write_now {
        return Err(anyhow!("onboarding cancelled"));
    }

    let spinner = cliclack::spinner();
    spinner.start("Writing configuration...");

    if reuse_existing_keystore {
        config.write_to_path(&config_path)?;
    } else {
        let mut passphrase = passphrase
            .take()
            .ok_or_else(|| anyhow!("missing keystore passphrase"))?;
        let passphrase_secret = SecretString::new(passphrase.to_string());
        wallet::write_keystore(&keystore_path, &keypair, &passphrase_secret)?;
        config.write_to_path(&config_path)?;
        drop(passphrase_secret);
        passphrase.zeroize();
    }

    if let Some(path) = delete_after_import {
        if let Err(err) = fs::remove_file(&path) {
            spinner.stop("Configuration written.");
            cliclack::log::warning(format!(
                "Failed to delete keypair JSON file {}: {err}",
                path.display()
            ))?;
            cliclack::outro("Setup complete! Run lasersell to start.")?;
            return Ok((config, keypair));
        }
    }

    spinner.stop("Configuration written.");
    cliclack::outro("Setup complete! Run lasersell to start.")?;
    Ok((config, keypair))
}

fn prompt_wallet() -> Result<WalletSelection> {
    let import: bool = cliclack::confirm("Import an existing wallet?")
        .initial_value(false)
        .interact()?;
    if import {
        prompt_import_wallet()
    } else {
        prompt_create_wallet()
    }
}

fn prompt_create_wallet() -> Result<WalletSelection> {
    let spinner = cliclack::spinner();
    spinner.start("Generating wallet...");
    let (mnemonic, keypair) = generate_new_wallet()?;
    spinner.stop("Wallet generated.");

    cliclack::note(
        "Seed Phrase",
        format!(
            "{}\n\nWallet pubkey: {}\n\nSave this seed phrase now! You will not see it again.",
            mnemonic.as_str(),
            keypair.pubkey()
        ),
    )?;

    loop {
        let confirmed: bool = cliclack::confirm("I have saved this seed phrase. Continue?")
            .initial_value(false)
            .interact()?;
        if confirmed {
            return Ok(WalletSelection {
                keypair,
                delete_after_import: None,
            });
        }
        cliclack::log::warning("Please save the seed phrase shown above before continuing.")?;
    }
}

fn prompt_import_wallet() -> Result<WalletSelection> {
    loop {
        let mode: ImportMode = cliclack::select("Select import method")
            .item(ImportMode::Base58Secret, "Private key (base58)", "Paste a base58-encoded secret key")
            .item(ImportMode::SeedPhrase, "Seed phrase", "12 or 24 word mnemonic")
            .item(ImportMode::KeypairJson, "Solana keypair JSON", "Path to a JSON keypair file")
            .interact()?;

        let (keypair, delete_after_import) = match mode {
            ImportMode::SeedPhrase => (prompt_seed_phrase_keypair()?, None),
            ImportMode::KeypairJson => {
                let (keypair, path) = prompt_keypair_json()?;
                (keypair, Some(path))
            }
            ImportMode::Base58Secret => (prompt_base58_keypair()?, None),
        };
        cliclack::log::info(format!("Derived wallet pubkey: {}", keypair.pubkey()))?;

        let use_wallet: bool = cliclack::confirm("Use this wallet?")
            .initial_value(true)
            .interact()?;
        if use_wallet {
            let delete_after_import = if let Some(path) = delete_after_import {
                let delete: bool = cliclack::confirm(
                    "Delete the plaintext keypair JSON file after it's imported?",
                )
                .initial_value(false)
                .interact()?;
                if delete { Some(path) } else { None }
            } else {
                None
            };
            return Ok(WalletSelection {
                keypair,
                delete_after_import,
            });
        }
        cliclack::log::warning("Let's try again.")?;
    }
}

fn prompt_seed_phrase_keypair() -> Result<Keypair> {
    loop {
        let phrase = Zeroizing::new(
            cliclack::password("Seed phrase")
                .mask('*')
                .interact()?,
        );
        match derive_keypair_from_mnemonic(phrase.as_str()) {
            Ok(keypair) => return Ok(keypair),
            Err(err) => {
                let msg = support::with_support_hint(format!("Invalid seed phrase: {err}"));
                cliclack::log::error(msg)?;
            }
        }
    }
}

fn prompt_keypair_json() -> Result<(Keypair, PathBuf)> {
    loop {
        let raw: String = cliclack::input("Solana keypair JSON path")
            .interact()?;
        let path = PathBuf::from(raw.trim());
        match read_keypair_file(&path) {
            Ok(keypair) => return Ok((keypair, path)),
            Err(err) => {
                let msg = support::with_support_hint(format!("Failed to read keypair: {err}"));
                cliclack::log::error(msg)?;
            }
        }
    }
}

fn prompt_base58_keypair() -> Result<Keypair> {
    loop {
        let raw = Zeroizing::new(
            cliclack::password("Base58 secret key")
                .mask('*')
                .interact()?,
        );
        match bs58::decode(raw.trim()).into_vec() {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                match Keypair::try_from(bytes.as_slice()) {
                    Ok(keypair) => return Ok(keypair),
                    Err(err) => {
                        let msg = support::with_support_hint(format!("Invalid key bytes: {err}"));
                        cliclack::log::error(msg)?;
                    }
                }
            }
            Err(err) => {
                let msg = support::with_support_hint(format!("Invalid base58 key: {err}"));
                cliclack::log::error(msg)?;
            }
        }
    }
}

fn prompt_passphrase() -> Result<Zeroizing<String>> {
    loop {
        let passphrase = Zeroizing::new(
            cliclack::password("Keystore passphrase")
                .mask('*')
                .interact()?,
        );
        if passphrase.trim().is_empty() {
            cliclack::log::warning("Passphrase cannot be empty.")?;
            continue;
        }
        let confirm = Zeroizing::new(
            cliclack::password("Confirm passphrase")
                .mask('*')
                .interact()?,
        );
        if passphrase.as_str() != confirm.as_str() {
            cliclack::log::warning("Passphrases do not match. Try again.")?;
            continue;
        }
        return Ok(passphrase);
    }
}

fn prompt_rpc_url() -> Result<String> {
    loop {
        let raw: String = cliclack::input("RPC URL (private recommended)")
            .placeholder(DEFAULT_RPC_URL)
            .required(false)
            .validate(|input: &String| {
                let effective = if input.trim().is_empty() { DEFAULT_RPC_URL } else { input.trim() };
                match Url::parse(effective) {
                    Ok(_) => Ok(()),
                    Err(_) => Err("Must be a valid URL (https://... or http://... for local)"),
                }
            })
            .interact()?;
        let value = if raw.trim().is_empty() {
            DEFAULT_RPC_URL.to_string()
        } else {
            raw.trim().to_string()
        };
        let lowered = value.to_ascii_lowercase();
        let parsed = Url::parse(&value).expect("already validated");
        match lowered.split(':').next() {
            Some("https") => return Ok(value),
            Some("http") => {
                let host = parsed.host_str().unwrap_or_default();
                if is_local_or_private_host(host) {
                    cliclack::log::warning(
                        "Using http:// for local/private RPC. Use https:// in production.",
                    )?;
                    return Ok(value);
                }
                cliclack::log::warning(
                    "http:// is only allowed for localhost/private RPC endpoints.",
                )?;
            }
            _ => {
                cliclack::log::warning(
                    "RPC URL must start with https:// (or http:// for local/private endpoints).",
                )?;
            }
        }
    }
}

fn prompt_api_key() -> Result<String> {
    loop {
        let value = Zeroizing::new(
            cliclack::password("LaserSell API key")
                .mask('*')
                .interact()?,
        );
        let trimmed = value.trim();
        if trimmed.is_empty() {
            cliclack::log::warning("API key cannot be empty.")?;
            continue;
        }
        return Ok(trimmed.to_string());
    }
}

fn prompt_strategy_inputs() -> Result<StrategyInputs> {
    let (target_profit, target_profit_enabled, stop_loss, stop_loss_enabled, trailing_stop, trailing_stop_enabled, sell_timeout_sec, timeout_enabled) = loop {
        let tp_enabled: bool = cliclack::confirm("Enable Target Profit?")
            .initial_value(true)
            .interact()?;
        let tp = if tp_enabled {
            prompt_strategy_amount("Target Profit (% of buy)", "6%", false)?
        } else {
            StrategyAmount::Percent(0.0)
        };

        let sl_enabled: bool = cliclack::confirm("Enable Stop Loss?")
            .initial_value(true)
            .interact()?;
        let sl = if sl_enabled {
            prompt_strategy_amount("Stop Loss (% of buy)", "10%", true)?
        } else {
            StrategyAmount::Percent(0.0)
        };

        let ts_enabled: bool = cliclack::confirm("Enable Trailing Stop?")
            .initial_value(false)
            .interact()?;
        let ts = if ts_enabled {
            prompt_strategy_amount("Trailing Stop (% of buy)", "5%", false)?
        } else {
            StrategyAmount::Percent(0.0)
        };

        let to_enabled: bool = cliclack::confirm("Enable Deadline Timeout?")
            .initial_value(true)
            .interact()?;
        let to_sec = if to_enabled {
            prompt_u64("Deadline Timeout (seconds)", 45)?
        } else {
            0
        };

        if !tp_enabled && !sl_enabled && !ts_enabled && !to_enabled {
            cliclack::log::warning(
                "At least one of Target Profit, Stop Loss, Trailing Stop, or Deadline Timeout must be enabled.",
            )?;
            continue;
        }

        break (tp, tp_enabled, sl, sl_enabled, ts, ts_enabled, to_sec, to_enabled);
    };

    let slippage_max_bps = prompt_slippage_percent("Slippage Tolerance (%)", "20%")?;

    let sell_on_graduation: bool = cliclack::confirm("Auto-sell when token graduates to a new DEX?")
        .initial_value(false)
        .interact()?;

    Ok(StrategyInputs {
        target_profit,
        target_profit_enabled,
        stop_loss,
        stop_loss_enabled,
        trailing_stop,
        trailing_stop_enabled,
        sell_timeout_sec,
        timeout_enabled,
        slippage_max_bps,
        sell_on_graduation,
    })
}

fn prompt_strategy_amount(prompt: &str, default: &str, allow_zero: bool) -> Result<StrategyAmount> {
    loop {
        let default_clone = default.to_string();
        let raw: String = cliclack::input(prompt)
            .placeholder(default)
            .required(false)
            .validate(move |input: &String| {
                let effective = if input.trim().is_empty() { &default_clone } else { input };
                match StrategyAmount::parse_str(effective) {
                    Ok(amount) => {
                        let numeric = match amount {
                            StrategyAmount::Percent(val) => val,
                        };
                        if !numeric.is_finite() {
                            return Err("Must be a finite number.");
                        }
                        if allow_zero {
                            if numeric < 0.0 {
                                return Err("Must be >= 0.");
                            }
                        } else if numeric <= 0.0 {
                            return Err("Must be > 0.");
                        }
                        Ok(())
                    }
                    Err(_) => Err("Invalid amount. Enter a percentage like 6%."),
                }
            })
            .interact()?;
        let value = if raw.trim().is_empty() { default.to_string() } else { raw };
        match StrategyAmount::parse_str(&value) {
            Ok(amount) => return Ok(amount),
            Err(err) => {
                let msg = support::with_support_hint(format!("Invalid strategy amount: {err}"));
                cliclack::log::error(msg)?;
            }
        }
    }
}

fn prompt_slippage_percent(prompt: &str, default: &str) -> Result<u16> {
    loop {
        let default_clone = default.to_string();
        let raw: String = cliclack::input(prompt)
            .placeholder(default)
            .required(false)
            .validate(move |input: &String| {
                let effective = if input.trim().is_empty() { &default_clone } else { input };
                match parse_percent_to_bps(effective, "slippage") {
                    Ok(_) => Ok(()),
                    Err(_) => Err("Invalid percentage. Enter a value like 20%."),
                }
            })
            .interact()?;
        let value = if raw.trim().is_empty() { default.to_string() } else { raw };
        match parse_percent_to_bps(&value, "slippage") {
            Ok(parsed) => return Ok(parsed),
            Err(err) => {
                let msg = support::with_support_hint(format!("Invalid slippage percent: {err}"));
                cliclack::log::error(msg)?;
            }
        }
    }
}

fn prompt_u64(prompt: &str, default: u64) -> Result<u64> {
    let default_str = default.to_string();
    loop {
        let default_clone = default_str.clone();
        let raw: String = cliclack::input(prompt)
            .placeholder(&default_str)
            .required(false)
            .validate(move |input: &String| {
                let effective = if input.trim().is_empty() { &default_clone } else { input };
                match effective.trim().parse::<u64>() {
                    Ok(_) => Ok(()),
                    Err(_) => Err("Must be a whole number."),
                }
            })
            .interact()?;
        let value = if raw.trim().is_empty() { default } else {
            match raw.trim().parse::<u64>() {
                Ok(parsed) => parsed,
                Err(_) => {
                    cliclack::log::warning(format!("{prompt} must be a whole number."))?;
                    continue;
                }
            }
        };
        return Ok(value);
    }
}

fn prompt_path(prompt: &str, default: &Path) -> Result<PathBuf> {
    let default_value = default.to_string_lossy().into_owned();
    let raw: String = cliclack::input(prompt)
        .placeholder(&default_value)
        .required(false)
        .interact()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(default.to_path_buf())
    } else {
        Ok(PathBuf::from(trimmed))
    }
}

fn build_config(inputs: &ConfigInputs, keystore_path: &Path) -> Result<Config> {
    Ok(Config {
        account: AccountConfig {
            keypair_path: keystore_path.to_string_lossy().to_string(),
            local: inputs.local,
            rpc_url: SecretString::new(inputs.rpc_url.clone()),
            api_key: SecretString::new(inputs.api_key.clone()),
            send_target: None,
            astralane_api_key: SecretString::new(String::new()),
        },
        strategy: StrategyConfig {
            target_profit: inputs.target_profit.clone(),
            stop_loss: inputs.stop_loss.clone(),
            trailing_stop: inputs.trailing_stop.clone(),
            deadline_timeout_sec: inputs.sell_timeout_sec,
            sell_on_graduation: inputs.sell_on_graduation,
        },
        sell: SellConfig {
            slippage_max_bps: inputs.slippage_max_bps,
            ..SellConfig::default()
        },
    })
}

fn build_summary_text(
    config_path: &Path,
    keystore_path: &Path,
    pubkey: &solana_sdk::pubkey::Pubkey,
    inputs: &ConfigInputs,
) -> String {
    let target_profit = if inputs.target_profit_enabled {
        format_strategy_amount(&inputs.target_profit)
    } else {
        "disabled".to_string()
    };
    let stop_loss = if inputs.stop_loss_enabled {
        format_strategy_amount(&inputs.stop_loss)
    } else {
        "disabled".to_string()
    };
    let trailing_stop = if inputs.trailing_stop_enabled {
        format_strategy_amount(&inputs.trailing_stop)
    } else {
        "disabled".to_string()
    };
    let deadline_label = if inputs.timeout_enabled {
        format!("{}s", inputs.sell_timeout_sec)
    } else {
        "disabled".to_string()
    };
    format!(
        "Strategy: Take Profit {target_profit}, Stop Loss {stop_loss}, Trailing Stop {trailing_stop}, Deadline {deadline_label}\n\
         RPC: {}\n\
         API Key: [configured]\n\
         Local Mode: {}\n\
         Wallet: {}\n\
         Files: {}, {}",
        rpc_summary(inputs),
        if inputs.local { "enabled" } else { "disabled" },
        short_pubkey(pubkey),
        path_label(config_path),
        path_label(keystore_path),
    )
}

fn format_strategy_amount(amount: &StrategyAmount) -> String {
    match amount {
        StrategyAmount::Percent(value) => format!("{value}%"),
    }
}

fn rpc_summary(inputs: &ConfigInputs) -> String {
    let url = inputs.rpc_url.trim();
    if url.is_empty() {
        "Missing RPC URL".to_string()
    } else {
        url.to_string()
    }
}

fn short_pubkey(pubkey: &solana_sdk::pubkey::Pubkey) -> String {
    let full = pubkey.to_string();
    if full.len() <= 4 {
        full
    } else {
        format!("{}...", &full[..4])
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
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

fn generate_new_wallet() -> Result<(Zeroizing<String>, Keypair)> {
    let mnemonic = Mnemonic::new(MnemonicType::Words12, Language::English);
    let phrase = Zeroizing::new(mnemonic.phrase().to_string());
    let keypair = derive_keypair_from_mnemonic(phrase.as_str())?;
    Ok((phrase, keypair))
}

fn derive_keypair_from_mnemonic(phrase: &str) -> Result<Keypair> {
    let mnemonic = Mnemonic::from_phrase(phrase, Language::English).context("invalid mnemonic")?;
    let seed = Seed::new(&mnemonic, "");
    let mut seed_bytes = Zeroizing::new([0u8; 64]);
    seed_bytes.copy_from_slice(seed.as_bytes());
    let derivation_path = DerivationPath::from_absolute_path_str(SOLANA_DERIVATION_PATH)
        .context("invalid derivation path")?;
    let keypair = keypair_from_seed_and_derivation_path(seed_bytes.as_ref(), Some(derivation_path))
        .map_err(|err| anyhow!("failed to derive keypair: {err}"))?;
    seed_bytes.zeroize();
    Ok(keypair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn mnemonic_derives_expected_pubkey() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let keypair = derive_keypair_from_mnemonic(phrase).unwrap();
        assert_eq!(
            keypair.pubkey().to_string(),
            "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk"
        );
    }

    #[test]
    fn find_existing_keystore_prefers_config_path() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.yml");
        let keystore_path = dir.path().join("custom.keystore.json");
        let keypair = Keypair::new();
        let passphrase = SecretString::new("test-passphrase".to_string());

        wallet::write_keystore(&keystore_path, &keypair, &passphrase).unwrap();
        fs::write(
            &config_path,
            "account:\n  keypair_path: custom.keystore.json\n",
        )
        .unwrap();

        let found = find_existing_encrypted_keystore(&config_path);
        assert_eq!(found, Some(keystore_path));
    }

    #[test]
    fn find_existing_keystore_falls_back_to_default() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.yml");
        let keystore_path = default_keystore_path_for_config(&config_path);
        let keypair = Keypair::new();
        let passphrase = SecretString::new("test-passphrase".to_string());

        wallet::write_keystore(&keystore_path, &keypair, &passphrase).unwrap();

        let found = find_existing_encrypted_keystore(&config_path);
        assert_eq!(found, Some(keystore_path));
    }
}

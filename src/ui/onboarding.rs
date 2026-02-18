use std::fs;
use std::io::{self, IsTerminal, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bip39::{Language, Mnemonic, MnemonicType, Seed};
use crossterm::style::Stylize;
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
    stop_loss: StrategyAmount,
    stop_loss_enabled: bool,
    sell_timeout_sec: u64,
    slippage_max_bps: u16,
}

#[derive(Debug, Clone)]
struct ConfigInputs {
    rpc_url: String,
    api_key: String,
    local: bool,
    target_profit: StrategyAmount,
    stop_loss: StrategyAmount,
    stop_loss_enabled: bool,
    sell_timeout_sec: u64,
    slippage_max_bps: u16,
}

pub fn run_onboarding(config_path: &Path) -> Result<(Config, Keypair)> {
    println!("{}", fmt_banner("🚀 LASERSELL SETUP"));
    println!();

    let use_recommended = prompt_yes_no_default_yes(
        "✨ Use recommended settings? (skips strategy + file path questions)",
    )?;

    print_section("Credentials & Network");
    print_step(1, "RPC URL");
    let rpc_url = prompt_rpc_url()?;
    print_step(2, "API KEY");
    let api_key = prompt_api_key()?;

    print_step(3, "STRATEGY");
    print_section("Strategy");
    let StrategyInputs {
        target_profit,
        stop_loss,
        stop_loss_enabled,
        sell_timeout_sec,
        slippage_max_bps,
    } = if use_recommended {
        let defaults = StrategyInputs {
            target_profit: StrategyAmount::Percent(6.0),
            stop_loss: StrategyAmount::Percent(10.0),
            stop_loss_enabled: true,
            sell_timeout_sec: 120,
            slippage_max_bps: 2000,
        };
        println!(
            "{}",
            fmt_success("✨ Applied recommended strategy defaults.")
        );
        let slippage_label = format_bps_percent(defaults.slippage_max_bps);
        let target_profit_label = format_strategy_amount(&defaults.target_profit);
        let stop_loss_label = if defaults.stop_loss_enabled {
            format_strategy_amount(&defaults.stop_loss)
        } else {
            "disabled".to_string()
        };
        let deadline_label = if defaults.sell_timeout_sec == 0 {
            "OFF".to_string()
        } else {
            format!("{}s", defaults.sell_timeout_sec)
        };
        println!(
            "  Take Profit: {target_profit_label} | Stop Loss: {stop_loss_label} | Deadline: {deadline_label} | Slippage: {slippage_label}"
        );
        defaults
    } else {
        prompt_strategy_inputs()?
    };

    print_step(4, "WALLET");
    print_section("Wallet");
    let wallet_plan = if let Some(path) = find_existing_encrypted_keystore(config_path) {
        println!(
            "{}",
            fmt_success(&format!(
                "🔑 Existing keystore detected: {}",
                path.display()
            ))
        );
        if prompt_yes_no_default_yes("Use existing wallet and keep the current keystore file?")? {
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

    print_step(5, "SECURITY");
    print_section("Security");
    let (keypair, delete_after_import, mut passphrase, existing_keystore_path) = match wallet_plan {
        WalletPlan::ReuseExistingKeystore { path } => {
            println!("Unlock your existing wallet keystore.");
            let keypair = loop {
                let passphrase = prompt_unlock_passphrase()?;
                let result = wallet::load_keypair_from_path(&path, || {
                    Ok(SecretString::new(passphrase.to_string()))
                });
                match result {
                    Ok(keypair) => break keypair,
                    Err(err) => {
                        print_error(format!("Failed to unlock keystore: {err}"));
                    }
                }
            };
            (keypair, None, None, Some(path))
        }
        WalletPlan::NewWallet { selection } => {
            println!("Set a password to encrypt your wallet.");
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

    print_step(6, "FILES");
    print_section("Save Configuration");
    let mut config_path = if use_recommended {
        config_path.to_path_buf()
    } else {
        prompt_path("Config file location", config_path)?
    };
    let mut keystore_path = if reuse_existing_keystore {
        let path =
            existing_keystore_path.ok_or_else(|| anyhow!("existing keystore path missing"))?;
        println!(
            "{}",
            fmt_success(&format!(
                "🔁 Reusing existing keystore at {} (no changes will be made to the wallet file).",
                path.display()
            ))
        );
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
        stop_loss,
        stop_loss_enabled,
        sell_timeout_sec,
        slippage_max_bps,
    };

    print_step(7, "REVIEW");
    print_section("Review");
    loop {
        let mut config_changed = false;
        if config_path.exists() {
            let prompt = format!(
                "Config file location {} exists. Overwrite?",
                config_path.display()
            );
            if !prompt_yes_no_default_no(&prompt)? {
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
            let prompt = format!(
                "Keystore file location {} exists. Overwrite?",
                keystore_path.display()
            );
            if !prompt_yes_no_default_no(&prompt)? {
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

    print_summary(&config_path, &keystore_path, &pubkey, &inputs);

    let write_now = if use_recommended {
        prompt_yes_no_default_yes("Write configuration now?")?
    } else {
        prompt_yes_no_default_no("Write configuration now?")?
    };
    if !write_now {
        return Err(anyhow!("onboarding cancelled"));
    }

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
            print_warning(format!(
                "Failed to delete keypair JSON file {}: {err}",
                path.display()
            ));
        }
    }

    print_success("Setup complete.");
    Ok((config, keypair))
}

fn ansi_enabled() -> bool {
    std::io::stdout().is_terminal()
}

fn fmt_header(text: &str) -> String {
    if ansi_enabled() {
        format!("{}", text.bold().cyan())
    } else {
        text.to_string()
    }
}

fn fmt_banner(text: &str) -> String {
    if ansi_enabled() {
        format!("{}", text.bold().magenta())
    } else {
        text.to_string()
    }
}

fn fmt_step(step: usize, title: &str) -> String {
    let text = format!("🧭 Step {step}: {title}");
    if ansi_enabled() {
        format!("{}", text.bold().magenta())
    } else {
        text
    }
}

fn fmt_section(title: &str) -> String {
    let emoji = section_emoji(title);
    let text = format!("{emoji} {title}");
    fmt_header(&text)
}

fn fmt_success(text: &str) -> String {
    if ansi_enabled() {
        format!("{}", text.bold().green())
    } else {
        text.to_string()
    }
}

fn fmt_warning(text: &str) -> String {
    if ansi_enabled() {
        format!("{}", text.bold().yellow())
    } else {
        text.to_string()
    }
}

fn fmt_error(text: &str) -> String {
    if ansi_enabled() {
        format!("{}", text.bold().red())
    } else {
        text.to_string()
    }
}

fn print_section(title: &str) {
    println!("{}", fmt_section(title));
}

fn print_step(step: usize, title: &str) {
    println!();
    println!("{}", fmt_step(step, title));
}

fn section_emoji(title: &str) -> &'static str {
    match title {
        "Credentials & Network" => "📦",
        "Strategy" => "🧠",
        "Wallet" => "🔐",
        "Security" => "🛡️",
        "Save Configuration" => "📦",
        "Review" => "✅",
        _ => "📦",
    }
}

fn print_warning(message: impl std::fmt::Display) {
    let text = format!("⚠️ {message}");
    eprintln!("{}", fmt_warning(&text));
}

fn print_error(message: impl std::fmt::Display) {
    let message = support::with_support_hint(message.to_string());
    let text = format!("✖ {message}");
    eprintln!("{}", fmt_error(&text));
}

fn print_success(message: impl std::fmt::Display) {
    let text = format!("✅ {message}");
    println!("{}", fmt_success(&text));
}

fn prompt_wallet() -> Result<WalletSelection> {
    if prompt_yes_no_default_no("Import an existing wallet?")? {
        prompt_import_wallet()
    } else {
        prompt_create_wallet()
    }
}

fn prompt_create_wallet() -> Result<WalletSelection> {
    let (mnemonic, keypair) = generate_new_wallet()?;
    println!("{}", fmt_header("🧠 Generated seed phrase"));
    println!("{}", mnemonic.as_str());
    println!("{}", fmt_warning("⚠️ Save it now"));
    println!("Wallet pubkey: {}", keypair.pubkey());

    loop {
        if prompt_yes_no_default_no("I have saved this seed phrase. Continue?")? {
            return Ok(WalletSelection {
                keypair,
                delete_after_import: None,
            });
        }
        print_warning("Please save the seed phrase shown above before continuing.");
    }
}

fn prompt_import_wallet() -> Result<WalletSelection> {
    loop {
        let mode = prompt_import_mode()?;
        let (keypair, delete_after_import) = match mode {
            ImportMode::SeedPhrase => (prompt_seed_phrase_keypair()?, None),
            ImportMode::KeypairJson => {
                let (keypair, path) = prompt_keypair_json()?;
                (keypair, Some(path))
            }
            ImportMode::Base58Secret => (prompt_base58_keypair()?, None),
        };
        println!("Derived wallet pubkey: {}", keypair.pubkey());
        if prompt_yes_no_default_yes("Use this wallet?")? {
            let delete_after_import = if let Some(path) = delete_after_import {
                if prompt_yes_no_default_no(
                    "Delete the plaintext keypair JSON file after it's imported?",
                )? {
                    Some(path)
                } else {
                    None
                }
            } else {
                None
            };
            return Ok(WalletSelection {
                keypair,
                delete_after_import,
            });
        }
        print_warning("Let's try again.");
    }
}

fn prompt_import_mode() -> Result<ImportMode> {
    loop {
        println!("Select import method:");
        println!("  1) Private key (base58)");
        println!("  2) Seed phrase");
        println!("  3) Solana keypair JSON path");
        let choice = prompt_line("Enter choice", None)?;
        match choice.trim() {
            "1" => return Ok(ImportMode::Base58Secret),
            "2" => return Ok(ImportMode::SeedPhrase),
            "3" => return Ok(ImportMode::KeypairJson),
            _ => print_warning("Please enter 1, 2, or 3."),
        }
    }
}

fn prompt_seed_phrase_keypair() -> Result<Keypair> {
    loop {
        let phrase = prompt_required_password_masked("Seed phrase")?;
        match derive_keypair_from_mnemonic(phrase.as_str()) {
            Ok(keypair) => return Ok(keypair),
            Err(err) => print_error(format!("Invalid seed phrase: {err}")),
        }
    }
}

fn prompt_keypair_json() -> Result<(Keypair, PathBuf)> {
    loop {
        let raw = prompt_required_line("Solana keypair JSON path")?;
        let path = PathBuf::from(raw);
        match read_keypair_file(&path) {
            Ok(keypair) => return Ok((keypair, path)),
            Err(err) => print_error(format!("Failed to read keypair: {err}")),
        }
    }
}

fn prompt_base58_keypair() -> Result<Keypair> {
    loop {
        let raw = prompt_required_password_masked("Base58 secret key")?;
        match bs58::decode(raw.trim()).into_vec() {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                match Keypair::try_from(bytes.as_slice()) {
                    Ok(keypair) => return Ok(keypair),
                    Err(err) => print_error(format!("Invalid key bytes: {err}")),
                }
            }
            Err(err) => print_error(format!("Invalid base58 key: {err}")),
        }
    }
}

fn prompt_passphrase() -> Result<Zeroizing<String>> {
    loop {
        let passphrase = prompt_password_masked("Keystore passphrase")?;
        if passphrase.trim().is_empty() {
            print_warning("Passphrase cannot be empty.");
            continue;
        }
        let confirm = prompt_password_masked("Confirm passphrase")?;
        if passphrase.as_str() != confirm.as_str() {
            print_warning("Passphrases do not match. Try again.");
            continue;
        }
        return Ok(passphrase);
    }
}

fn prompt_unlock_passphrase() -> Result<Zeroizing<String>> {
    loop {
        let passphrase = prompt_password_masked("Keystore passphrase")?;
        if passphrase.trim().is_empty() {
            print_warning("Passphrase cannot be empty.");
            continue;
        }
        return Ok(passphrase);
    }
}

fn prompt_rpc_url() -> Result<String> {
    loop {
        let value = prompt_required_line("RPC URL")?;
        let trimmed = value.trim();
        let lowered = trimmed.to_ascii_lowercase();
        let parsed = match Url::parse(trimmed) {
            Ok(parsed) => parsed,
            Err(_) => {
                print_warning("RPC URL must be a valid URL.");
                continue;
            }
        };
        match lowered.split(':').next() {
            Some("https") => return Ok(trimmed.to_string()),
            Some("http") => {
                let host = parsed.host_str().unwrap_or_default();
                if is_local_or_private_host(host) {
                    print_warning(
                        "Using http:// for local/private RPC. Use https:// in production.",
                    );
                    return Ok(trimmed.to_string());
                }
                print_warning("http:// is only allowed for localhost/private RPC endpoints.");
            }
            _ => {
                print_warning(
                    "RPC URL must start with https:// (or http:// for local/private endpoints).",
                );
            }
        }
    }
}

fn prompt_api_key() -> Result<String> {
    loop {
        let value = prompt_required_password_masked("LaserSell API key")?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            print_warning("API key cannot be empty.");
            continue;
        }
        return Ok(trimmed.to_string());
    }
}

fn prompt_strategy_inputs() -> Result<StrategyInputs> {
    let target_profit = prompt_strategy_amount("Target Profit (% of buy)", "6%", false)?;
    let stop_loss_enabled = prompt_yes_no_default_yes("Enable Stop Loss?")?;
    let stop_loss = if stop_loss_enabled {
        prompt_strategy_amount("Stop Loss (% of buy)", "0%", true)?
    } else {
        StrategyAmount::Percent(0.0)
    };
    let sell_timeout_sec = prompt_u64("Deadline Timeout (seconds, 0 disables)", 45)?;
    let slippage_max_bps = prompt_slippage_percent("Slippage Tolerance (%)", "20%")?;
    Ok(StrategyInputs {
        target_profit,
        stop_loss,
        stop_loss_enabled,
        sell_timeout_sec,
        slippage_max_bps,
    })
}

fn build_config(inputs: &ConfigInputs, keystore_path: &Path) -> Result<Config> {
    Ok(Config {
        account: AccountConfig {
            keypair_path: keystore_path.to_string_lossy().to_string(),
            local: inputs.local,
            rpc_url: SecretString::new(inputs.rpc_url.clone()),
            api_key: SecretString::new(inputs.api_key.clone()),
        },
        strategy: StrategyConfig {
            target_profit: inputs.target_profit.clone(),
            stop_loss: inputs.stop_loss.clone(),
            deadline_timeout_sec: inputs.sell_timeout_sec,
        },
        sell: SellConfig {
            slippage_max_bps: inputs.slippage_max_bps,
            ..SellConfig::default()
        },
    })
}

fn print_summary(
    config_path: &Path,
    keystore_path: &Path,
    pubkey: &solana_sdk::pubkey::Pubkey,
    inputs: &ConfigInputs,
) {
    println!("Summary:");
    let target_profit = format_strategy_amount(&inputs.target_profit);
    let stop_loss = if inputs.stop_loss_enabled {
        format_strategy_amount(&inputs.stop_loss)
    } else {
        "disabled".to_string()
    };
    let deadline_label = if inputs.sell_timeout_sec == 0 {
        "OFF".to_string()
    } else {
        format!("{}s", inputs.sell_timeout_sec)
    };
    println!(
        "  Strategy: Take Profit {target_profit}, Stop Loss {stop_loss}, Deadline {deadline_label}",
    );
    println!("  RPC: {}", rpc_summary(inputs));
    println!("  API Key: [configured]");
    println!(
        "  Local Mode: {}",
        if inputs.local { "enabled" } else { "disabled" }
    );
    println!("  Wallet: {}", short_pubkey(pubkey));
    println!(
        "  Files: {}, {}",
        path_label(config_path),
        path_label(keystore_path)
    );
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

fn prompt_path(prompt: &str, default: &Path) -> Result<PathBuf> {
    let default_value = default.to_string_lossy().into_owned();
    loop {
        let value = prompt_line(prompt, Some(&default_value))?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            print_warning(format!("{prompt} cannot be empty."));
            continue;
        }
        return Ok(PathBuf::from(trimmed));
    }
}

fn prompt_required_line(prompt: &str) -> Result<String> {
    loop {
        let value = prompt_line(prompt, None)?;
        if value.trim().is_empty() {
            print_warning(format!("{prompt} cannot be empty."));
            continue;
        }
        return Ok(value);
    }
}

fn prompt_u64(prompt: &str, default: u64) -> Result<u64> {
    let default_value = default.to_string();
    loop {
        let value = prompt_line(prompt, Some(&default_value))?;
        match value.trim().parse::<u64>() {
            Ok(parsed) => return Ok(parsed),
            Err(_) => print_warning(format!("{prompt} must be a whole number.")),
        }
    }
}

fn prompt_strategy_amount(prompt: &str, default: &str, allow_zero: bool) -> Result<StrategyAmount> {
    loop {
        let value = prompt_line(prompt, Some(default))?;
        match StrategyAmount::parse_str(&value) {
            Ok(amount) => {
                let numeric = match amount {
                    StrategyAmount::Percent(val) => val,
                };
                if !numeric.is_finite() {
                    print_warning(format!("{prompt} must be a finite number."));
                    continue;
                }
                if allow_zero {
                    if numeric < 0.0 {
                        print_warning(format!("{prompt} must be >= 0."));
                        continue;
                    }
                } else if numeric <= 0.0 {
                    print_warning(format!("{prompt} must be > 0."));
                    continue;
                }
                return Ok(amount);
            }
            Err(err) => print_error(format!("Invalid strategy amount: {err}")),
        }
    }
}

fn prompt_slippage_percent(prompt: &str, default: &str) -> Result<u16> {
    loop {
        let value = prompt_line(prompt, Some(default))?;
        match parse_percent_to_bps(&value, "slippage") {
            Ok(parsed) => return Ok(parsed),
            Err(err) => print_error(format!("Invalid slippage percent: {err}")),
        }
    }
}

fn prompt_required_password_masked(prompt: &str) -> Result<Zeroizing<String>> {
    loop {
        let value = prompt_password_masked(prompt)?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            print_warning(format!("{prompt} cannot be empty."));
            continue;
        }
        return Ok(Zeroizing::new(trimmed.to_string()));
    }
}

fn prompt_line(prompt: &str, default: Option<&str>) -> Result<String> {
    let mut input = String::new();
    if let Some(default_value) = default {
        print!("{prompt} [{default_value}]: ");
    } else {
        print!("{prompt}: ");
    }
    io::stdout().flush().ok();
    let bytes = io::stdin().read_line(&mut input).context("read stdin")?;
    if bytes == 0 {
        return Err(anyhow!("stdin closed"));
    }
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        if let Some(default_value) = default {
            return Ok(default_value.to_string());
        }
    }
    Ok(trimmed)
}

fn prompt_yes_no_default_no(prompt: &str) -> Result<bool> {
    loop {
        let answer = prompt_line(&format!("{prompt} [y/N]"), None)?;
        let normalized = answer.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Ok(false);
        }
        match normalized.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => print_warning("Please enter y or n."),
        }
    }
}

fn prompt_yes_no_default_yes(prompt: &str) -> Result<bool> {
    loop {
        let answer = prompt_line(&format!("{prompt} [Y/n]"), None)?;
        let normalized = answer.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Ok(true);
        }
        match normalized.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => print_warning("Please enter y or n."),
        }
    }
}

fn prompt_password_masked(prompt: &str) -> Result<Zeroizing<String>> {
    print!("{prompt}: ");
    io::stdout().flush().ok();
    let value = rpassword::read_password().context("read passphrase")?;
    Ok(Zeroizing::new(value))
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

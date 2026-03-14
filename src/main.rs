use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use lasersell_sdk::exit_api::{
    BuildSellTxRequest, ExitApiClient, ExitApiClientOptions, SellOutput,
};
use lasersell_sdk::stream::client::{StreamClient as SdkStreamClient, StreamConfigure};
use lasersell_sdk::stream::proto::{ServerMessage, StrategyConfigMsg};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

mod app;
mod config;
mod events;
mod market;
mod network;
mod onboarding;
mod stream;
mod tx;
mod util;
mod wallet;

fn main() -> Result<()> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    let runtime = builder.build().context("build tokio runtime")?;
    runtime.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let cli = parse_cli_args()?;
    if cli.export_private_key {
        export_private_key(&cli)?;
        return Ok(());
    }
    if cli.smoke {
        match run_smoke_mode(&cli.config_path).await {
            Ok(()) => println!("SMOKE OK"),
            Err(failure) => {
                eprintln!("SMOKE FAIL {}", failure.step);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Kick off version check in the background immediately — before wallet
    // unlock so the user sees the banner while the passphrase prompt is up.
    let update_check_handle = tokio::spawn(util::update_check::check_for_update());

    let config_path = cli.config_path.clone();
    if let Err(err) = util::paths::ensure_data_dir_exists() {
        eprintln!(
            "{}",
            util::support::with_support_hint(format!("Failed to create data dir: {err}"))
        );
    }
    let (cfg, keypair): (config::Config, solana_sdk::signature::Keypair) = if cli.setup {
        onboarding::run_onboarding(&config_path)?
    } else {
        if !config_path.exists() {
            if std::io::stdin().is_terminal() {
                onboarding::run_onboarding(&config_path)?
            } else {
                return Err(anyhow!(
                    "config file {} not found; run --setup in an interactive terminal",
                    config_path.display()
                ));
            }
        } else {
            let cfg = config::Config::load_from_path(&config_path)?;
            let keypair_path = PathBuf::from(&cfg.account.keypair_path);
            let wallet_kind = wallet::detect_wallet_file_kind(&keypair_path)?;
            let keypair = match wallet_kind {
                wallet::WalletFileKind::EncryptedKeystore => {
                    let keystore_pubkey = wallet::read_keystore_pubkey(&keypair_path).ok();
                    wallet::load_keypair_from_path(&keypair_path, || {
                        read_passphrase_cli(keystore_pubkey.as_deref())
                    })?
                }
                wallet::WalletFileKind::PlaintextSolanaJson => {
                    let keypair = wallet::load_keypair_from_path(&keypair_path, || {
                        Err(anyhow!("passphrase not required"))
                    })?;
                    if std::io::stdin().is_terminal() {
                        let migrate = cliclack::confirm(
                            "Plaintext keypair detected. Encrypt this wallet now?",
                        )
                        .initial_value(true)
                        .interact()
                        .unwrap_or(false);
                        if migrate {
                            let passphrase = prompt_new_passphrase()?;
                            let keystore_path = wallet::default_keystore_path(&keypair_path);
                            let mut cfg = cfg.clone();
                            wallet::migrate_plaintext_to_keystore(
                                &keypair_path,
                                &keystore_path,
                                passphrase,
                                |path| {
                                    cfg.account.keypair_path = path.to_string_lossy().to_string();
                                    cfg.write_to_path(&config_path)?;
                                    Ok(())
                                },
                            )?;
                        }
                    } else {
                        eprintln!(
                            "Warning: plaintext keypair file in use ({}). Run --setup to migrate.",
                            keypair_path.display()
                        );
                    }
                    keypair
                }
            };
            (cfg, keypair)
        }
    };

    // Collect the update check result.
    let update_available = update_check_handle.await.ok().flatten();
    if let Some(ref update) = update_available {
        util::update_check::print_update_banner(update);
    }

    util::logging::init_redactions(vec![
        cfg.account.rpc_url.expose_secret().to_string(),
        cfg.account.api_key.expose_secret().to_string(),
    ]);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if cli.debug {
            EnvFilter::new("info,lasersell=debug,lasersell_sdk=debug,lasersell_sdk::stream::client=trace")
        } else {
            EnvFilter::new("info")
        }
    });
    let _debug_log_guard = init_tracing(cli.debug, filter);
    let wallet_pubkey = cfg.wallet_pubkey(&keypair)?;

    events::emit(events::AppEvent::Startup {
        version: env!("CARGO_PKG_VERSION").to_string(),
        wallet_pubkey,
    });

    // Install Ctrl+C handler for graceful shutdown.
    let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(events::AppCommand::Quit);
        }
    });

    app::run(cfg, keypair, Some(shutdown_rx)).await
}

fn export_private_key(cli: &CliArgs) -> Result<()> {
    let keystore_path = resolve_export_private_key_path(cli)?;
    if !keystore_path.is_file() {
        return Err(anyhow!(
            "keystore file {} not found",
            keystore_path.display()
        ));
    }
    let wallet_kind = wallet::detect_wallet_file_kind(&keystore_path)?;
    if wallet_kind != wallet::WalletFileKind::EncryptedKeystore {
        return Err(anyhow!(
            "wallet file {} is plaintext JSON; run --setup to migrate",
            keystore_path.display()
        ));
    }
    let keypair = wallet::load_keypair_from_path(&keystore_path, || read_passphrase_cli(None))?;
    let bytes = Zeroizing::new(keypair.to_bytes());
    let b58 = Zeroizing::new(bs58::encode(bytes.as_ref()).into_string());
    let mut out = std::io::stdout();
    out.write_all(b58.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn resolve_export_private_key_path(cli: &CliArgs) -> Result<PathBuf> {
    if let Some(path) = cli.export_private_key_path.as_ref() {
        return Ok(path.clone());
    }
    if let Ok(value) = env::var("LASERSELL_KEYPAIR_PATH") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if !cli.config_path.as_os_str().is_empty() {
        return keypair_path_from_config(&cli.config_path);
    }
    if let Ok(value) = env::var("LASERSELL_CONFIG_PATH") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return keypair_path_from_config(Path::new(trimmed));
        }
    }
    let default_config_path = default_config_path()?;
    if default_config_path.is_file() {
        return keypair_path_from_config(&default_config_path);
    }
    Ok(util::paths::default_data_dir()?.join("wallet.keystore.json"))
}

fn keypair_path_from_config(config_path: &Path) -> Result<PathBuf> {
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("read config file {}", config_path.display()))?;
    let cfg: ExportConfig = serde_yaml::from_str(&raw)
        .with_context(|| format!("parse yaml config {}", config_path.display()))?;
    let keypair_path = cfg.account.keypair_path.trim();
    if keypair_path.is_empty() {
        return Err(anyhow!("account.keypair_path must not be empty"));
    }
    Ok(PathBuf::from(keypair_path))
}

/// Read passphrase from env or terminal prompt.
fn read_passphrase_cli(wallet_pubkey: Option<&str>) -> Result<SecretString> {
    if let Ok(value) = env::var("LASERSELL_WALLET_PASSPHRASE") {
        if !value.trim().is_empty() {
            return Ok(SecretString::new(value));
        }
    }
    if let Some(pubkey) = wallet_pubkey {
        let truncated = if pubkey.len() > 8 {
            format!("{}...{}", &pubkey[..4], &pubkey[pubkey.len() - 4..])
        } else {
            pubkey.to_string()
        };
        eprint!("Unlock wallet ({truncated}): ");
    } else {
        eprint!("Keystore passphrase: ");
    }
    std::io::stderr().flush().ok();
    let passphrase = rpassword::read_password().context("read passphrase")?;
    if passphrase.trim().is_empty() {
        return Err(anyhow!("passphrase cannot be empty"));
    }
    Ok(SecretString::new(passphrase))
}

/// Prompt for a new passphrase with confirmation.
fn prompt_new_passphrase() -> Result<SecretString> {
    loop {
        eprint!("Set keystore passphrase: ");
        std::io::stderr().flush().ok();
        let passphrase = rpassword::read_password().context("read passphrase")?;
        if passphrase.trim().is_empty() {
            eprintln!("Passphrase cannot be empty.");
            continue;
        }
        eprint!("Confirm passphrase: ");
        std::io::stderr().flush().ok();
        let confirm = rpassword::read_password().context("read passphrase confirmation")?;
        if passphrase != confirm {
            eprintln!("Passphrases do not match. Try again.");
            continue;
        }
        return Ok(SecretString::new(passphrase));
    }
}

#[derive(Clone, Debug)]
struct CliArgs {
    config_path: PathBuf,
    debug: bool,
    setup: bool,
    smoke: bool,
    export_private_key: bool,
    export_private_key_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Parser)]
#[command(
    name = "lasersell",
    version,
    about = "LaserSell CLI — automated exit daemon for Solana."
)]
struct RawCliArgs {
    #[arg(
        short = 'f',
        long = "config",
        value_name = "path",
        env = "LASERSELL_CONFIG_PATH"
    )]
    config_path: Option<PathBuf>,
    #[arg(long = "debug", help = "Write debug-level logs to debug.log")]
    debug: bool,
    #[arg(long = "setup")]
    setup: bool,
    #[arg(long = "smoke")]
    smoke: bool,
    #[arg(long = "export-private-key", value_name = "path", num_args = 0..=1)]
    export_private_key: Option<Option<PathBuf>>,
}

#[derive(Debug, Deserialize)]
struct ExportConfig {
    account: ExportAccount,
}

#[derive(Debug, Deserialize)]
struct ExportAccount {
    keypair_path: String,
}

fn parse_cli_args() -> Result<CliArgs> {
    normalize_cli_args(RawCliArgs::parse())
}

#[cfg(test)]
fn parse_cli_args_from<I, T>(args: I) -> Result<CliArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let raw = RawCliArgs::try_parse_from(args).map_err(|err| anyhow!(err.to_string()))?;
    normalize_cli_args(raw)
}

fn normalize_cli_args(raw: RawCliArgs) -> Result<CliArgs> {
    let export_private_key = raw.export_private_key.is_some();
    let export_private_key_path = raw.export_private_key.flatten();
    if export_private_key_path
        .as_ref()
        .map(|path| path.as_os_str().is_empty())
        .unwrap_or(false)
    {
        return Err(anyhow!("--export-private-key requires a path after '='"));
    }
    if raw.smoke && raw.setup {
        return Err(anyhow!("--smoke cannot be combined with --setup"));
    }
    if raw.smoke && export_private_key {
        return Err(anyhow!(
            "--smoke cannot be combined with --export-private-key"
        ));
    }
    if export_private_key {
        if raw.setup {
            return Err(anyhow!(
                "--export-private-key cannot be combined with --setup"
            ));
        }
        return Ok(CliArgs {
            config_path: raw.config_path.unwrap_or_default(),
            debug: raw.debug,
            setup: raw.setup,
            smoke: raw.smoke,
            export_private_key,
            export_private_key_path,
        });
    }
    let config_path = match raw.config_path {
        Some(path) => path,
        None => default_config_path()?,
    };

    Ok(CliArgs {
        config_path,
        debug: raw.debug,
        setup: raw.setup,
        smoke: raw.smoke,
        export_private_key,
        export_private_key_path,
    })
}

fn default_config_path() -> Result<PathBuf> {
    if let Ok(value) = env::var("LASERSELL_CONFIG_PATH") {
        if !value.trim().is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    util::paths::default_config_path()
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn parse_export_private_key_without_path() {
        let cli =
            parse_cli_args_from(["lasersell", "--export-private-key"]).expect("parse cli args");
        assert!(cli.export_private_key);
        assert!(cli.export_private_key_path.is_none());
        assert!(cli.config_path.as_os_str().is_empty());
    }

    #[test]
    fn parse_export_private_key_with_path() {
        let cli = parse_cli_args_from([
            "lasersell",
            "--export-private-key",
            "/tmp/wallet.keystore.json",
        ])
        .expect("parse cli args");
        assert!(cli.export_private_key);
        assert_eq!(
            cli.export_private_key_path,
            Some(PathBuf::from("/tmp/wallet.keystore.json"))
        );
    }

    #[test]
    fn parse_rejects_smoke_setup_combo() {
        let err =
            parse_cli_args_from(["lasersell", "--smoke", "--setup"]).expect_err("should fail");
        assert!(err
            .to_string()
            .contains("--smoke cannot be combined with --setup"));
    }

    #[test]
    fn parse_rejects_smoke_export_combo() {
        let err = parse_cli_args_from(["lasersell", "--smoke", "--export-private-key"])
            .expect_err("should fail");
        assert!(err
            .to_string()
            .contains("--smoke cannot be combined with --export-private-key"));
    }
}

#[derive(Clone, Copy, Debug)]
struct SmokeFailure {
    step: &'static str,
}

impl SmokeFailure {
    const fn new(step: &'static str) -> Self {
        Self { step }
    }
}

const SMOKE_WALLET_PUBKEY: &str = "11111111111111111111111111111111";
const SMOKE_MINT: &str = "So11111111111111111111111111111111111111112";

async fn run_smoke_mode(config_path: &Path) -> std::result::Result<(), SmokeFailure> {
    let previous = env::var_os("LASERSELL_SUPPRESS_CONFIG_WARNINGS");
    env::set_var("LASERSELL_SUPPRESS_CONFIG_WARNINGS", "1");
    let cfg_result = config::Config::load_from_path(config_path);
    match previous {
        Some(value) => env::set_var("LASERSELL_SUPPRESS_CONFIG_WARNINGS", value),
        None => env::remove_var("LASERSELL_SUPPRESS_CONFIG_WARNINGS"),
    }
    let cfg = cfg_result.map_err(|_| SmokeFailure::new("config"))?;
    smoke_stream_check(&cfg).await?;
    smoke_exit_api_check(&cfg).await?;
    Ok(())
}

async fn smoke_stream_check(cfg: &config::Config) -> std::result::Result<(), SmokeFailure> {
    let stream_client =
        SdkStreamClient::new(cfg.account.api_key.clone()).with_local_mode(cfg.account.local);
    let strategy = StrategyConfigMsg {
        target_profit_pct: cfg.strategy.target_profit.percent_value(),
        stop_loss_pct: cfg.strategy.stop_loss.percent_value(),
        trailing_stop_pct: cfg.strategy.trailing_stop.percent_value(),
        sell_on_graduation: cfg.strategy.sell_on_graduation,
        ..Default::default()
    };
    let mut configure = StreamConfigure::single_wallet(SMOKE_WALLET_PUBKEY.to_string(), strategy);
    configure.deadline_timeout_sec = cfg.strategy.deadline_timeout_sec;
    let mut connection = timeout(Duration::from_secs(5), stream_client.connect(configure))
        .await
        .map_err(|_| SmokeFailure::new("stream_connect_timeout"))?
        .map_err(|_| SmokeFailure::new("stream_connect"))?;

    let first_msg = timeout(Duration::from_secs(2), connection.recv())
        .await
        .map_err(|_| SmokeFailure::new("stream_hello_timeout"))?;
    match first_msg {
        Some(ServerMessage::HelloOk { .. }) => {}
        Some(_) => return Err(SmokeFailure::new("stream_hello_invalid")),
        None => return Err(SmokeFailure::new("stream_hello_missing")),
    }

    connection
        .sender()
        .request_exit_signal(1, Some(1000))
        .map_err(|_| SmokeFailure::new("stream_request_exit_signal_send"))?;
    Ok(())
}

async fn smoke_exit_api_check(cfg: &config::Config) -> std::result::Result<(), SmokeFailure> {
    let options = ExitApiClientOptions {
        connect_timeout: cfg.exit_api_connect_timeout(),
        attempt_timeout: cfg.exit_api_request_timeout(),
        ..ExitApiClientOptions::default()
    };
    let exit_api =
        ExitApiClient::with_options(optional_api_key_for_smoke(&cfg.account.api_key), options)
            .map(|client| client.with_local_mode(cfg.account.local))
            .map_err(|_| SmokeFailure::new("exit_api_client"))?;

    let request = BuildSellTxRequest {
        mint: SMOKE_MINT.to_string(),
        user_pubkey: SMOKE_WALLET_PUBKEY.to_string(),
        amount_tokens: 1,
        output: SellOutput::Sol,
        slippage_bps: 1000,
        ..Default::default()
    };

    let response = timeout(Duration::from_secs(5), exit_api.build_sell_tx(&request))
        .await
        .map_err(|_| SmokeFailure::new("exit_api_timeout"))?
        .map_err(|_| SmokeFailure::new("exit_api_request"))?;
    if response.tx.trim().is_empty() {
        return Err(SmokeFailure::new("exit_api_empty_tx"));
    }
    Ok(())
}

fn optional_api_key_for_smoke(api_key: &SecretString) -> Option<SecretString> {
    let trimmed = api_key.expose_secret().trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(SecretString::new(trimmed.to_string()))
}

fn init_tracing(
    debug: bool,
    filter: EnvFilter,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let error_log_path = match util::paths::default_error_log_path() {
        Ok(path) => Some(path),
        Err(err) => {
            eprintln!(
                "{}",
                util::support::with_support_hint(format!(
                    "Failed to resolve error log path: {err}"
                ))
            );
            None
        }
    };
    let debug_log_path = if debug {
        match util::paths::default_debug_log_path() {
            Ok(path) => Some(path),
            Err(err) => {
                eprintln!(
                    "{}",
                    util::support::with_support_hint(format!(
                        "Failed to resolve debug log path: {err}"
                    ))
                );
                None
            }
        }
    } else {
        None
    };
    if error_log_path.is_some() || debug_log_path.is_some() {
        if let Err(err) = util::paths::ensure_data_dir_exists() {
            eprintln!(
                "{}",
                util::support::with_support_hint(format!(
                    "Failed to create data dir for logs: {err}"
                ))
            );
        }
    }

    install_error_log_panic_hook(error_log_path.clone(), debug_log_path.clone());

    let error_file_layer = tracing_subscriber::fmt::layer()
        .with_writer({
            let error_log_path = error_log_path.clone();
            move || {
                let writer: Box<dyn Write + Send> = match error_log_path.as_ref() {
                    Some(path) => match OpenOptions::new().create(true).append(true).open(path) {
                        Ok(file) => Box::new(file),
                        Err(err) => {
                            eprintln!(
                                "{}",
                                util::support::with_support_hint(format!(
                                    "Failed to open error log {}: {err}",
                                    path.display()
                                ))
                            );
                            Box::new(std::io::sink())
                        }
                    },
                    None => Box::new(std::io::sink()),
                };
                util::logging::RedactingWriter::new(writer)
            }
        })
        .with_ansi(false)
        .with_filter(tracing_subscriber::filter::LevelFilter::WARN);

    let debug_guard = if let Some(path) = debug_log_path.as_ref() {
        write_debug_session_header(path);
        let dir = path.parent().expect("debug log path has parent");
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "debug.log".to_string());
        let file_appender = tracing_appender::rolling::never(dir, file_name);
        Some(tracing_appender::non_blocking(file_appender))
    } else {
        None
    };

    let (debug_writer, guard) = match debug_guard {
        Some((non_blocking, guard)) => (Some(non_blocking), Some(guard)),
        None => (None, None),
    };

    let debug_file_layer = debug_writer.map(|non_blocking| {
        tracing_subscriber::fmt::layer()
            .with_writer(move || util::logging::RedactingWriter::new(non_blocking.clone()))
            .with_ansi(false)
    });

    // CLI mode: always log to stderr
    tracing_subscriber::registry()
        .with(error_file_layer)
        .with(debug_file_layer)
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    guard
}

fn write_debug_session_header(debug_log_path: &Path) {
    let timestamp = utc_timestamp();
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(debug_log_path)
    {
        Ok(mut file) => {
            let pid = std::process::id();
            let _ = writeln!(file, "\n----- debug session start -----");
            let line = format!(
                "utc={} version={} pid={}",
                timestamp,
                env!("CARGO_PKG_VERSION"),
                pid
            );
            let _ = writeln!(file, "{}", util::logging::scrub_sensitive(&line));
        }
        Err(err) => {
            eprintln!(
                "Failed to open debug log {} for session header: {err}",
                debug_log_path.display()
            );
        }
    }
}

fn install_error_log_panic_hook(
    error_log_path: Option<PathBuf>,
    debug_log_path: Option<PathBuf>,
) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = error_log_path.as_ref() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let message = panic_message(info).replace('\n', "\\n");
                let mut line = format!("utc={} panic message={message}", utc_timestamp());
                if let Some(location) = info.location() {
                    line.push_str(&format!(
                        " location={}:{}",
                        location.file(),
                        location.line()
                    ));
                }
                let scrubbed = util::logging::scrub_sensitive(&line);
                let _ = writeln!(file, "{scrubbed}");
            }
        }
        if let Some(path) = debug_log_path.as_ref() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "\n----- panic -----");
                let message = util::logging::scrub_sensitive(&panic_message(info));
                let _ = writeln!(file, "message: {message}");
                if let Some(location) = info.location() {
                    let line = format!("location: {}:{}", location.file(), location.line());
                    let _ = writeln!(file, "{}", util::logging::scrub_sensitive(&line));
                }
                let backtrace = format!("{:?}", std::backtrace::Backtrace::force_capture());
                let backtrace = util::logging::scrub_sensitive(&backtrace);
                let _ = writeln!(file, "backtrace:\n{backtrace}");
            }
        }
        default_hook(info);
    }));
}

fn utc_timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "0000-00-00T00:00:00Z".to_string())
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

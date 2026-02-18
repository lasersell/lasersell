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
mod stream;
mod tx;
mod ui;
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
    let use_tui = !(cli.verbose || cli.no_tui);
    let config_path = cli.config_path.clone();
    if let Err(err) = util::paths::ensure_data_dir_exists() {
        eprintln!(
            "{}",
            util::support::with_support_hint(format!("Failed to create data dir: {err}"))
        );
    }
    let (cfg, keypair): (config::Config, solana_sdk::signature::Keypair) = if cli.setup {
        ui::onboarding::run_onboarding(&config_path)?
    } else {
        if !config_path.exists() {
            if !use_tui {
                if std::io::stdin().is_terminal() {
                    ui::onboarding::run_onboarding(&config_path)?
                } else {
                    return Err(anyhow!(
                        "config file {} not found; run --setup in an interactive terminal",
                        config_path.display()
                    ));
                }
            } else {
                ui::onboarding::run_onboarding(&config_path)?
            }
        } else {
            let mut cfg = config::Config::load_from_path(&config_path)?;
            let keypair_path = PathBuf::from(&cfg.account.keypair_path);
            let wallet_kind = wallet::detect_wallet_file_kind(&keypair_path)?;
            let keypair = match wallet_kind {
                wallet::WalletFileKind::EncryptedKeystore => {
                    wallet::load_keypair_from_path(&keypair_path, || {
                        if use_tui {
                            ui::unlock::prompt_passphrase("Unlock wallet keystore")
                        } else {
                            read_passphrase_non_tui()
                        }
                    })?
                }
                wallet::WalletFileKind::PlaintextSolanaJson => {
                    let keypair = wallet::load_keypair_from_path(&keypair_path, || {
                        Err(anyhow!("passphrase not required"))
                    })?;
                    if use_tui {
                        let migrate = ui::unlock::prompt_yes_no(
                            "Plaintext keypair detected. Encrypt this wallet now?",
                        )?;
                        if migrate {
                            let passphrase =
                                ui::unlock::prompt_passphrase("Set keystore passphrase")?;
                            let keystore_path = wallet::default_keystore_path(&keypair_path);
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
                            "Warning: plaintext keypair file in use ({}). Run with TUI to migrate.",
                            keypair_path.display()
                        );
                    }
                    keypair
                }
            };
            (cfg, keypair)
        }
    };

    util::logging::init_redactions(vec![
        cfg.account.rpc_url.expose_secret().to_string(),
        cfg.account.api_key.expose_secret().to_string(),
    ]);

    let mut tui_event_rx: Option<mpsc::UnboundedReceiver<events::AppEvent>> = None;
    if use_tui {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<events::AppEvent>();
        events::set_sender(event_tx);

        let (tui_tx, tui_rx) = mpsc::unbounded_channel::<events::AppEvent>();
        tui_event_rx = Some(tui_rx);

        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = tui_tx.send(event);
            }
        });
    }

    #[cfg(feature = "devnet")]
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,lasersell=debug"));
    #[cfg(not(feature = "devnet"))]
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    #[cfg(feature = "devnet")]
    let _devnet_log_guard = init_devnet_logging(use_tui, filter);
    #[cfg(not(feature = "devnet"))]
    init_tracing(use_tui, filter);
    let wallet_pubkey = cfg.wallet_pubkey(&keypair)?;

    events::emit(events::AppEvent::Startup {
        version: env!("CARGO_PKG_VERSION").to_string(),
        devnet: cfg!(feature = "devnet"),
        wallet_pubkey,
    });

    let (cmd_tx, cmd_rx) = if use_tui {
        let (tx, rx) = mpsc::unbounded_channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    if use_tui {
        let cmd_tx = cmd_tx.expect("tui command tx missing");
        let cfg_arc = std::sync::Arc::new(cfg.clone());
        let app_task = tokio::spawn(async move { app::run(cfg, keypair, cmd_rx).await });
        let tui_res = ui::run_tui(
            std::sync::Arc::clone(&cfg_arc),
            config_path.clone(),
            tui_event_rx.expect("tui event rx missing"),
            cmd_tx.clone(),
        )
        .await;
        if tui_res.is_err() {
            let _ = cmd_tx.send(events::AppCommand::Quit);
        }
        let app_res = app_task
            .await
            .map_err(|err| anyhow!("app task join error: {err}"))?;
        app_res?;
        tui_res
    } else {
        app::run(cfg, keypair, cmd_rx).await
    }
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
            "wallet file {} is plaintext JSON; run with TUI to migrate",
            keystore_path.display()
        ));
    }
    let keypair = wallet::load_keypair_from_path(&keystore_path, || read_passphrase_non_tui())?;
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

fn read_passphrase_non_tui() -> Result<SecretString> {
    if let Ok(value) = env::var("LASERSELL_WALLET_PASSPHRASE") {
        if !value.trim().is_empty() {
            return Ok(SecretString::new(value));
        }
    }
    eprint!("Keystore passphrase: ");
    std::io::stderr().flush().ok();
    let passphrase = rpassword::read_password().context("read passphrase")?;
    if passphrase.trim().is_empty() {
        return Err(anyhow!("passphrase cannot be empty"));
    }
    Ok(SecretString::new(passphrase))
}

#[derive(Clone, Debug)]
struct CliArgs {
    config_path: PathBuf,
    verbose: bool,
    no_tui: bool,
    setup: bool,
    smoke: bool,
    export_private_key: bool,
    export_private_key_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Parser)]
#[command(
    name = "lasersell",
    version,
    about = "Single-wallet CLI daemon that listens for exit signals and signs/sends sells."
)]
struct RawCliArgs {
    #[arg(
        short = 'f',
        long = "config",
        value_name = "path",
        env = "LASERSELL_CONFIG_PATH"
    )]
    config_path: Option<PathBuf>,
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,
    #[arg(long = "no-tui")]
    no_tui: bool,
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
    let use_tui = !(raw.verbose || raw.no_tui);
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
            verbose: raw.verbose,
            no_tui: raw.no_tui,
            setup: raw.setup,
            smoke: raw.smoke,
            export_private_key,
            export_private_key_path,
        });
    }
    let config_path = match raw.config_path {
        Some(path) => path,
        None => {
            if use_tui || raw.smoke {
                default_config_path()?
            } else {
                return Err(anyhow!("-f /path/to/config.yml is required"));
            }
        }
    };

    Ok(CliArgs {
        config_path,
        verbose: raw.verbose,
        no_tui: raw.no_tui,
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
    fn parse_headless_requires_config() {
        let err = parse_cli_args_from(["lasersell", "--no-tui"]).expect_err("should fail");
        assert!(err
            .to_string()
            .contains("-f /path/to/config.yml is required"));
    }

    #[test]
    fn parse_headless_with_config() {
        let cli = parse_cli_args_from(["lasersell", "--no-tui", "-f", "config.yml"])
            .expect("parse cli args");
        assert_eq!(cli.config_path, PathBuf::from("config.yml"));
        assert!(cli.no_tui);
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
        deadline_timeout_sec: cfg.strategy.deadline_timeout_sec,
    };
    let mut connection = timeout(
        Duration::from_secs(5),
        stream_client.connect(StreamConfigure::single_wallet(
            SMOKE_WALLET_PUBKEY.to_string(),
            strategy,
        )),
    )
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
        slippage_bps: Some(1000),
        mode: None,
        output: Some(SellOutput::Sol),
        referral_id: None,
        market_context: None,
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

#[cfg(not(feature = "devnet"))]
fn init_tracing(use_tui: bool, filter: EnvFilter) {
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
    if error_log_path.is_some() {
        if let Err(err) = util::paths::ensure_data_dir_exists() {
            eprintln!(
                "{}",
                util::support::with_support_hint(format!(
                    "Failed to create data dir for error log: {err}"
                ))
            );
        }
    }

    install_error_log_panic_hook(error_log_path.clone());

    let file_layer = tracing_subscriber::fmt::layer()
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

    if use_tui {
        tracing_subscriber::registry()
            .with(file_layer)
            .with(events::log_layer())
            .with(filter)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(file_layer)
            .with(tracing_subscriber::fmt::layer())
            .with(filter)
            .init();
    }
}

fn install_error_log_panic_hook(error_log_path: Option<PathBuf>) {
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
        default_hook(info);
    }));
}

#[cfg(feature = "devnet")]
fn init_devnet_logging(
    use_tui: bool,
    filter: EnvFilter,
) -> tracing_appender::non_blocking::WorkerGuard {
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
    let debug_log_path = match util::paths::default_debug_log_path() {
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
    };
    if debug_log_path.is_some() || error_log_path.is_some() {
        if let Err(err) = util::paths::ensure_data_dir_exists() {
            eprintln!(
                "{}",
                util::support::with_support_hint(format!(
                    "Failed to create data dir for logs: {err}"
                ))
            );
        }
    }

    install_error_log_panic_hook(error_log_path.clone());
    if let Some(path) = debug_log_path.as_ref() {
        write_devnet_session_header(path);
    }
    install_devnet_panic_hook(debug_log_path.clone());

    let (non_blocking, guard) = match debug_log_path.as_ref() {
        Some(path) => {
            if let Some(dir) = path.parent() {
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "debug.log".to_string());
                let file_appender = tracing_appender::rolling::never(dir, file_name);
                tracing_appender::non_blocking(file_appender)
            } else {
                tracing_appender::non_blocking(std::io::sink())
            }
        }
        None => tracing_appender::non_blocking(std::io::sink()),
    };
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer({
            let non_blocking = non_blocking.clone();
            move || util::logging::RedactingWriter::new(non_blocking.clone())
        })
        .with_ansi(false);
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

    if use_tui {
        tracing_subscriber::registry()
            .with(error_file_layer)
            .with(file_layer)
            .with(events::log_layer())
            .with(filter)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(error_file_layer)
            .with(file_layer)
            .with(tracing_subscriber::fmt::layer())
            .with(filter)
            .init();
    }

    guard
}

fn utc_timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "0000-00-00T00:00:00Z".to_string())
}

#[cfg(feature = "devnet")]
fn write_devnet_session_header(debug_log_path: &Path) {
    let timestamp = utc_timestamp();
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(debug_log_path)
    {
        Ok(mut file) => {
            let pid = std::process::id();
            let _ = writeln!(file, "\n----- devnet session start -----");
            let line = format!(
                "utc={} version={} pid={} devnet build",
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

#[cfg(feature = "devnet")]
fn install_devnet_panic_hook(debug_log_path: Option<PathBuf>) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
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

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

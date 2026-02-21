use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use lasersell_sdk::exit_api::{
    BuildSellTxRequest, ExitApiClient, ExitApiClientOptions, SellOutput,
};
use lasersell_sdk::stream::proto::{MarketContextMsg, StrategyConfigMsg};
use lasersell_sdk::tx::TxSubmitError;
use parking_lot::RwLock as ParkingRwLock;
use secrecy::{ExposeSecret, SecretString};
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::config::{Config, SellConfig, StrategyConfig};
use crate::events::{emit, AppCommand, AppEvent};
use crate::market::context_from_msg::market_context_from_msg;
use crate::market::context_to_msg::market_context_to_msg;
use crate::market::{usd1_mint, MarketContext, MarketType};
use crate::network::{rpc_result, StreamClient, StreamEvent, StreamHandle};
use crate::stream::InMemoryMarketStreamState;
use crate::tx::{send_tx, sign_unsigned_tx};

const HEARTBEAT_INTERVAL_SECS: u64 = 1;
const AUTOSELL_REFRESH_TIMEOUT_MS: u64 = 1_500;

#[derive(Clone, Debug)]
struct PositionSnapshot {
    position_id: u64,
    token_program: Option<String>,
    tokens: u64,
    market_context: Option<MarketContext>,
}

enum LoopControl {
    Continue,
    Break,
    DropCommands,
}

struct AppEngine {
    runtime_strategy: Arc<ParkingRwLock<StrategyConfig>>,
    runtime_sell: Arc<ParkingRwLock<SellConfig>>,
    wallet_pubkey: Pubkey,
    keypair_bytes: [u8; 64],
    rpc_http: reqwest::Client,
    rpc_url: String,
    local_mode: bool,
    exit_api: Arc<ExitApiClient>,
    stream_handle: Arc<StreamHandle>,
    market_contexts: Arc<ParkingRwLock<HashMap<Pubkey, MarketContext>>>,
    stream_states: Arc<ParkingRwLock<HashMap<Pubkey, Arc<InMemoryMarketStreamState>>>>,
    position_snapshots: Arc<ParkingRwLock<HashMap<Pubkey, PositionSnapshot>>>,
    in_flight_auto_sells: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>>,
    paused: bool,
}

pub async fn run(
    cfg: Config,
    keypair: Keypair,
    mut cmd_rx: Option<mpsc::UnboundedReceiver<AppCommand>>,
) -> Result<()> {
    let (mut engine, mut evt_rx) = AppEngine::new(cfg, keypair).await?;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));

    loop {
        tokio::select! {
            maybe_evt = evt_rx.recv() => {
                let Some(evt) = maybe_evt else {
                    break;
                };
                engine.handle_stream_event(evt).await?;
            }
            cmd = async {
                if let Some(rx) = cmd_rx.as_mut() {
                    rx.recv().await
                } else {
                    std::future::pending::<Option<AppCommand>>().await
                }
            } => {
                match engine.handle_user_command(cmd).await? {
                    LoopControl::Continue => {}
                    LoopControl::Break => break,
                    LoopControl::DropCommands => {
                        cmd_rx = None;
                    }
                }
            }
            _ = heartbeat.tick() => {
                engine.handle_heartbeat();
            }
        }
    }

    Ok(())
}

impl AppEngine {
    async fn new(
        cfg: Config,
        keypair: Keypair,
    ) -> Result<(Self, mpsc::UnboundedReceiver<StreamEvent>)> {
        let runtime_strategy = Arc::new(ParkingRwLock::new(cfg.strategy.clone()));
        let runtime_sell = Arc::new(ParkingRwLock::new(cfg.sell.clone()));
        let wallet_pubkey = cfg.wallet_pubkey(&keypair)?;
        let keypair_bytes = keypair.to_bytes();
        let rpc_http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(cfg.rpc_connect_timeout())
            .timeout(cfg.rpc_request_timeout())
            .build()?;
        let rpc_url = cfg.http_rpc_url();
        let local_mode = cfg.account.local;

        spawn_wallet_balance_fetch(rpc_http.clone(), rpc_url.clone(), wallet_pubkey);
        spawn_usd1_balance_poller(rpc_http.clone(), rpc_url.clone(), wallet_pubkey);

        let exit_api = Arc::new(build_exit_api_client(
            &cfg.account.api_key,
            cfg.account.local,
            cfg.exit_api_connect_timeout(),
            cfg.exit_api_request_timeout(),
        )?);

        let stream_client = StreamClient::new(
            cfg.account.api_key.clone(),
            cfg.account.local,
            wallet_pubkey.to_string(),
            strategy_to_msg(&cfg.strategy),
            cfg.strategy.deadline_timeout_sec,
        );
        let (stream_handle, evt_rx) = stream_client.connect().await?;
        let stream_handle = Arc::new(stream_handle);

        let market_contexts = Arc::new(ParkingRwLock::new(HashMap::<Pubkey, MarketContext>::new()));
        let stream_states = Arc::new(ParkingRwLock::new(HashMap::<
            Pubkey,
            Arc<InMemoryMarketStreamState>,
        >::new()));
        let position_snapshots = Arc::new(ParkingRwLock::new(
            HashMap::<Pubkey, PositionSnapshot>::new(),
        ));
        let in_flight_auto_sells = Arc::new(Mutex::new(HashMap::<
            u64,
            mpsc::UnboundedSender<String>,
        >::new()));

        Ok((
            Self {
                runtime_strategy,
                runtime_sell,
                wallet_pubkey,
                keypair_bytes,
                rpc_http,
                rpc_url,
                local_mode,
                exit_api,
                stream_handle,
                market_contexts,
                stream_states,
                position_snapshots,
                in_flight_auto_sells,
                paused: false,
            },
            evt_rx,
        ))
    }

    async fn handle_stream_event(&mut self, evt: StreamEvent) -> Result<()> {
        match evt {
            StreamEvent::ConnectionStatus { connected } => {
                emit(AppEvent::SolanaWsStatus { connected });
            }
            StreamEvent::BalanceUpdate {
                mint,
                token_program,
                token_account: _token_account,
                tokens,
                slot: _slot,
            } => self.handle_balance_update(mint, token_program, tokens),
            StreamEvent::PositionOpened {
                position_id,
                mint,
                token_program,
                token_account,
                tokens,
                slot,
                market_context,
            } => self.handle_position_opened(
                position_id,
                mint,
                token_program,
                token_account,
                tokens,
                slot,
                market_context,
            ),
            StreamEvent::PositionClosed {
                position_id,
                mint,
                token_account: _token_account,
                reason,
                slot,
            } => {
                self.handle_position_closed(position_id, mint, reason, slot)
                    .await;
            }
            StreamEvent::ExitSignalWithTx {
                position_id,
                mint,
                token_program,
                token_account: _token_account,
                position_tokens,
                profit_units,
                reason,
                triggered_at_ms: _triggered_at_ms,
                market_context,
                unsigned_tx_b64,
            } => {
                self.handle_exit_signal_with_tx(
                    position_id,
                    mint,
                    token_program,
                    position_tokens,
                    profit_units,
                    reason,
                    market_context,
                    unsigned_tx_b64,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn handle_user_command(&mut self, cmd: Option<AppCommand>) -> Result<LoopControl> {
        match cmd {
            Some(AppCommand::Quit) => Ok(LoopControl::Break),
            Some(AppCommand::TogglePauseNewSessions) => {
                self.paused = !self.paused;
                emit(AppEvent::PauseState {
                    paused: self.paused,
                });
                Ok(LoopControl::Continue)
            }
            Some(AppCommand::ApplySettings { strategy, sell }) => {
                *self.runtime_strategy.write() = strategy.clone();
                *self.runtime_sell.write() = sell.clone();
                if let Err(err) = self
                    .stream_handle
                    .update_strategy(strategy_to_msg(&strategy), strategy.deadline_timeout_sec)
                {
                    warn!(event = "stream_update_strategy_failed", error = %err);
                }
                Ok(LoopControl::Continue)
            }
            Some(AppCommand::RequestExitSignal { mint }) => {
                self.handle_manual_sell_request(mint).await;
                Ok(LoopControl::Continue)
            }
            None => Ok(LoopControl::DropCommands),
        }
    }

    fn handle_heartbeat(&self) {
        emit(AppEvent::Heartbeat);
    }

    fn handle_balance_update(&self, mint: String, token_program: Option<String>, tokens: u64) {
        if let Ok(mint) = Pubkey::from_str(&mint) {
            {
                let mut snapshots = self.position_snapshots.write();
                let entry = snapshots.entry(mint).or_insert(PositionSnapshot {
                    position_id: 0,
                    token_program: None,
                    tokens: 0,
                    market_context: None,
                });
                if token_program.is_some() {
                    entry.token_program = token_program;
                }
                entry.tokens = tokens;
            }
            if let Some(stream_state) = self.stream_states.read().get(&mint).cloned() {
                stream_state.set_position_tokens(Some(tokens));
            }
            emit(AppEvent::PositionTokensUpdated { mint, tokens });
        }
    }

    fn handle_position_opened(
        &self,
        position_id: u64,
        mint: String,
        token_program: Option<String>,
        token_account: String,
        tokens: u64,
        _slot: u64,
        market_context: Option<MarketContextMsg>,
    ) {
        if let Ok(mint) = Pubkey::from_str(&mint) {
            let parsed_context = match apply_market_context_update(
                mint,
                market_context,
                self.market_contexts.as_ref(),
            ) {
                Ok(context) => context,
                Err(err) => {
                    emit(AppEvent::SessionError {
                        mint,
                        error: err.to_string(),
                    });
                    None
                }
            };
            let context_for_state = parsed_context
                .clone()
                .or_else(|| self.market_contexts.read().get(&mint).cloned());
            upsert_market_stream_state(
                self.stream_states.as_ref(),
                mint,
                context_for_state.as_ref(),
                Some(tokens),
            );
            self.position_snapshots.write().insert(
                mint,
                PositionSnapshot {
                    position_id,
                    token_program,
                    tokens,
                    market_context: parsed_context,
                },
            );
            emit(AppEvent::MintDetected {
                mint,
                token_account: Pubkey::from_str(&token_account).unwrap_or_default(),
            });
            emit(AppEvent::PositionTokensUpdated { mint, tokens });
        }
    }

    async fn handle_position_closed(
        &self,
        position_id: u64,
        mint: String,
        reason: String,
        slot: u64,
    ) {
        if let Ok(mint) = Pubkey::from_str(&mint) {
            {
                let mut snapshots = self.position_snapshots.write();
                let should_remove = snapshots
                    .get(&mint)
                    .map(|snapshot| {
                        snapshot.position_id == position_id || snapshot.position_id == 0
                    })
                    .unwrap_or(false);
                if should_remove {
                    snapshots.remove(&mint);
                }
            }
            self.market_contexts.write().remove(&mint);
            self.stream_states.write().remove(&mint);
            self.in_flight_auto_sells.lock().await.remove(&position_id);
            debug!(
                event = "position_closed",
                mint = %mint,
                position_id,
                reason = %reason,
                slot
            );
            emit(AppEvent::SessionClosed { mint });
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_exit_signal_with_tx(
        &self,
        position_id: u64,
        mint: String,
        token_program: Option<String>,
        position_tokens: u64,
        profit_units: i64,
        reason: String,
        market_context: Option<MarketContextMsg>,
        unsigned_tx_b64: String,
    ) -> Result<()> {
        process_exit_signal_with_tx(
            self.paused,
            position_id,
            mint,
            token_program,
            position_tokens,
            profit_units,
            reason,
            market_context,
            unsigned_tx_b64,
            self.stream_handle.clone(),
            self.rpc_http.clone(),
            self.keypair_bytes,
            self.rpc_url.clone(),
            self.local_mode,
            self.runtime_sell.clone(),
            self.in_flight_auto_sells.clone(),
            self.market_contexts.clone(),
            self.stream_states.clone(),
            self.position_snapshots.clone(),
        )
        .await
    }

    async fn handle_manual_sell_request(&self, mint: Pubkey) {
        trigger_manual_sell(
            mint,
            self.exit_api.clone(),
            self.rpc_http.clone(),
            self.keypair_bytes,
            self.wallet_pubkey,
            self.rpc_url.clone(),
            self.local_mode,
            self.runtime_sell.clone(),
            self.position_snapshots.clone(),
            self.market_contexts.clone(),
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_exit_signal_with_tx(
    paused: bool,
    position_id: u64,
    mint: String,
    token_program: Option<String>,
    position_tokens: u64,
    profit_units: i64,
    reason: String,
    market_context_msg: Option<MarketContextMsg>,
    unsigned_tx_b64: String,
    stream_handle: Arc<StreamHandle>,
    rpc_http: reqwest::Client,
    keypair_bytes: [u8; 64],
    rpc_url: String,
    local_mode: bool,
    runtime_sell: Arc<ParkingRwLock<SellConfig>>,
    in_flight_auto_sells: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>>,
    market_contexts: Arc<ParkingRwLock<HashMap<Pubkey, MarketContext>>>,
    stream_states: Arc<ParkingRwLock<HashMap<Pubkey, Arc<InMemoryMarketStreamState>>>>,
    position_snapshots: Arc<ParkingRwLock<HashMap<Pubkey, PositionSnapshot>>>,
) -> Result<()> {
    let mint_pubkey = match Pubkey::from_str(&mint) {
        Ok(value) => value,
        Err(_) => {
            warn!(event = "exit_signal_invalid_mint", mint = %mint);
            return Ok(());
        }
    };

    let parsed_context = match apply_market_context_update(
        mint_pubkey,
        market_context_msg,
        market_contexts.as_ref(),
    ) {
        Ok(context) => context,
        Err(err) => {
            emit(AppEvent::SessionError {
                mint: mint_pubkey,
                error: err.to_string(),
            });
            None
        }
    };
    let context_for_state = parsed_context
        .clone()
        .or_else(|| market_contexts.read().get(&mint_pubkey).cloned());
    upsert_market_stream_state(
        stream_states.as_ref(),
        mint_pubkey,
        context_for_state.as_ref(),
        Some(position_tokens),
    );

    position_snapshots.write().insert(
        mint_pubkey,
        PositionSnapshot {
            position_id,
            token_program: token_program.clone(),
            tokens: position_tokens,
            market_context: parsed_context
                .clone()
                .or_else(|| market_contexts.read().get(&mint_pubkey).cloned()),
        },
    );

    if paused {
        return Ok(());
    }

    let mut in_flight = in_flight_auto_sells.lock().await;
    if let Some(existing_tx) = in_flight.get(&position_id) {
        let _ = existing_tx.send(unsigned_tx_b64);
        return Ok(());
    }
    let (refresh_tx, refresh_rx) = mpsc::unbounded_channel::<String>();
    in_flight.insert(position_id, refresh_tx);
    drop(in_flight);

    let runtime_sell = runtime_sell.clone();
    let rpc_http = rpc_http.clone();
    let rpc_url = rpc_url.clone();
    let in_flight_auto_sells = in_flight_auto_sells.clone();
    let position_snapshots = position_snapshots.clone();
    let market_contexts = market_contexts.clone();
    let stream_states = stream_states.clone();
    let stream_handle = stream_handle.clone();
    tokio::spawn(async move {
        let sell_reason = canonical_sell_reason(&reason).to_string();
        let token_program =
            match resolve_token_program(&rpc_http, &rpc_url, &token_program, mint_pubkey).await {
                Ok(value) => value,
                Err(err) => {
                    emit(AppEvent::SessionError {
                        mint: mint_pubkey,
                        error: err.to_string(),
                    });
                    in_flight_auto_sells.lock().await.remove(&position_id);
                    return;
                }
            };

        emit(AppEvent::SessionStarted {
            mint: mint_pubkey,
            token_program,
            started_at_ms: now_ms(),
        });
        emit(AppEvent::PositionTokensUpdated {
            mint: mint_pubkey,
            tokens: position_tokens,
        });
        info!(
            event = "sell_scheduled",
            mint = %mint_pubkey,
            reason = %sell_reason,
            profit_lamports = profit_units
        );
        emit(AppEvent::SellScheduled {
            mint: mint_pubkey,
            reason: sell_reason.clone(),
            profit_lamports: profit_units,
        });

        let sell_cfg = runtime_sell.read().clone();
        let result = execute_auto_sell_with_refresh(
            stream_handle,
            refresh_rx,
            rpc_http,
            keypair_bytes,
            rpc_url,
            local_mode,
            mint_pubkey,
            position_id,
            sell_cfg,
            unsigned_tx_b64,
        )
        .await;

        match result {
            Ok((signature, slippage_bps)) => {
                info!(
                    event = "sell_complete",
                    mint = %mint_pubkey,
                    signature = %signature,
                    reason = %sell_reason,
                    slippage_bps
                );
                emit(AppEvent::SellComplete {
                    mint: mint_pubkey,
                    signature,
                    reason: sell_reason,
                    slippage_bps,
                });
                emit(AppEvent::SessionClosed { mint: mint_pubkey });
                position_snapshots.write().remove(&mint_pubkey);
                market_contexts.write().remove(&mint_pubkey);
                stream_states.write().remove(&mint_pubkey);
            }
            Err(err) => {
                warn!(
                    event = "autosell_failed",
                    mint = %mint_pubkey,
                    position_id,
                    error = %err
                );
                warn!(event = "session_error", mint = %mint_pubkey, error = %err);
                emit(AppEvent::SessionError {
                    mint: mint_pubkey,
                    error: err.to_string(),
                });
            }
        }

        in_flight_auto_sells.lock().await.remove(&position_id);
    });

    Ok(())
}

async fn execute_auto_sell_with_refresh(
    stream_handle: Arc<StreamHandle>,
    mut refresh_rx: mpsc::UnboundedReceiver<String>,
    rpc_http: reqwest::Client,
    keypair_bytes: [u8; 64],
    rpc_url: String,
    local_mode: bool,
    mint: Pubkey,
    position_id: u64,
    sell_cfg: SellConfig,
    initial_unsigned_tx_b64: String,
) -> Result<(String, u16)> {
    let keypair = Keypair::try_from(&keypair_bytes[..]).context("decode keypair")?;
    let mut unsigned_tx_b64 = initial_unsigned_tx_b64;
    let mut attempt = 1usize;
    let mut refreshes_used = 0usize;
    let mut slippage_bps = sell_cfg.slippage_pad_bps;

    loop {
        emit(AppEvent::SellAttempt {
            mint,
            attempt,
            slippage_bps,
        });

        let send_result = async {
            let signed_tx = sign_unsigned_tx(&unsigned_tx_b64, &keypair)?;
            send_tx(
                &rpc_http,
                &rpc_url,
                &signed_tx,
                local_mode,
                Duration::from_secs(sell_cfg.confirm_timeout_sec),
            )
            .await
        }
        .await;

        match send_result {
            Ok(signature) => return Ok((signature, slippage_bps)),
            Err(err) => {
                if refreshes_used >= sell_cfg.max_retries {
                    return Err(anyhow!(
                        "autosell failed for position_id {position_id} after {attempt} attempts: {err}"
                    ));
                }
                emit(AppEvent::SellRetry {
                    mint,
                    attempt,
                    phase: classify_sell_retry_phase(&err).to_string(),
                    error: err.to_string(),
                });

                slippage_bps = bumped_slippage_bps(slippage_bps, refreshes_used, &sell_cfg);
                refreshes_used += 1;
                stream_handle
                    .request_exit_signal(position_id, Some(slippage_bps))
                    .context("request sell refresh over stream")?;
                unsigned_tx_b64 = recv_refreshed_sell_tx(&mut refresh_rx, position_id).await?;
                attempt += 1;
            }
        }
    }
}

fn classify_sell_retry_phase(err: &anyhow::Error) -> &'static str {
    if err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<TxSubmitError>(),
            Some(TxSubmitError::ConfirmTimeout { .. } | TxSubmitError::TxFailed { .. })
        )
    }) {
        "tx_confirm"
    } else {
        "tx_send"
    }
}

fn canonical_sell_reason(reason: &str) -> &str {
    match reason {
        "target" | "profit" | "target_profit" => "target",
        "stop_loss" => "stop_loss",
        "timeout" | "deadline_timeout" | "deadline" => "timeout",
        "manual" | "manual_sell" => "manual",
        _ => reason,
    }
}

async fn recv_refreshed_sell_tx(
    refresh_rx: &mut mpsc::UnboundedReceiver<String>,
    position_id: u64,
) -> Result<String> {
    loop {
        let next = tokio::time::timeout(
            Duration::from_millis(AUTOSELL_REFRESH_TIMEOUT_MS),
            refresh_rx.recv(),
        )
        .await
        .map_err(|_| {
            anyhow!("timed out waiting for refreshed sell tx (position_id={position_id})")
        })?;
        let payload =
            next.ok_or_else(|| anyhow!("sell refresh channel closed (position_id={position_id})"))?;
        let trimmed = payload.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
}

fn bumped_slippage_bps(current: u16, refreshes_used: usize, cfg: &SellConfig) -> u16 {
    let bump = if refreshes_used == 0 {
        cfg.slippage_retry_bump_bps_first
    } else {
        cfg.slippage_retry_bump_bps_next
    };
    current.saturating_add(bump).min(cfg.slippage_max_bps)
}

#[allow(clippy::too_many_arguments)]
async fn trigger_manual_sell(
    mint: Pubkey,
    exit_api: Arc<ExitApiClient>,
    rpc_http: reqwest::Client,
    keypair_bytes: [u8; 64],
    wallet_pubkey: Pubkey,
    rpc_url: String,
    local_mode: bool,
    runtime_sell: Arc<ParkingRwLock<SellConfig>>,
    position_snapshots: Arc<ParkingRwLock<HashMap<Pubkey, PositionSnapshot>>>,
    market_contexts: Arc<ParkingRwLock<HashMap<Pubkey, MarketContext>>>,
) {
    let snapshot = position_snapshots.read().get(&mint).cloned();
    let Some(snapshot) = snapshot else {
        warn!(event = "manual_sell_missing_position", mint = %mint);
        emit(AppEvent::SessionError {
            mint,
            error: "manual sell failed: no position data for mint".to_string(),
        });
        return;
    };

    if snapshot.tokens == 0 {
        warn!(event = "manual_sell_zero_tokens", mint = %mint);
        emit(AppEvent::SessionError {
            mint,
            error: "manual sell failed: position token balance is 0".to_string(),
        });
        return;
    }

    let sell_cfg = runtime_sell.read().clone();
    let slippage_bps = sell_cfg.slippage_pad_bps;
    let confirm_timeout_sec = sell_cfg.confirm_timeout_sec;
    let token_program_hint = snapshot.token_program.clone();
    let tokens = snapshot.tokens;
    let market_context = snapshot
        .market_context
        .clone()
        .or_else(|| market_contexts.read().get(&mint).cloned());

    let exit_api = exit_api.clone();
    let rpc_http = rpc_http.clone();
    let rpc_url = rpc_url.clone();
    tokio::spawn(async move {
        let sell_reason = canonical_sell_reason("manual_sell").to_string();
        let token_program =
            match resolve_token_program(&rpc_http, &rpc_url, &token_program_hint, mint).await {
                Ok(value) => value,
                Err(err) => {
                    emit(AppEvent::SessionError {
                        mint,
                        error: err.to_string(),
                    });
                    return;
                }
            };

        emit(AppEvent::SessionStarted {
            mint,
            token_program,
            started_at_ms: now_ms(),
        });
        emit(AppEvent::PositionTokensUpdated { mint, tokens });
        info!(
            event = "sell_scheduled",
            mint = %mint,
            reason = %sell_reason,
            profit_lamports = 0i64
        );
        emit(AppEvent::SellScheduled {
            mint,
            reason: sell_reason.clone(),
            profit_lamports: 0,
        });
        emit(AppEvent::SellAttempt {
            mint,
            attempt: 1,
            slippage_bps,
        });

        let result = execute_manual_sell(
            exit_api,
            rpc_http,
            keypair_bytes,
            mint,
            wallet_pubkey,
            rpc_url,
            local_mode,
            tokens,
            slippage_bps,
            confirm_timeout_sec,
            market_context,
        )
        .await;

        match result {
            Ok(signature) => {
                info!(
                    event = "sell_complete",
                    mint = %mint,
                    signature = %signature,
                    reason = %sell_reason,
                    slippage_bps
                );
                emit(AppEvent::SellComplete {
                    mint,
                    signature,
                    reason: sell_reason.clone(),
                    slippage_bps,
                });
                emit(AppEvent::SessionClosed { mint });
            }
            Err(err) => {
                warn!(event = "manual_sell_failed", mint = %mint, error = %err);
                warn!(event = "session_error", mint = %mint, error = %err);
                emit(AppEvent::SessionError {
                    mint,
                    error: err.to_string(),
                });
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn execute_manual_sell(
    exit_api: Arc<ExitApiClient>,
    rpc_http: reqwest::Client,
    keypair_bytes: [u8; 64],
    mint: Pubkey,
    wallet_pubkey: Pubkey,
    rpc_url: String,
    local_mode: bool,
    tokens: u64,
    slippage_bps: u16,
    confirm_timeout_sec: u64,
    market_context: Option<MarketContext>,
) -> Result<String> {
    let request = build_manual_sell_request(
        mint,
        wallet_pubkey,
        tokens,
        slippage_bps,
        market_context.as_ref(),
    )?;

    let unsigned_tx_b64 = exit_api
        .build_sell_tx_b64(&request)
        .await
        .context("build sell tx")?;
    let keypair = Keypair::try_from(&keypair_bytes[..]).context("decode keypair")?;
    let signed_tx = sign_unsigned_tx(&unsigned_tx_b64, &keypair)?;
    let signature = send_tx(
        &rpc_http,
        &rpc_url,
        &signed_tx,
        local_mode,
        Duration::from_secs(confirm_timeout_sec),
    )
    .await?;
    Ok(signature)
}

fn build_exit_api_client(
    api_key: &SecretString,
    local: bool,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<ExitApiClient> {
    let options = ExitApiClientOptions {
        connect_timeout,
        attempt_timeout: request_timeout,
        ..ExitApiClientOptions::default()
    };
    ExitApiClient::with_options(optional_api_key(api_key), options)
        .map(|client| client.with_local_mode(local))
        .context("build exit-api client")
}

fn optional_api_key(api_key: &SecretString) -> Option<SecretString> {
    let trimmed = api_key.expose_secret().trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(SecretString::new(trimmed.to_string()))
}

fn build_manual_sell_request(
    mint: Pubkey,
    wallet_pubkey: Pubkey,
    amount_tokens: u64,
    slippage_bps: u16,
    market_context: Option<&MarketContext>,
) -> Result<BuildSellTxRequest> {
    if amount_tokens == 0 {
        return Err(anyhow!("manual sell failed: amount_tokens must be > 0"));
    }

    Ok(BuildSellTxRequest {
        mint: mint.to_string(),
        user_pubkey: wallet_pubkey.to_string(),
        amount_tokens,
        slippage_bps: Some(slippage_bps),
        mode: None,
        output: Some(infer_manual_sell_output(market_context)),
        referral_id: None,
        market_context: market_context.map(market_context_to_msg),
    })
}

fn infer_manual_sell_output(market_context: Option<&MarketContext>) -> SellOutput {
    let Some(context) = market_context else {
        return SellOutput::Sol;
    };
    match context.market_type {
        MarketType::MeteoraDbc => {
            if context
                .meteora_dbc
                .as_ref()
                .map(|ctx| ctx.quote_mint == usd1_mint())
                .unwrap_or(false)
            {
                SellOutput::Usd1
            } else {
                SellOutput::Sol
            }
        }
        MarketType::RaydiumLaunchpad => {
            if context
                .raydium_launchpad
                .as_ref()
                .map(|ctx| ctx.quote_mint == usd1_mint())
                .unwrap_or(false)
            {
                SellOutput::Usd1
            } else {
                SellOutput::Sol
            }
        }
        MarketType::RaydiumCpmm => {
            if context
                .raydium_cpmm
                .as_ref()
                .map(|ctx| ctx.quote_mint == usd1_mint())
                .unwrap_or(false)
            {
                SellOutput::Usd1
            } else {
                SellOutput::Sol
            }
        }
        MarketType::PumpFun | MarketType::PumpSwap | MarketType::MeteoraDammV2 => SellOutput::Sol,
    }
}

fn strategy_to_msg(strategy: &StrategyConfig) -> StrategyConfigMsg {
    StrategyConfigMsg {
        target_profit_pct: strategy.target_profit.percent_value(),
        stop_loss_pct: strategy.stop_loss.percent_value(),
    }
}

fn apply_market_context_update(
    mint: Pubkey,
    market_context: Option<MarketContextMsg>,
    market_contexts: &ParkingRwLock<HashMap<Pubkey, MarketContext>>,
) -> Result<Option<MarketContext>> {
    let Some(msg) = market_context else {
        return Ok(None);
    };
    let context = market_context_from_msg(&msg)?;
    market_contexts.write().insert(mint, context.clone());
    Ok(Some(context))
}

fn upsert_market_stream_state(
    stream_states: &ParkingRwLock<HashMap<Pubkey, Arc<InMemoryMarketStreamState>>>,
    mint: Pubkey,
    market_context: Option<&MarketContext>,
    position_tokens: Option<u64>,
) {
    let mut states = stream_states.write();

    let state = match market_context {
        Some(context) => {
            let replace = states
                .get(&mint)
                .map(|state| state.market_type_value() != context.market_type)
                .unwrap_or(true);
            if replace {
                let created = Arc::new(InMemoryMarketStreamState::new(context.market_type));
                states.insert(mint, created.clone());
                created
            } else {
                states
                    .get(&mint)
                    .cloned()
                    .expect("stream state exists after replace check")
            }
        }
        None => {
            let Some(existing) = states.get(&mint).cloned() else {
                return;
            };
            existing
        }
    };

    if let Some(tokens) = position_tokens {
        state.set_position_tokens(Some(tokens));
    }

    drop(states);
    emit(AppEvent::SessionStreamState {
        mint,
        stream_state: state,
    });
}

async fn resolve_token_program(
    client: &reqwest::Client,
    rpc_url: &str,
    token_program_hint: &Option<String>,
    mint: Pubkey,
) -> Result<Pubkey> {
    if let Some(hint) = token_program_hint {
        if let Ok(program) = Pubkey::from_str(hint) {
            return Ok(program);
        }
    }

    let result = rpc_result(
        client,
        rpc_url,
        "getAccountInfo",
        serde_json::json!([
            mint.to_string(),
            {
                "encoding": "base64",
                "commitment": "processed"
            }
        ]),
    )
    .await?;

    let value = result
        .get("value")
        .cloned()
        .ok_or_else(|| anyhow!("mint account missing"))?;
    let owner = value
        .get("owner")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("mint owner missing"))?;
    let owner = Pubkey::from_str(owner).context("invalid token program owner")?;
    Ok(owner)
}

fn spawn_wallet_balance_fetch(rpc_http: reqwest::Client, rpc_url: String, wallet_pubkey: Pubkey) {
    tokio::spawn(async move {
        match fetch_wallet_balance(&rpc_http, &rpc_url, &wallet_pubkey).await {
            Ok(lamports) => {
                emit(AppEvent::BalanceUpdate { lamports });
            }
            Err(err) => {
                warn!(event = "wallet_balance_fetch_error", error = %err);
            }
        }
    });
}

fn spawn_usd1_balance_poller(rpc_http: reqwest::Client, rpc_url: String, wallet_pubkey: Pubkey) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            match fetch_usd1_balance(&rpc_http, &rpc_url, &wallet_pubkey).await {
                Ok(base_units) => {
                    emit(AppEvent::Usd1BalanceUpdate { base_units });
                }
                Err(err) => {
                    warn!(event = "usd1_balance_fetch_error", error = %err);
                }
            }
        }
    });
}

async fn fetch_wallet_balance(
    client: &reqwest::Client,
    rpc_url: &str,
    wallet_pubkey: &Pubkey,
) -> Result<u64> {
    let result = rpc_result(
        client,
        rpc_url,
        "getBalance",
        serde_json::json!([
            wallet_pubkey.to_string(),
            {
                "commitment": "processed"
            }
        ]),
    )
    .await?;

    result
        .get("value")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow!("wallet balance missing"))
}

#[derive(Clone, Debug)]
struct TokenAccountInfo {
    mint: Pubkey,
    owner: Pubkey,
    amount: u64,
}

async fn fetch_usd1_balance(
    client: &reqwest::Client,
    rpc_url: &str,
    wallet_pubkey: &Pubkey,
) -> Result<u64> {
    let mut total = 0u64;
    let programs = [spl_token::id(), spl_token_2022::id()];
    for program_id in programs {
        let result = rpc_result(
            client,
            rpc_url,
            "getTokenAccountsByOwner",
            serde_json::json!([
                wallet_pubkey.to_string(),
                { "programId": program_id.to_string() },
                { "encoding": "base64" }
            ]),
        )
        .await?;
        total = total.saturating_add(parse_token_accounts_for_mint(
            &result,
            *wallet_pubkey,
            usd1_mint(),
        ));
    }
    Ok(total)
}

fn parse_token_accounts_for_mint(result: &serde_json::Value, owner: Pubkey, mint: Pubkey) -> u64 {
    let Some(entries) = result.get("value").and_then(|value| value.as_array()) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries {
        let data_b64 = entry
            .get("account")
            .and_then(|value| value.get("data"))
            .and_then(|data| data.get(0))
            .and_then(|value| value.as_str())
            .or_else(|| {
                entry
                    .get("data")
                    .and_then(|data| data.get(0))
                    .and_then(|value| value.as_str())
            });
        let Some(data_b64) = data_b64 else {
            continue;
        };
        let data = match BASE64_STANDARD.decode(data_b64) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let Some(info) = decode_token_account(&data) else {
            continue;
        };
        if info.owner != owner || info.mint != mint {
            continue;
        }
        total = total.saturating_add(info.amount);
    }
    total
}

fn decode_token_account(data: &[u8]) -> Option<TokenAccountInfo> {
    if let Ok(account) = spl_token::state::Account::unpack_from_slice(data) {
        return Some(TokenAccountInfo {
            mint: account.mint,
            owner: account.owner,
            amount: account.amount,
        });
    }

    let account =
        spl_token_2022::extension::StateWithExtensions::<spl_token_2022::state::Account>::unpack(
            data,
        )
        .ok()?;
    Some(TokenAccountInfo {
        mint: account.base.mint,
        owner: account.base.owner,
        amount: account.base.amount,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use crate::market::{MarketContext, MeteoraDbcContext};

    use super::{build_manual_sell_request, canonical_sell_reason, usd1_mint};
    use solana_sdk::pubkey::Pubkey;

    #[test]
    fn manual_sell_request_serializes_output_and_slippage() {
        let request =
            build_manual_sell_request(Pubkey::new_unique(), Pubkey::new_unique(), 42, 1200, None)
                .expect("build manual sell request");
        let serialized = serde_json::to_value(request).expect("serialize request");

        assert_eq!(
            serialized
                .get("amount_tokens")
                .and_then(serde_json::Value::as_u64),
            Some(42)
        );
        assert_eq!(
            serialized
                .get("slippage_bps")
                .and_then(serde_json::Value::as_u64),
            Some(1200)
        );
        assert_eq!(
            serialized.get("output").and_then(serde_json::Value::as_str),
            Some("SOL")
        );
        assert!(serialized.get("amount").is_none());
    }

    #[test]
    fn manual_sell_request_uses_usd1_output_from_market_context() {
        let market_context = MarketContext::meteora_dbc(MeteoraDbcContext {
            pool: Pubkey::new_unique(),
            config: Pubkey::new_unique(),
            quote_mint: usd1_mint(),
        });
        let request = build_manual_sell_request(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            1,
            777,
            Some(&market_context),
        )
        .expect("build manual sell request");
        let serialized = serde_json::to_value(request).expect("serialize request");

        assert_eq!(
            serialized.get("output").and_then(serde_json::Value::as_str),
            Some("USD1")
        );
    }

    #[test]
    fn canonical_sell_reason_normalizes_deadline_to_timeout() {
        assert_eq!(canonical_sell_reason("deadline"), "timeout");
        assert_eq!(canonical_sell_reason("deadline_timeout"), "timeout");
        assert_eq!(canonical_sell_reason("timeout"), "timeout");
    }
}

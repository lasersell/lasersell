use solana_sdk::pubkey::Pubkey;

/// Fire-and-forget event emission. In CLI mode events are logged via tracing.
pub fn emit(event: AppEvent) {
    match &event {
        AppEvent::Startup { version, wallet_pubkey } => {
            tracing::info!(event = "startup", version = %version, wallet = %wallet_pubkey);
        }
        AppEvent::BalanceUpdate { lamports } => {
            tracing::debug!(event = "balance_update", lamports);
        }
        AppEvent::Usd1BalanceUpdate { base_units } => {
            tracing::debug!(event = "usd1_balance_update", base_units);
        }
        AppEvent::MintDetected { mint } => {
            tracing::info!(event = "mint_detected", mint = %mint);
        }
        AppEvent::SessionStarted { mint } => {
            tracing::info!(event = "session_started", mint = %mint);
        }
        AppEvent::PositionTokensUpdated { mint, tokens } => {
            tracing::debug!(event = "position_tokens_updated", mint = %mint, tokens);
        }
        AppEvent::CostBasisSet { mint, cost_basis_lamports } => {
            tracing::info!(event = "cost_basis_set", mint = %mint, cost_basis_lamports);
        }
        AppEvent::PnlUpdate { mint, profit_lamports, proceeds_lamports } => {
            tracing::debug!(event = "pnl_update", mint = %mint, profit_lamports, proceeds_lamports);
        }
        AppEvent::SellScheduled { mint, reason, profit_lamports } => {
            tracing::info!(event = "sell_scheduled", mint = %mint, reason = %reason, profit_lamports);
        }
        AppEvent::SellAttempt { mint, attempt, slippage_bps } => {
            tracing::info!(event = "sell_attempt", mint = %mint, attempt, slippage_bps);
        }
        AppEvent::SellRetry { mint, attempt, phase, error } => {
            tracing::warn!(event = "sell_retry", mint = %mint, attempt, phase = %phase, error = %error);
        }
        AppEvent::SellComplete { mint, signature, reason, slippage_bps } => {
            tracing::info!(event = "sell_complete", mint = %mint, signature = %signature, reason = %reason, slippage_bps);
        }
        AppEvent::SessionClosed { mint } => {
            tracing::info!(event = "session_closed", mint = %mint);
        }
        AppEvent::SessionError { mint, error } => {
            tracing::warn!(event = "session_error", mint = %mint, error = %error);
        }
        AppEvent::SolanaWsStatus { connected } => {
            if *connected {
                tracing::info!(event = "stream_connected");
            } else {
                tracing::warn!(event = "stream_disconnected");
            }
        }
        AppEvent::Heartbeat => {}
    }
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    Startup {
        version: String,
        wallet_pubkey: Pubkey,
    },
    BalanceUpdate {
        lamports: u64,
    },
    Usd1BalanceUpdate {
        base_units: u64,
    },
    SolanaWsStatus {
        connected: bool,
    },
    MintDetected {
        mint: Pubkey,
    },
    SessionStarted {
        mint: Pubkey,
    },
    PositionTokensUpdated {
        mint: Pubkey,
        tokens: u64,
    },
    CostBasisSet {
        mint: Pubkey,
        cost_basis_lamports: u64,
    },
    PnlUpdate {
        mint: Pubkey,
        profit_lamports: i64,
        proceeds_lamports: u64,
    },
    SellScheduled {
        mint: Pubkey,
        reason: String,
        profit_lamports: i64,
    },
    SellAttempt {
        mint: Pubkey,
        attempt: usize,
        slippage_bps: u16,
    },
    SellRetry {
        mint: Pubkey,
        attempt: usize,
        phase: String,
        error: String,
    },
    SellComplete {
        mint: Pubkey,
        signature: String,
        reason: String,
        slippage_bps: u16,
    },
    SessionClosed {
        mint: Pubkey,
    },
    SessionError {
        mint: Pubkey,
        error: String,
    },
    Heartbeat,
}

#[derive(Clone, Debug)]
pub enum AppCommand {
    Quit,
}

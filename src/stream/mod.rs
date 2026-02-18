use async_trait::async_trait;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::RwLock;

use crate::market::MarketType;

#[derive(Clone, Debug)]
pub struct BondingCurveAccount {
    pub complete: bool,
}

#[derive(Debug)]
pub struct InMemoryMarketStreamState {
    market_type: MarketType,
    latest_curve: RwLock<Option<BondingCurveAccount>>,
    latest_fee_bps: AtomicU64,
    p95_down_per_slot: AtomicU64,
    position_tokens: RwLock<Option<u64>>,
    quoted_tokens: AtomicU64,
    quoted_sell_proceeds: AtomicU64,
    has_quote: AtomicBool,
}

impl InMemoryMarketStreamState {
    pub fn new(market_type: MarketType) -> Self {
        Self {
            market_type,
            latest_curve: RwLock::new(None),
            latest_fee_bps: AtomicU64::new(0),
            p95_down_per_slot: AtomicU64::new(0),
            position_tokens: RwLock::new(None),
            quoted_tokens: AtomicU64::new(0),
            quoted_sell_proceeds: AtomicU64::new(0),
            has_quote: AtomicBool::new(false),
        }
    }

    pub fn market_type_value(&self) -> MarketType {
        self.market_type
    }

    pub fn set_position_tokens(&self, tokens: Option<u64>) {
        *self.position_tokens.write() = tokens;
    }
}

#[async_trait]
pub trait MarketStreamState: Send + Sync {
    fn market_type(&self) -> MarketType;
    async fn latest_curve(&self) -> Option<BondingCurveAccount>;
    fn latest_fee_bps(&self) -> u64;
    fn p95_down_per_slot(&self) -> u64;
    async fn position_tokens(&self) -> Option<u64>;
    async fn quote_sell_proceeds(&self, tokens: u64) -> Option<u64>;
}

#[async_trait]
impl MarketStreamState for InMemoryMarketStreamState {
    fn market_type(&self) -> MarketType {
        self.market_type
    }

    async fn latest_curve(&self) -> Option<BondingCurveAccount> {
        self.latest_curve.read().clone()
    }

    fn latest_fee_bps(&self) -> u64 {
        self.latest_fee_bps.load(Ordering::Relaxed)
    }

    fn p95_down_per_slot(&self) -> u64 {
        self.p95_down_per_slot.load(Ordering::Relaxed)
    }

    async fn position_tokens(&self) -> Option<u64> {
        *self.position_tokens.read()
    }

    async fn quote_sell_proceeds(&self, tokens: u64) -> Option<u64> {
        if !self.has_quote.load(Ordering::Relaxed) {
            return None;
        }
        if self.quoted_tokens.load(Ordering::Relaxed) != tokens {
            return None;
        }
        Some(self.quoted_sell_proceeds.load(Ordering::Relaxed))
    }
}

impl fmt::Debug for dyn MarketStreamState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MarketStreamState")
    }
}

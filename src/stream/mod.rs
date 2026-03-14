use parking_lot::RwLock;

use crate::market::MarketType;

#[derive(Debug)]
pub struct InMemoryMarketStreamState {
    market_type: MarketType,
    position_tokens: RwLock<Option<u64>>,
}

impl InMemoryMarketStreamState {
    pub fn new(market_type: MarketType) -> Self {
        Self {
            market_type,
            position_tokens: RwLock::new(None),
        }
    }

    pub fn market_type_value(&self) -> MarketType {
        self.market_type
    }

    pub fn set_position_tokens(&self, tokens: Option<u64>) {
        *self.position_tokens.write() = tokens;
    }
}

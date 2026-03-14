use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

pub mod context_from_msg;

pub const USD1_MINT: &str = "USD1ttGY1N17NEEHLmELoaybftRBUSErhqYiQzvEmuB";

pub fn usd1_mint() -> Pubkey {
    Pubkey::from_str(USD1_MINT).expect("USD1_MINT invalid")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketType {
    #[serde(alias = "pumpfun")]
    PumpFun,
    MeteoraDbc,
    #[serde(alias = "pumpswap")]
    PumpSwap,
    MeteoraDammV2,
    RaydiumLaunchpad,
    RaydiumCpmm,
}

#[derive(Clone, Copy, Debug)]
pub struct MarketContext {
    pub market_type: MarketType,
}

#[cfg(test)]
mod tests {
    use super::MarketType;

    #[test]
    fn market_type_deserialize_accepts_aliases() {
        let pump_swap: MarketType = serde_json::from_str("\"pump_swap\"").unwrap();
        assert_eq!(pump_swap, MarketType::PumpSwap);

        let pump_swap_alias: MarketType = serde_json::from_str("\"pumpswap\"").unwrap();
        assert_eq!(pump_swap_alias, MarketType::PumpSwap);

        let pump_fun: MarketType = serde_json::from_str("\"pump_fun\"").unwrap();
        assert_eq!(pump_fun, MarketType::PumpFun);

        let pump_fun_alias: MarketType = serde_json::from_str("\"pumpfun\"").unwrap();
        assert_eq!(pump_fun_alias, MarketType::PumpFun);
    }
}

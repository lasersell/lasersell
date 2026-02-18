use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

pub mod context_from_msg;
pub mod context_to_msg;

#[cfg(not(feature = "devnet"))]
pub const USD1_MINT: &str = "USD1ttGY1N17NEEHLmELoaybftRBUSErhqYiQzvEmuB";
#[cfg(feature = "devnet")]
pub const USD1_MINT: &str = "USDCoctVLVnvTXBEuP9s8hntucdJokbo17RwHuNXemT";

pub const USD1_DECIMALS: u8 = 6;

pub fn usd1_mint() -> Pubkey {
    Pubkey::from_str(USD1_MINT).expect("USD1_MINT invalid")
}

pub fn usd1_decimals() -> u8 {
    USD1_DECIMALS
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

impl MarketType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MarketType::PumpFun => "pumpfun",
            MarketType::MeteoraDbc => "meteora_dbc",
            MarketType::PumpSwap => "pumpswap",
            MarketType::MeteoraDammV2 => "meteora_damm_v2",
            MarketType::RaydiumLaunchpad => "raydium_launchpad",
            MarketType::RaydiumCpmm => "raydium_cpmm",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MeteoraDbcContext {
    pub pool: Pubkey,
    pub config: Pubkey,
    pub quote_mint: Pubkey,
}

#[derive(Clone, Debug)]
pub struct PumpSwapContext {
    pub pool: Pubkey,
    pub global_config: Option<Pubkey>,
}

#[derive(Clone, Debug)]
pub struct DammV2Context {
    pub pool: Pubkey,
}

#[derive(Clone, Debug)]
pub struct RaydiumLaunchpadContext {
    pub pool: Pubkey,
    pub config: Pubkey,
    pub platform: Pubkey,
    pub quote_mint: Pubkey,
    pub user_quote_account: Pubkey,
}

#[derive(Clone, Debug)]
pub struct RaydiumCpmmContext {
    pub pool: Pubkey,
    pub config: Pubkey,
    pub quote_mint: Pubkey,
    pub user_quote_account: Pubkey,
}

#[derive(Clone, Debug)]
pub struct MarketContext {
    pub market_type: MarketType,
    pub meteora_dbc: Option<MeteoraDbcContext>,
    pub pumpswap: Option<PumpSwapContext>,
    pub damm_v2: Option<DammV2Context>,
    pub raydium_launchpad: Option<RaydiumLaunchpadContext>,
    pub raydium_cpmm: Option<RaydiumCpmmContext>,
}

impl MarketContext {
    pub fn pumpfun() -> Self {
        Self {
            market_type: MarketType::PumpFun,
            meteora_dbc: None,
            pumpswap: None,
            damm_v2: None,
            raydium_launchpad: None,
            raydium_cpmm: None,
        }
    }

    pub fn meteora_dbc(context: MeteoraDbcContext) -> Self {
        Self {
            market_type: MarketType::MeteoraDbc,
            meteora_dbc: Some(context),
            pumpswap: None,
            damm_v2: None,
            raydium_launchpad: None,
            raydium_cpmm: None,
        }
    }

    pub fn pumpswap(context: PumpSwapContext) -> Self {
        Self {
            market_type: MarketType::PumpSwap,
            meteora_dbc: None,
            pumpswap: Some(context),
            damm_v2: None,
            raydium_launchpad: None,
            raydium_cpmm: None,
        }
    }

    pub fn meteora_damm_v2(context: DammV2Context) -> Self {
        Self {
            market_type: MarketType::MeteoraDammV2,
            meteora_dbc: None,
            pumpswap: None,
            damm_v2: Some(context),
            raydium_launchpad: None,
            raydium_cpmm: None,
        }
    }

    pub fn raydium_launchpad(context: RaydiumLaunchpadContext) -> Self {
        Self {
            market_type: MarketType::RaydiumLaunchpad,
            meteora_dbc: None,
            pumpswap: None,
            damm_v2: None,
            raydium_launchpad: Some(context),
            raydium_cpmm: None,
        }
    }

    pub fn raydium_cpmm(context: RaydiumCpmmContext) -> Self {
        Self {
            market_type: MarketType::RaydiumCpmm,
            meteora_dbc: None,
            pumpswap: None,
            damm_v2: None,
            raydium_launchpad: None,
            raydium_cpmm: Some(context),
        }
    }
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

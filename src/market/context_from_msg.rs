use lasersell_sdk::stream::proto::{MarketContextMsg, MarketTypeMsg};

use crate::market::{MarketContext, MarketType};

pub fn market_context_from_msg(msg: &MarketContextMsg) -> MarketContext {
    let market_type = match msg.market_type {
        MarketTypeMsg::PumpFun => MarketType::PumpFun,
        MarketTypeMsg::PumpSwap => MarketType::PumpSwap,
        MarketTypeMsg::MeteoraDbc => MarketType::MeteoraDbc,
        MarketTypeMsg::MeteoraDammV2 => MarketType::MeteoraDammV2,
        MarketTypeMsg::RaydiumLaunchpad => MarketType::RaydiumLaunchpad,
        MarketTypeMsg::RaydiumCpmm => MarketType::RaydiumCpmm,
    };
    MarketContext { market_type }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_context(market_type: MarketTypeMsg) -> MarketContextMsg {
        MarketContextMsg {
            market_type,
            pumpfun: None,
            pumpswap: None,
            meteora_dbc: None,
            meteora_damm_v2: None,
            raydium_launchpad: None,
            raydium_cpmm: None,
        }
    }

    #[test]
    fn converts_all_market_types() {
        let cases = [
            (MarketTypeMsg::PumpFun, MarketType::PumpFun),
            (MarketTypeMsg::PumpSwap, MarketType::PumpSwap),
            (MarketTypeMsg::MeteoraDbc, MarketType::MeteoraDbc),
            (MarketTypeMsg::MeteoraDammV2, MarketType::MeteoraDammV2),
            (MarketTypeMsg::RaydiumLaunchpad, MarketType::RaydiumLaunchpad),
            (MarketTypeMsg::RaydiumCpmm, MarketType::RaydiumCpmm),
        ];
        for (msg_type, expected) in cases {
            let ctx = market_context_from_msg(&empty_context(msg_type));
            assert_eq!(ctx.market_type, expected);
        }
    }
}

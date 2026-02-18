use lasersell_sdk::stream::proto::{
    MarketContextMsg, MarketTypeMsg, MeteoraDammV2ContextMsg, MeteoraDbcContextMsg,
    PumpFunContextMsg, PumpSwapContextMsg, RaydiumCpmmContextMsg, RaydiumLaunchpadContextMsg,
};

use crate::market::{MarketContext, MarketType};

pub fn market_context_to_msg(context: &MarketContext) -> MarketContextMsg {
    match context.market_type {
        MarketType::PumpFun => MarketContextMsg {
            market_type: MarketTypeMsg::PumpFun,
            pumpfun: Some(PumpFunContextMsg {}),
            pumpswap: None,
            meteora_dbc: None,
            meteora_damm_v2: None,
            raydium_launchpad: None,
            raydium_cpmm: None,
        },
        MarketType::PumpSwap => {
            let ctx = context
                .pumpswap
                .as_ref()
                .expect("pumpswap context missing for PumpSwap market");
            MarketContextMsg {
                market_type: MarketTypeMsg::PumpSwap,
                pumpfun: None,
                pumpswap: Some(PumpSwapContextMsg {
                    pool: ctx.pool.to_string(),
                    global_config: ctx.global_config.map(|pk| pk.to_string()),
                }),
                meteora_dbc: None,
                meteora_damm_v2: None,
                raydium_launchpad: None,
                raydium_cpmm: None,
            }
        }
        MarketType::MeteoraDbc => {
            let ctx = context
                .meteora_dbc
                .as_ref()
                .expect("meteora_dbc context missing for MeteoraDbc market");
            MarketContextMsg {
                market_type: MarketTypeMsg::MeteoraDbc,
                pumpfun: None,
                pumpswap: None,
                meteora_dbc: Some(MeteoraDbcContextMsg {
                    pool: ctx.pool.to_string(),
                    config: ctx.config.to_string(),
                    quote_mint: ctx.quote_mint.to_string(),
                }),
                meteora_damm_v2: None,
                raydium_launchpad: None,
                raydium_cpmm: None,
            }
        }
        MarketType::MeteoraDammV2 => {
            let ctx = context
                .damm_v2
                .as_ref()
                .expect("damm_v2 context missing for MeteoraDammV2 market");
            MarketContextMsg {
                market_type: MarketTypeMsg::MeteoraDammV2,
                pumpfun: None,
                pumpswap: None,
                meteora_dbc: None,
                meteora_damm_v2: Some(MeteoraDammV2ContextMsg {
                    pool: ctx.pool.to_string(),
                }),
                raydium_launchpad: None,
                raydium_cpmm: None,
            }
        }
        MarketType::RaydiumLaunchpad => {
            let ctx = context
                .raydium_launchpad
                .as_ref()
                .expect("raydium_launchpad context missing for RaydiumLaunchpad market");
            MarketContextMsg {
                market_type: MarketTypeMsg::RaydiumLaunchpad,
                pumpfun: None,
                pumpswap: None,
                meteora_dbc: None,
                meteora_damm_v2: None,
                raydium_launchpad: Some(RaydiumLaunchpadContextMsg {
                    pool: ctx.pool.to_string(),
                    config: ctx.config.to_string(),
                    platform: ctx.platform.to_string(),
                    quote_mint: ctx.quote_mint.to_string(),
                    user_quote_account: ctx.user_quote_account.to_string(),
                }),
                raydium_cpmm: None,
            }
        }
        MarketType::RaydiumCpmm => {
            let ctx = context
                .raydium_cpmm
                .as_ref()
                .expect("raydium_cpmm context missing for RaydiumCpmm market");
            MarketContextMsg {
                market_type: MarketTypeMsg::RaydiumCpmm,
                pumpfun: None,
                pumpswap: None,
                meteora_dbc: None,
                meteora_damm_v2: None,
                raydium_launchpad: None,
                raydium_cpmm: Some(RaydiumCpmmContextMsg {
                    pool: ctx.pool.to_string(),
                    config: ctx.config.to_string(),
                    quote_mint: ctx.quote_mint.to_string(),
                    user_quote_account: ctx.user_quote_account.to_string(),
                }),
            }
        }
    }
}

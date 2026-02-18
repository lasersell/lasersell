use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use lasersell_sdk::stream::proto::{MarketContextMsg, MarketTypeMsg};
use solana_sdk::pubkey::Pubkey;

use crate::market::{
    DammV2Context, MarketContext, MeteoraDbcContext, PumpSwapContext, RaydiumCpmmContext,
    RaydiumLaunchpadContext,
};

fn parse_pubkey(field: &str, value: &str) -> Result<Pubkey> {
    Pubkey::from_str(value).with_context(|| format!("invalid pubkey for {field}"))
}

pub fn market_context_from_msg(msg: &MarketContextMsg) -> Result<MarketContext> {
    match msg.market_type {
        MarketTypeMsg::PumpFun => Ok(MarketContext::pumpfun()),
        MarketTypeMsg::PumpSwap => {
            let ctx = msg
                .pumpswap
                .as_ref()
                .ok_or_else(|| anyhow!("pumpswap context missing"))?;
            Ok(MarketContext::pumpswap(PumpSwapContext {
                pool: parse_pubkey("pumpswap.pool", &ctx.pool)?,
                global_config: match &ctx.global_config {
                    Some(value) => Some(parse_pubkey("pumpswap.global_config", value)?),
                    None => None,
                },
            }))
        }
        MarketTypeMsg::MeteoraDbc => {
            let ctx = msg
                .meteora_dbc
                .as_ref()
                .ok_or_else(|| anyhow!("meteora_dbc context missing"))?;
            Ok(MarketContext::meteora_dbc(MeteoraDbcContext {
                pool: parse_pubkey("meteora_dbc.pool", &ctx.pool)?,
                config: parse_pubkey("meteora_dbc.config", &ctx.config)?,
                quote_mint: parse_pubkey("meteora_dbc.quote_mint", &ctx.quote_mint)?,
            }))
        }
        MarketTypeMsg::MeteoraDammV2 => {
            let ctx = msg
                .meteora_damm_v2
                .as_ref()
                .ok_or_else(|| anyhow!("meteora_damm_v2 context missing"))?;
            Ok(MarketContext::meteora_damm_v2(DammV2Context {
                pool: parse_pubkey("meteora_damm_v2.pool", &ctx.pool)?,
            }))
        }
        MarketTypeMsg::RaydiumLaunchpad => {
            let ctx = msg
                .raydium_launchpad
                .as_ref()
                .ok_or_else(|| anyhow!("raydium_launchpad context missing"))?;
            Ok(MarketContext::raydium_launchpad(RaydiumLaunchpadContext {
                pool: parse_pubkey("raydium_launchpad.pool", &ctx.pool)?,
                config: parse_pubkey("raydium_launchpad.config", &ctx.config)?,
                platform: parse_pubkey("raydium_launchpad.platform", &ctx.platform)?,
                quote_mint: parse_pubkey("raydium_launchpad.quote_mint", &ctx.quote_mint)?,
                user_quote_account: parse_pubkey(
                    "raydium_launchpad.user_quote_account",
                    &ctx.user_quote_account,
                )?,
            }))
        }
        MarketTypeMsg::RaydiumCpmm => {
            let ctx = msg
                .raydium_cpmm
                .as_ref()
                .ok_or_else(|| anyhow!("raydium_cpmm context missing"))?;
            Ok(MarketContext::raydium_cpmm(RaydiumCpmmContext {
                pool: parse_pubkey("raydium_cpmm.pool", &ctx.pool)?,
                config: parse_pubkey("raydium_cpmm.config", &ctx.config)?,
                quote_mint: parse_pubkey("raydium_cpmm.quote_mint", &ctx.quote_mint)?,
                user_quote_account: parse_pubkey(
                    "raydium_cpmm.user_quote_account",
                    &ctx.user_quote_account,
                )?,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::MarketType;
    use lasersell_sdk::stream::proto::{
        MeteoraDammV2ContextMsg, MeteoraDbcContextMsg, PumpSwapContextMsg, RaydiumCpmmContextMsg,
        RaydiumLaunchpadContextMsg,
    };

    fn pk(seed: u8) -> (Pubkey, String) {
        let key = Pubkey::new_from_array([seed; 32]);
        (key, key.to_string())
    }

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
    fn converts_pumpfun_context() {
        let msg = empty_context(MarketTypeMsg::PumpFun);
        let ctx = market_context_from_msg(&msg).expect("convert pumpfun");
        assert_eq!(ctx.market_type, MarketType::PumpFun);
    }

    #[test]
    fn converts_pumpswap_context() {
        let (pool_pk, pool) = pk(1);
        let (global_pk, global_config) = pk(2);
        let mut msg = empty_context(MarketTypeMsg::PumpSwap);
        msg.pumpswap = Some(PumpSwapContextMsg {
            pool,
            global_config: Some(global_config),
        });
        let ctx = market_context_from_msg(&msg).expect("convert pumpswap");
        assert_eq!(ctx.market_type, MarketType::PumpSwap);
        let inner = ctx.pumpswap.expect("pumpswap context");
        assert_eq!(inner.pool, pool_pk);
        assert_eq!(inner.global_config, Some(global_pk));
    }

    #[test]
    fn converts_meteora_dbc_context() {
        let (pool_pk, pool) = pk(1);
        let (config_pk, config) = pk(2);
        let (quote_pk, quote_mint) = pk(3);
        let mut msg = empty_context(MarketTypeMsg::MeteoraDbc);
        msg.meteora_dbc = Some(MeteoraDbcContextMsg {
            pool,
            config,
            quote_mint,
        });
        let ctx = market_context_from_msg(&msg).expect("convert meteora_dbc");
        assert_eq!(ctx.market_type, MarketType::MeteoraDbc);
        let inner = ctx.meteora_dbc.expect("meteora_dbc context");
        assert_eq!(inner.pool, pool_pk);
        assert_eq!(inner.config, config_pk);
        assert_eq!(inner.quote_mint, quote_pk);
    }

    #[test]
    fn converts_meteora_damm_v2_context() {
        let (pool_pk, pool) = pk(4);
        let mut msg = empty_context(MarketTypeMsg::MeteoraDammV2);
        msg.meteora_damm_v2 = Some(MeteoraDammV2ContextMsg { pool });
        let ctx = market_context_from_msg(&msg).expect("convert meteora_damm_v2");
        assert_eq!(ctx.market_type, MarketType::MeteoraDammV2);
        let inner = ctx.damm_v2.expect("damm_v2 context");
        assert_eq!(inner.pool, pool_pk);
    }

    #[test]
    fn converts_raydium_launchpad_context() {
        let (pool_pk, pool) = pk(5);
        let (config_pk, config) = pk(6);
        let (platform_pk, platform) = pk(7);
        let (quote_pk, quote_mint) = pk(8);
        let (user_pk, user_quote_account) = pk(9);
        let mut msg = empty_context(MarketTypeMsg::RaydiumLaunchpad);
        msg.raydium_launchpad = Some(RaydiumLaunchpadContextMsg {
            pool,
            config,
            platform,
            quote_mint,
            user_quote_account,
        });
        let ctx = market_context_from_msg(&msg).expect("convert raydium_launchpad");
        assert_eq!(ctx.market_type, MarketType::RaydiumLaunchpad);
        let inner = ctx.raydium_launchpad.expect("raydium_launchpad context");
        assert_eq!(inner.pool, pool_pk);
        assert_eq!(inner.config, config_pk);
        assert_eq!(inner.platform, platform_pk);
        assert_eq!(inner.quote_mint, quote_pk);
        assert_eq!(inner.user_quote_account, user_pk);
    }

    #[test]
    fn converts_raydium_cpmm_context() {
        let (pool_pk, pool) = pk(10);
        let (config_pk, config) = pk(11);
        let (quote_pk, quote_mint) = pk(12);
        let (user_pk, user_quote_account) = pk(13);
        let mut msg = empty_context(MarketTypeMsg::RaydiumCpmm);
        msg.raydium_cpmm = Some(RaydiumCpmmContextMsg {
            pool,
            config,
            quote_mint,
            user_quote_account,
        });
        let ctx = market_context_from_msg(&msg).expect("convert raydium_cpmm");
        assert_eq!(ctx.market_type, MarketType::RaydiumCpmm);
        let inner = ctx.raydium_cpmm.expect("raydium_cpmm context");
        assert_eq!(inner.pool, pool_pk);
        assert_eq!(inner.config, config_pk);
        assert_eq!(inner.quote_mint, quote_pk);
        assert_eq!(inner.user_quote_account, user_pk);
    }
}

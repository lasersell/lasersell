# LaserSell CLI v1.0.0

LaserSell CLI is now a headless daemon. The TUI has been removed in favor of structured log output, making the CLI ideal for VPS, Docker, and systemd deployments. This release also adds support for every v1.0 SDK feature.

## What's New

### Exit Ladder
Sell partial amounts at multiple profit thresholds. Each level specifies the profit % to trigger, the % of the position to sell, and an optional trailing stop for that tranche.

### Liquidity Guard
Monitor pool depth and automatically exit when liquidity drops below a safe threshold.

### Breakeven Trail
A trailing stop that activates once your position breaks even. Protects gains without cutting winners early.

### Copy Trading
Watch external wallets and optionally auto-buy when they buy. Positions from watched wallets use the same exit strategy as your own. Configure per-wallet SOL and USD1 buy amounts.

### Wallet Registration
The CLI now proves wallet ownership via Ed25519 signature before connecting to the stream. Registration failures surface as clear errors instead of silent downstream failures.

### Priority Lanes
Exit signals are delivered on a high-priority channel so they are never delayed by PnL updates.

## Breaking Changes

- **TUI removed.** The CLI no longer has an interactive terminal UI. All output is structured log events via tracing.
- **`--no-tui` and `--verbose` flags removed.** CLI-only mode is now the default and only mode.
- **No manual sell.** The CLI is a passive exit daemon. Use the desktop app or SDK for manual sells.
- **No pause/resume.** Stop the daemon with Ctrl+C instead.
- **Default RPC changed** from `api.mainnet-beta.solana.com` to `solana-rpc.publicnode.com`.

## CLI Flags

| Flag | Description |
|------|-------------|
| `--setup` | Run the interactive onboarding wizard |
| `--smoke` | Run a connectivity health check and exit |
| `--debug` | Write debug-level logs to `~/.lasersell/debug.log` |
| `--export-private-key` | Export wallet's base58 private key to stdout |
| `-f <path>` | Path to a custom config file |

## Supported DEXs

Pump.fun, PumpSwap, Raydium (Launchpad, CPMM), Meteora (DBC, DAMM v2), Bags.fm.

## Upgrade

```
curl -fsSL https://dl.lasersell.io/install.sh | bash
```

Existing `~/.lasersell/config.yml` files are fully compatible. New fields (`take_profit_levels`, `liquidity_guard`, `breakeven_trail`, `watch_wallets`) default to disabled.

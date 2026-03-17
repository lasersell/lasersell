<p align="center">
  <a href="https://lasersell.io">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://lasersell.io/brand/lasersell-dark-full-1.png">
      <img alt="LaserSell" src="https://lasersell.io/brand/lasersell-light-full-2.png" width="260">
    </picture>
  </a>
</p>

<p align="center">
  <strong>LaserSell CLI</strong><br>
  Open-source Solana exit daemon.<br>
  Automated exits. Real-time position monitoring. Non-custodial.
</p>

<p align="center">
  <a href="https://www.lasersell.io/download"><strong>Download the Desktop App &rarr;</strong></a>
</p>

<p align="center">
  <a href="https://crates.io/crates/lasersell"><img alt="crates.io" src="https://img.shields.io/crates/v/lasersell.svg"></a>
  <a href="https://hub.docker.com/r/lasersell/lasersell"><img alt="Docker" src="https://img.shields.io/docker/v/lasersell/lasersell?label=docker&logo=docker"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <br>
  <a href="https://discord.gg/lasersell"><img alt="Discord" src="https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://x.com/lasersellhq"><img alt="X (Twitter)" src="https://img.shields.io/twitter/follow/lasersellhq?style=flat&logo=x&color=000000"></a>
</p>

---

> **New to LaserSell?** Read [Your First Automated Exit](https://www.lasersell.io/blog/your-first-automated-exit) for a full walkthrough from install to your first automated sell.

## What is the LaserSell CLI?

The LaserSell CLI is an open-source daemon that automatically sells Solana tokens when your exit conditions are met. It connects to the LaserSell stream, a server-side position monitor that watches your holdings in real time and delivers ready-to-sign exit transactions the moment your strategy triggers.

You configure your strategy, start the daemon, and exits execute automatically.

```bash
lasersell --setup   # one-time: RPC, API key, strategy, wallet
lasersell           # start the daemon
```

**Supported DEXs:** Pump.fun, PumpSwap, Raydium (Launchpad, CPMM), Meteora (DBC, DAMM v2), Bags.fm. SOL and USD1 quote currencies.

**Non-custodial and secure by design.** Your private key never leaves your machine. LaserSell stores it in an encrypted keystore, signs transactions locally, and submits directly to the network. The server only sees your public key. It builds unsigned transactions and sends them to you.

#### Features

- **Automated exit strategies.** Take-profit, stop-loss, trailing stop, deadline timeout, sell-on-graduation, exit ladder (multi-level take-profit), liquidity guard, and breakeven trail. LaserSell auto-sells when any condition is met.
- **Copy trading.** Watch other wallets and optionally auto-buy when they buy. Monitored positions are managed by the same exit strategy.
- **Adaptive slippage.** Slippage starts at your configured baseline and bumps automatically on retries, up to a hard cap you control.
- **Headless operation.** Designed for VPS and server deployments with structured log output.
- **Graceful shutdown.** Ctrl+C cleanly shuts down all connections.

#### Why LaserSell?

- **Rug pull protection.** Automatically exits positions the moment conditions deteriorate, helping prevent total profit loss.
- **Built for high frequency trading.** No polling, no stale data, and adaptive retries.
- **Locks in profit instantly.** Secures gains the moment a profit window is available, before the market moves against you.

## How it works

LaserSell connects to the LaserSell stream over WebSocket. The stream monitors your positions server-side against your configured thresholds and pushes a **pre-built unsigned transaction** to your client the instant conditions are met. Your client signs locally and submits in one step with no polling and no stale data.

This is fundamentally different from bots that poll a price API and then build a transaction (two steps, each with latency). It's also different from limit orders, which sit passively on-chain and get skipped when the price gaps past them during rapid dumps. The stream fires an immediate market swap using real-time on-chain data.

## Installation

```bash
curl -fsSL https://dl.lasersell.io/install.sh | sh
```

Supports macOS, Linux, Windows (WSL), and Raspberry Pi. The installer auto-detects your platform and uses Homebrew, APT, or a standalone binary as appropriate. You can also install a specific version with `--version X.Y.Z`.

## Quick start

### 1. Get an API key

Sign up for free at [app.lasersell.io](https://app.lasersell.io/auth/sign-up) and create an API key from the dashboard. No credit card required.

### 2. Run the setup wizard

```bash
lasersell --setup
```

The wizard walks you through:
- Connecting your Solana RPC endpoint
- Entering your LaserSell API key
- Setting your default strategy (take-profit, stop-loss, trailing stop, deadline, slippage)
- Creating or importing a wallet (seed phrase, Solana JSON keypair, or base58 secret key)

Config and keystore are saved to `~/.lasersell/`.

### 3. Start the daemon

```bash
lasersell
```

LaserSell connects to the stream, monitors your positions, and auto-sells when your strategy triggers. Press Ctrl+C to gracefully shut down.

## Configuration

LaserSell reads from `~/.lasersell/config.yml`. Override with `-f path/to/config.yml` or environment variables.

> **Note:** The strategy values below are examples only, not an official trading strategy. Configure based on your own risk tolerance.

```yaml
account:
  rpc_url: "https://your-rpc-endpoint.com"
  api_key: "your-lasersell-api-key"

strategy:
  target_profit: "20%"       # take-profit as % of buy amount
  stop_loss: "10%"           # stop-loss as % of buy amount (0% disables)
  trailing_stop: "5%"        # exit when profit drops this % of entry from peak (0% disables)
  deadline_timeout: 0        # force-sell after N seconds (0 disables)
  sell_on_graduation: false  # auto-sell when token graduates (e.g. Pump.fun -> PumpSwap)
  liquidity_guard: false     # exit when liquidity drops below safe threshold
  breakeven_trail: "0%"      # trailing stop from breakeven point (0% disables)
  take_profit_levels: []     # exit ladder: multi-level take-profit (see docs)

sell:
  slippage_pad_bps: 2500     # base slippage (basis points)
  slippage_max_bps: 3000     # hard cap
  max_retries: 3

watch_wallets: []            # copy trading: list of wallets to mirror
# - pubkey: "WalletPubkeyHere"
#   label: "trader1"
#   auto_buy:
#     amount: 0.1            # SOL amount to auto-buy
#     amount_usd1: 0.0       # USD1 amount to auto-buy
```

See [`config.example.yml`](config.example.yml) for all options with inline documentation.

<details>
<summary>Environment variable overrides</summary>

| Variable | Overrides |
|----------|-----------|
| `LASERSELL_CONFIG_PATH` | Config file path |
| `LASERSELL_KEYPAIR_PATH` | `account.keypair_path` |
| `LASERSELL_RPC_URL` | `account.rpc_url` |
| `LASERSELL_API_KEY` | `account.api_key` |
| `LASERSELL_SEND_TARGET` | `account.send_target` |
| `LASERSELL_WALLET_PASSPHRASE` | Keystore passphrase (headless unlock) |

</details>

<details>
<summary>CLI flags</summary>

```
lasersell                          # Start the daemon
lasersell --setup                  # Interactive onboarding wizard
lasersell --debug                  # Write debug-level logs to debug.log
lasersell --smoke                  # Health check: connect, verify, exit
lasersell --export-private-key     # Print base58 private key to stdout
lasersell -f /path/to/config.yml   # Use a specific config file
```

</details>

<details>
<summary>Transaction submission targets</summary>

Configure via `account.send_target` in your config or `LASERSELL_SEND_TARGET`:

| Target | Description |
|--------|-------------|
| `helius_sender` | Default. Routes through Helius for optimized landing. |
| `astralane` | Alternative sender (requires `astralane_api_key`). |
| `rpc` | Direct submission to your Solana RPC endpoint. |

</details>

## Security

LaserSell is non-custodial. Private keys never leave your machine and are never transmitted to LaserSell servers.

- **Encrypted keystore.** Argon2id key derivation + XChaCha20-Poly1305 authenticated encryption at rest.
- **Log redaction.** RPC URLs, API keys, and auth headers are automatically scrubbed from all log output.
- **Memory safety.** Sensitive data (keypair bytes, passphrases) is zeroized after use.
- **Open source.** Full auditability.

Learn more at [lasersell.io/security](https://www.lasersell.io/security).

## FAQ

### How does the LaserSell CLI relate to the SDK?

The CLI is built on top of the [LaserSell SDK](https://github.com/lasersell/lasersell-sdk). It's a ready-to-use exit daemon. The SDK is for developers who want to build their own applications, bots, or integrations on top of the LaserSell API.

### How does the CLI relate to the desktop app?

The [LaserSell Desktop App](https://lasersell.io) provides a full GUI with portfolio management, charts, achievements, and more. The CLI is a lightweight alternative for headless servers, VPS deployments, or users who prefer terminal-based workflows.

### What happens if my machine goes offline?

LaserSell runs locally, so if your machine is off, no exits will fire. For always-on operation, run it on a VPS.

### Do I need a private RPC?

Strongly recommended. LaserSell works with the public Solana RPC, but you'll get slower balance updates (60s vs 5s polling) and transaction confirmation will be less reliable under load.

## Building from source

```bash
# Production build
cargo build --release

# Devnet build (devnet endpoints + debug logging)
cargo build --features devnet
```

## LaserSell ecosystem

| Resource | Link |
|----------|------|
| Getting started | [Your First Automated Exit](https://www.lasersell.io/blog/your-first-automated-exit) |
| Website | [lasersell.io](https://lasersell.io) |
| Desktop App | [lasersell.io](https://lasersell.io) |
| Dashboard & API keys | [app.lasersell.io](https://app.lasersell.io) |
| Documentation | [docs.lasersell.io](https://docs.lasersell.io) |
| Blog | [lasersell.io/blog](https://www.lasersell.io/blog) |
| Benchmarks | [38x faster than every major Solana trading API](https://www.lasersell.io/blog/benchmark-results) |
| SDKs (Rust, TypeScript, Python, Go) | [github.com/lasersell/lasersell-sdk](https://github.com/lasersell/lasersell-sdk) |
| Discord | [discord.gg/lasersell](https://discord.gg/lasersell) |
| X (Twitter) | [@lasersellhq](https://x.com/lasersellhq) |

## License

[MIT](LICENSE)

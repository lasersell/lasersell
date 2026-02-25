<p align="center">
  <a href="https://lasersell.io">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://lasersell.io/brand/lasersell-dark-full-1.png">
      <img alt="LaserSell" src="https://lasersell.io/brand/lasersell-light-full-2.png" width="260">
    </picture>
  </a>
</p>

<p align="center">
  Open-source Solana exit daemon.<br>
  Automated exits. Real-time position monitoring. Non-custodial.
</p>

<p align="center">
  <a href="https://crates.io/crates/lasersell"><img alt="crates.io" src="https://img.shields.io/crates/v/lasersell.svg"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <br>
  <a href="https://discord.gg/lasersell"><img alt="Discord" src="https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://x.com/lasersellhq"><img alt="X (Twitter)" src="https://img.shields.io/twitter/follow/lasersellhq?style=flat&logo=x&color=000000"></a>
</p>

---

## What is LaserSell?

LaserSell is an open-source CLI daemon that automatically sells Solana tokens when your exit conditions are met. It connects to the Exit Intelligence stream, a server-side position monitor that watches your holdings in real time and delivers ready-to-sign exit transactions the moment your take-profit, stop-loss, or deadline triggers.

You configure your strategy, LaserSell runs in your terminal (or headless on a server), and exits execute automatically.

```bash
lasersell --setup   # one-time: RPC, API key, strategy, wallet
lasersell           # start the daemon
```

**Supported DEXs:** Pump.fun, PumpSwap, Raydium (Launchpad, CPMM), Meteora (DBC, DAMM v2), Bags.fm. SOL and USD1 quote currencies.

🔒 **Non-custodial and secure by design.** Your private key never leaves your machine. LaserSell stores it in an encrypted keystore, signs transactions locally, and submits directly to the network. The server only sees your public key. It builds unsigned transactions and sends them to you.

#### Benefits

- **Rug pull protection.** Automatically exits positions the moment conditions deteriorate, helping prevent total profit loss.
- **Built for high frequency trading.** Sub-200ms exit delivery with no polling, no stale data, and adaptive retries.
- **Locks in profit instantly.** Secures gains the moment a profit window is available, before the market moves against you.
- **Makes trading feel like a video game.** A live terminal dashboard with real-time PnL, session status, and interactive controls.

## How it works

LaserSell connects to the Exit Intelligence stream over WebSocket. The stream monitors your positions server-side against your configured thresholds and pushes a **pre-built unsigned transaction** to your client the instant conditions are met. Your client signs locally and submits in one step with no polling and no stale data.

This is fundamentally different from bots that poll a price API and then build a transaction (two steps, each with latency). It's also different from limit orders, which sit passively on-chain and get skipped when the price gaps past them during rapid dumps. Exit Intelligence fires an immediate market swap using real-time on-chain data.

## What can you do with the LaserSell CLI?

- **Automated exit strategies.** Set take-profit, stop-loss, and deadline timeout. LaserSell auto-sells when any condition is met. Buy a token in any supported DEX, and LaserSell picks it up automatically.
- **Live terminal dashboard.** Real-time PnL tracking, session status, strategy controls, and a built-in command prompt.
- **Headless server deployments.** Run with `--no-tui` on a VPS for always-on operation with plain log output.
- **On-the-fly strategy changes.** Adjust take-profit, stop-loss, slippage, and timeout from the TUI command pane. Changes apply immediately and persist to disk.
- **Adaptive slippage.** Slippage starts at your configured baseline and bumps automatically on retries, up to a hard cap you control.

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
- Setting your default strategy (take-profit, stop-loss, deadline, slippage)
- Creating or importing a wallet (seed phrase, Solana JSON keypair, or base58 secret key)

Config and keystore are saved to `~/.lasersell/`.

### 3. Start the daemon

```bash
lasersell
```

LaserSell connects to the Exit Intelligence stream, monitors your positions, and auto-sells when your strategy triggers.

### 4. Adjust on the fly

Inside the TUI, press `Tab` to switch to the command pane:

```
set tp 20%          # change take-profit to 20%
set sl 5%           # set stop-loss to 5%
set timeout 60      # force-sell after 60 seconds
sell                 # manually sell the selected position
pause / resume       # pause or resume new sessions
?                    # show all commands
```

## Configuration

LaserSell reads from `~/.lasersell/config.yml`. Override with `-f path/to/config.yml` or environment variables.

```yaml
account:
  rpc_url: "https://your-rpc-endpoint.com"
  api_key: "your-lasersell-api-key"

strategy:
  target_profit: "10%"       # take-profit as % of buy amount
  stop_loss: "0%"            # 0% disables stop-loss
  deadline_timeout: 30       # force-sell after N seconds (0 disables)

sell:
  slippage_pad_bps: 2000     # base slippage (basis points)
  slippage_max_bps: 2500     # hard cap
  max_retries: 2
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
lasersell                          # TUI mode (default)
lasersell --setup                  # Interactive onboarding wizard
lasersell --no-tui                 # Headless mode (plain log output)
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

### What happens if my machine goes offline?

LaserSell runs locally, so if your machine is off, no exits will fire. For always-on operation, run it on a VPS with `--no-tui`.

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
| Website | [lasersell.io](https://lasersell.io) |
| Dashboard & API keys | [app.lasersell.io](https://app.lasersell.io) |
| Documentation | [docs.lasersell.io](https://docs.lasersell.io) |
| SDKs (Rust, TypeScript, Python, Go) | [github.com/lasersell/lasersell-sdk](https://github.com/lasersell/lasersell-sdk) |
| Discord | [discord.gg/lasersell](https://discord.gg/lasersell) |
| X (Twitter) | [@lasersellhq](https://x.com/lasersellhq) |

## License

[MIT](LICENSE)

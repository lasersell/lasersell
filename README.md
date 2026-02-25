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
  Automated sells. Terminal UI. Non-custodial.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <br>
  <a href="https://discord.gg/lasersell"><img alt="Discord" src="https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://x.com/lasersellhq"><img alt="X (Twitter)" src="https://img.shields.io/twitter/follow/lasersellhq?style=flat&logo=x&color=000000"></a>
</p>

---

## What is LaserSell?

LaserSell is a single-wallet CLI daemon that listens for exit signals from the [LaserSell Exit Intelligence](https://docs.lasersell.io/core-concepts/comparison-laser-sell-vs-standard-market-orders) stream and automatically signs and submits sell transactions on Solana. Set your take-profit, stop-loss, and deadline, and LaserSell fires the instant conditions are met.

- **Terminal UI** — live session list, PnL tracking, strategy controls, and a command prompt, all in your terminal.
- **Headless mode** — run with `--no-tui` for scripted or server deployments.
- **Non-custodial** — your private key never leaves your machine. Transactions are signed locally with an encrypted keystore (Argon2id + XChaCha20-Poly1305).
- **Adaptive slippage** — starts at your configured baseline and bumps automatically on retries, up to a hard cap you control.

### Supported DEXs

Pump.fun, PumpSwap, Raydium (Launchpad, CPMM), Meteora (DBC, DAMM v2).

### Quote currencies

SOL and USD1.

## Install

### From source

```bash
cargo install --path .
```

### Debian / Ubuntu

Pre-built `.deb` packages are available from the LaserSell APT repository. See the [install docs](https://docs.lasersell.io) for setup instructions.

## Quick start

### 1. Run the setup wizard

```bash
lasersell --setup
```

The wizard walks you through:
- Connecting your Solana RPC endpoint
- Entering your LaserSell API key (free at [app.lasersell.io](https://app.lasersell.io/auth/sign-up))
- Setting your default strategy (take-profit, stop-loss, deadline, slippage)
- Creating or importing a wallet (seed phrase, Solana JSON keypair, or base58 secret key)

Config and keystore are saved to `~/.lasersell/`.

### 2. Start the daemon

```bash
lasersell
```

That's it. LaserSell connects to the Exit Intelligence stream, monitors your positions, and auto-sells when your strategy triggers.

### 3. Control it live

Inside the TUI, press `Tab` to switch to the command pane:

```
set tp 20%          # change take-profit to 20%
set sl 5%           # set stop-loss to 5%
set timeout 60      # force-sell after 60 seconds
sell                 # manually sell the selected position
pause / resume       # pause or resume new sessions
?                    # show all commands
```

Use `Up`/`Down` in the sessions pane to browse positions, `Enter` to expand details, and `Space` to pause/resume.

## Configuration

LaserSell reads from `~/.lasersell/config.yml` by default. Override with `-f /path/to/config.yml` or environment variables.

```yaml
account:
  keypair_path: "~/.lasersell/wallet.keystore.json"
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

See [`config.example.yml`](config.example.yml) for all options with comments.

### Environment variables

| Variable | Overrides |
|----------|-----------|
| `LASERSELL_CONFIG_PATH` | Config file path |
| `LASERSELL_KEYPAIR_PATH` | `account.keypair_path` |
| `LASERSELL_RPC_URL` | `account.rpc_url` |
| `LASERSELL_API_KEY` | `account.api_key` |
| `LASERSELL_SEND_TARGET` | `account.send_target` |
| `LASERSELL_WALLET_PASSPHRASE` | Keystore passphrase (headless unlock) |

## CLI flags

```
lasersell                          # TUI mode (default)
lasersell --setup                  # Interactive onboarding wizard
lasersell --no-tui                 # Headless mode (plain log output)
lasersell --verbose                # Same as --no-tui
lasersell --smoke                  # Health check: connect, verify, exit
lasersell --export-private-key     # Print base58 private key to stdout
lasersell -f /path/to/config.yml   # Use a specific config file
```

## Transaction submission

LaserSell supports three send targets, configured via `account.send_target` in your config or `LASERSELL_SEND_TARGET`:

| Target | Description |
|--------|-------------|
| `helius_sender` | Default. Routes through Helius for optimized landing. |
| `astralane` | Alternative sender (requires `astralane_api_key`). |
| `rpc` | Direct submission to your Solana RPC endpoint. |

## Security

- **Encrypted keystore** — wallet keys are encrypted at rest with Argon2id key derivation and XChaCha20-Poly1305 authenticated encryption.
- **Non-custodial** — private keys never leave your machine and are never transmitted to LaserSell servers.
- **Log redaction** — RPC URLs, API keys, and auth headers are automatically scrubbed from all log output.
- **Atomic writes** — config and keystore files are written atomically to prevent corruption.
- **Memory safety** — sensitive data (keypair bytes, passphrases) is zeroized after use.

## Building from source

```bash
# Production build
cargo build --release

# Devnet build (uses devnet endpoints + debug logging)
cargo build --features devnet
```

The release profile uses fat LTO, single codegen unit, and symbol stripping for a small, optimized binary.

## LaserSell ecosystem

| Resource | Link |
|----------|------|
| Website | [lasersell.io](https://lasersell.io) |
| Dashboard & API keys | [app.lasersell.io](https://app.lasersell.io) |
| Documentation | [docs.lasersell.io](https://docs.lasersell.io) |
| SDKs | [github.com/lasersell/lasersell-sdk](https://github.com/lasersell/lasersell-sdk) |
| Discord | [discord.gg/lasersell](https://discord.gg/lasersell) |
| X (Twitter) | [@lasersellhq](https://x.com/lasersellhq) |

## License

[MIT](LICENSE)

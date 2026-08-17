# StellarSend Backend

A production-quality Rust backend for **StellarSend** — a global money-transfer application built on the [Stellar](https://stellar.org) network.

---

## Table of Contents

1. [Tech Stack](#tech-stack)
2. [Architecture](#architecture)
3. [Getting Started](#getting-started)
4. [Environment Variables](#environment-variables)
5. [Database Setup](#database-setup)
6. [Stellar Integration Notes](#stellar-integration-notes)
7. [API Reference](#api-reference)
   - [Health Check](#health-check)
   - [Authentication](#authentication)
   - [Payments](#payments)
   - [Transactions](#transactions)
   - [Accounts](#accounts)
   - [Rates](#rates)
8. [Differentiator Features](#differentiator-features)
   - [Scheduled & recurring payments](#scheduled--recurring-payments)
   - [Split / batch payments](#split--batch-payments)
   - [Payment requests / invoicing](#payment-requests--invoicing)
   - [Escrow / conditional transfers](#escrow--conditional-transfers)
   - [Keeper design](#keeper-design)
9. [Error Handling](#error-handling)
10. [Deployment Guide](#deployment-guide)
11. [Docker](#docker)

---

## Tech Stack

| Concern | Crate |
|---|---|
| HTTP framework | `axum 0.7` |
| Async runtime | `tokio` |
| Database | `sqlx 0.7` + PostgreSQL |
| HTTP client | `reqwest` |
| Serialisation | `serde` / `serde_json` |
| Authentication | `jsonwebtoken` + `bcrypt` |
| Middleware | `tower-http` (CORS, tracing, request-id, timeout) |
| Structured logging | `tracing` + `tracing-subscriber` |
| Error handling | `thiserror` + `anyhow` |
| IDs | `uuid v4` |
| Timestamps | `chrono` |

---

## Architecture

```
src/
├── main.rs              # App startup: DB pool, router, bind
├── config.rs            # Config struct loaded from env
├── error.rs             # AppError enum → HTTP responses
├── db.rs                # Pool setup + migration runner
├── models/
│   ├── user.rs          # User, UserRow, JwtClaims, CreateUserRequest, LoginRequest
│   ├── transaction.rs   # Transaction, TransactionStatus, CreateTransaction, pagination
│   └── payment.rs       # Asset, QuoteRequest/Response, SendPaymentRequest, PaymentResult
├── routes/
│   ├── mod.rs           # Router assembly
│   ├── auth.rs          # POST /api/auth/register, /login
│   ├── payments.rs      # POST /api/payments/quote, /send  — GET /api/payments/:id
│   ├── transactions.rs  # GET /api/transactions (paginated), /transactions/:id
│   ├── accounts.rs      # GET /api/accounts/:address, /:address/balances
│   └── rates.rs         # GET /api/rates?from=&to=
├── services/
│   ├── stellar.rs       # Horizon API wrapper
│   ├── payment.rs       # Quote + send business logic
│   ├── transaction.rs   # DB CRUD for transactions
│   └── rate.rs          # Rate fetching with in-memory TTL cache
└── middleware/
    ├── auth.rs          # JWT extractor (AuthUser), issue_jwt, decode_jwt
    └── cors.rs          # CORS layer builder
migrations/
├── 001_initial.sql      # users + transactions tables
└── 002_add_indices.sql  # Performance indices
```

---

## Getting Started

### Prerequisites

- **Rust** >= 1.75 (`rustup update stable`)
- **PostgreSQL** >= 14
- **sqlx-cli** for running migrations outside of the app: `cargo install sqlx-cli --no-default-features --features rustls,postgres`

### 1. Clone and configure

```bash
git clone https://github.com/your-org/stellarsend
cd stellarsend/backend
cp .env.example .env
# Edit .env with your DATABASE_URL, JWT_SECRET, etc.
```

### 2. Create the database

```bash
createdb stellarsend
# Or with psql:
psql -c "CREATE DATABASE stellarsend;"
```

### 3. Run migrations

Migrations run automatically on startup, or manually via:

```bash
sqlx migrate run --database-url postgres://user:pass@localhost/stellarsend
```

### 4. Build and run

```bash
cargo run
# Development with auto-reload:
cargo install cargo-watch
cargo watch -x run
```

The server binds to `http://0.0.0.0:8080` by default.

---

## Environment Variables

Copy `.env.example` to `.env` and adjust all values before running.

| Variable | Default | Description |
|---|---|---|
| `PORT` | `8080` | HTTP listen port |
| `HOST` | `0.0.0.0` | HTTP bind address |
| `RUST_LOG` | `info` | Log level filter (`error`, `warn`, `info`, `debug`, `trace`) |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `DATABASE_MAX_CONNECTIONS` | `20` | PgPool max connections |
| `DATABASE_MIN_CONNECTIONS` | `2` | PgPool min connections |
| `DATABASE_CONNECT_TIMEOUT_SECS` | `30` | Connection acquisition timeout |
| `JWT_SECRET` | — | HMAC-SHA256 signing secret, **min 32 chars** (required) |
| `JWT_EXPIRY_HOURS` | `24` | Token lifetime in hours |
| `HORIZON_URL` | `https://horizon-testnet.stellar.org` | Stellar Horizon base URL |
| `STELLAR_NETWORK_PASSPHRASE` | testnet passphrase | Used for XDR verification |
| `RATE_CACHE_TTL_SECS` | `60` | Exchange-rate cache TTL |
| `ALLOWED_ORIGINS` | `*` | Comma-separated CORS origins, or `*` for all |
| `APP_ENV` | `development` | `development` or `production` |

---

## Database Setup

### Schema overview

**`users`**

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | `gen_random_uuid()` |
| `email` | TEXT UNIQUE | Lowercase-normalised on insert |
| `password_hash` | TEXT | bcrypt |
| `full_name` | TEXT | |
| `stellar_address` | TEXT nullable | Stellar G… public key |
| `is_active` | BOOLEAN | Default `true` |
| `created_at` | TIMESTAMPTZ | |
| `updated_at` | TIMESTAMPTZ | Auto-set by trigger |

**`transactions`**

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | |
| `user_id` | UUID FK → users | Cascade delete |
| `stellar_tx_hash` | TEXT nullable UNIQUE | Set after Horizon submission |
| `from_asset` | TEXT | e.g. `XLM` or `USDC:GA5Z…` |
| `to_asset` | TEXT | |
| `send_amount` | TEXT | |
| `receive_amount` | TEXT nullable | Filled after execution |
| `source_account` | TEXT | Stellar public key |
| `destination_account` | TEXT | |
| `status` | ENUM | pending → submitted → completed / failed |
| `error_message` | TEXT nullable | Horizon error detail on failure |
| `fee_xlm` | TEXT nullable | |
| `path` | JSONB nullable | Asset path hops |
| `created_at` | TIMESTAMPTZ | |
| `updated_at` | TIMESTAMPTZ | |

### Running migrations manually

Migrations use sqlx's **reversible format** — every migration is a paired `NNN_name.up.sql` / `NNN_name.down.sql` file, so a bad migration can be undone with the tooling instead of a hand-written corrective script. (Existing `001`–`011` migrations were retrofitted to this format with their SQL payload **byte-for-byte unchanged**; because sqlx checksums a reversible migration over its `.up.sql` file alone, the checksums already recorded in any live database are preserved and nothing needs reconciling.)

```bash
# Apply all pending migrations
sqlx migrate run --database-url "$DATABASE_URL"

# Roll back the last applied migration
sqlx migrate revert --database-url "$DATABASE_URL"

# Check migration status
sqlx migrate info --database-url "$DATABASE_URL"
```

To add a new migration, generate the pair (no-op scripts you then fill in):

```bash
sqlx migrate add -r <description>
# creates NNN_<description>.up.sql and NNN_<description>.down.sql in ./migrations
```

`revert` steps one migration back at a time; use `--target-version <N>` to roll back to a specific version (or `--target-version 0` to revert everything) — see `sqlx migrate revert --help`.

> **Migration 010 is not fully revertible.** Its `ALTER TYPE transaction_status ADD VALUE 'submitted_unconfirmed'` cannot be undone — Postgres has no `ALTER TYPE … DROP VALUE`. Its `.down.sql` reverts the reversible parts and carries a full operator runbook for undoing the enum value (a create-new-type / migrate-columns / swap procedure); see the header of `migrations/010_batch_reconciliation.down.sql`.

> **Smoke test.** `scripts/migrate-revert-smoke.sh` applies every migration to a fresh database, reverts `011 → 010 → 009`, asserts the schema matches the pre-migration state at each step, re-applies, and tears down. It runs in CI on every PR (`.github/workflows/migrations.yml`) and locally via:
>
> ```bash
> BASE_URL=postgres://user:pass@localhost:5432 bash scripts/migrate-revert-smoke.sh
> ```

---

## Stellar Integration Notes

### Testnet vs Mainnet

Switch between networks by changing two env vars:

```env
# Testnet (default)
HORIZON_URL=https://horizon-testnet.stellar.org
STELLAR_NETWORK_PASSPHRASE=Test SDF Network ; September 2015

# Mainnet
HORIZON_URL=https://horizon.stellar.org
STELLAR_NETWORK_PASSPHRASE=Public Global Stellar Network ; September 2015
```

### How payments work

1. **Quote** — the client calls `POST /api/payments/quote` with the send/receive assets and amount.  The backend queries Horizon's `/paths/strict-send` endpoint to find the best DEX path and returns an estimated receive amount, implied rate, fee, and path array.

2. **Client-side signing** — the client uses any Stellar SDK (JavaScript, Flutter, etc.) to build a `PathPaymentStrictSend` operation, sign it with the user's secret key, and base64-encode the `TransactionEnvelope` XDR.

3. **Submit** — the client posts the signed XDR to `POST /api/payments/send`.  The backend relays it to Horizon and records the outcome (hash, ledger, fee) in the database.

### Asset format

Assets are passed as JSON objects:

```json
{ "code": "XLM",  "issuer": null }
{ "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" }
```

For query parameters (e.g. `/api/rates`), pass `XLM` or `CODE:ISSUER`.

### Exchange Rates

The rate service queries Horizon's path-payment endpoint with a send amount of `1` and reads the best destination amount.  Results are cached in-process with a configurable TTL.  For production workloads consider replacing the fallback with a dedicated price oracle (CoinGecko, Stellar Expert, etc.).

---

## API Reference

All responses share this envelope:

```json
{ "success": true,  "data": { … } }
{ "success": false, "error": { "code": "ERROR_CODE", "message": "Human-readable detail" } }
```

---

### Health Check

#### `GET /health`

Lightweight liveness probe.  No authentication required.

```bash
curl http://localhost:8080/health
```

```json
{
  "success": true,
  "data": { "status": "ok", "service": "stellarsend-backend" }
}
```

---

### Authentication

#### `POST /api/auth/register`

Create a new user account and receive a JWT.

**Request**

```bash
curl -X POST http://localhost:8080/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{
    "email": "alice@example.com",
    "password": "supersecret123",
    "full_name": "Alice Doe",
    "stellar_address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN"
  }'
```

**Response `201`**

```json
{
  "success": true,
  "data": {
    "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9…",
    "user": {
      "id": "01920c1a-0000-7000-8000-000000000001",
      "email": "alice@example.com",
      "full_name": "Alice Doe",
      "stellar_address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
      "is_active": true,
      "created_at": "2024-01-15T10:00:00Z"
    }
  }
}
```

**Error codes**

| Code | Meaning |
|---|---|
| `VALIDATION_ERROR` | Missing field or invalid format |
| `CONFLICT` | Email already registered |

---

#### `POST /api/auth/login`

Authenticate with email + password and receive a JWT.

**Request**

```bash
curl -X POST http://localhost:8080/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{
    "email": "alice@example.com",
    "password": "supersecret123"
  }'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9…",
    "user": {
      "id": "01920c1a-0000-7000-8000-000000000001",
      "email": "alice@example.com",
      "full_name": "Alice Doe",
      "stellar_address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
      "is_active": true,
      "created_at": "2024-01-15T10:00:00Z"
    }
  }
}
```

**Error codes**

| Code | Meaning |
|---|---|
| `INVALID_CREDENTIALS` | Wrong email or password |

---

### Payments

All payment endpoints require `Authorization: Bearer <token>`.

#### `POST /api/payments/quote`

Get the best path-payment quote from the Stellar DEX without committing anything.

**Request**

```bash
curl -X POST http://localhost:8080/api/payments/quote \
  -H 'Authorization: Bearer <token>' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
    "to_asset":   { "code": "XLM",  "issuer": null },
    "amount": "100",
    "destination": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
  }'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "send_amount": "100",
    "estimated_receive": "909.0909090",
    "rate": "9.0909090",
    "fee_xlm": "0.0000100",
    "from_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
    "to_asset":   { "code": "XLM",  "issuer": null },
    "path": []
  }
}
```

**Error codes**

| Code | Meaning |
|---|---|
| `NO_PATH_FOUND` | No DEX path exists between the two assets |
| `BAD_REQUEST` | Invalid amount |
| `HORIZON_ERROR` | Horizon returned an error |

---

#### `POST /api/payments/send`

Submit a **client-signed** Stellar transaction XDR.  The client is responsible for building and signing the transaction using any Stellar SDK before calling this endpoint.

**Workflow**

1. Call `/api/payments/quote` to get the path.
2. Build a `PathPaymentStrictSend` transaction in your Stellar SDK.
3. Sign with the user's Stellar secret key.
4. Base64-encode the `TransactionEnvelope` XDR.
5. POST to this endpoint.

**Request**

```bash
curl -X POST http://localhost:8080/api/payments/send \
  -H 'Authorization: Bearer <token>' \
  -H 'Content-Type: application/json' \
  -d '{
    "source_account": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
    "signed_xdr": "AAAAAgAAAABiXst2pnqBtsr…AAAAAAAAAAA=",
    "from_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
    "to_asset":   { "code": "XLM",  "issuer": null },
    "send_amount": "100",
    "destination_account": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
  }'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "tx_hash": "a1b2c3d4e5f6…",
    "success": true,
    "ledger": 48293011,
    "fee_charged": "100",
    "transaction_id": "01920c1a-0000-7000-8000-000000000042"
  }
}
```

**Error codes**

| Code | Meaning |
|---|---|
| `TRANSACTION_FAILED` | Horizon rejected the XDR |
| `BAD_REQUEST` | Empty or malformed XDR |
| `HORIZON_ERROR` | Network-level Horizon error |

---

#### `GET /api/payments/:id`

Retrieve an internal payment record plus live Stellar data (when a hash is available).

```bash
curl http://localhost:8080/api/payments/01920c1a-0000-7000-8000-000000000042 \
  -H 'Authorization: Bearer <token>'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "payment": {
      "id": "01920c1a-0000-7000-8000-000000000042",
      "user_id": "01920c1a-0000-7000-8000-000000000001",
      "stellar_tx_hash": "a1b2c3d4e5f6…",
      "from_asset": "USDC:GA5Z…",
      "to_asset": "XLM:native",
      "send_amount": "100",
      "receive_amount": null,
      "source_account": "GAAZI4…",
      "destination_account": "GBBD47…",
      "status": "completed",
      "fee_xlm": "0.0000100",
      "path": [],
      "created_at": "2024-01-15T10:05:00Z",
      "updated_at": "2024-01-15T10:05:03Z"
    },
    "stellar": {
      "id": "a1b2c3d4e5f6…",
      "hash": "a1b2c3d4e5f6…",
      "ledger": 48293011,
      "created_at": "2024-01-15T10:05:01Z",
      "source_account": "GAAZI4…",
      "fee_charged": "100",
      "successful": true
    }
  }
}
```

---

### Transactions

All transaction endpoints require `Authorization: Bearer <token>`.

#### `GET /api/transactions`

Return a paginated list of the authenticated user's transactions with optional filters.

**Query parameters**

| Parameter | Type | Description |
|---|---|---|
| `status` | string | Filter: `pending`, `submitted`, `completed`, `failed`, `expired` |
| `from_asset` | string | Filter by source asset |
| `to_asset` | string | Filter by destination asset |
| `page` | integer | Page number (default: 1) |
| `per_page` | integer | Results per page (default: 20, max: 100) |

```bash
# All transactions, newest first
curl 'http://localhost:8080/api/transactions' \
  -H 'Authorization: Bearer <token>'

# Only completed, page 2
curl 'http://localhost:8080/api/transactions?status=completed&page=2&per_page=10' \
  -H 'Authorization: Bearer <token>'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "items": [ { "id": "…", "status": "completed", "…": "…" } ],
    "total": 42,
    "page": 1,
    "per_page": 20,
    "total_pages": 3
  }
}
```

---

#### `GET /api/transactions/:id`

Retrieve a single transaction by its internal UUID.

```bash
curl http://localhost:8080/api/transactions/01920c1a-0000-7000-8000-000000000042 \
  -H 'Authorization: Bearer <token>'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "id": "01920c1a-0000-7000-8000-000000000042",
    "user_id": "01920c1a-0000-7000-8000-000000000001",
    "stellar_tx_hash": "a1b2c3…",
    "from_asset": "XLM",
    "to_asset": "USDC:GA5Z…",
    "send_amount": "50",
    "receive_amount": "5.5",
    "source_account": "GAAZI4…",
    "destination_account": "GBBD47…",
    "status": "completed",
    "error_message": null,
    "fee_xlm": "0.0000100",
    "path": [],
    "created_at": "2024-01-15T10:00:00Z",
    "updated_at": "2024-01-15T10:00:03Z"
  }
}
```

**Error codes**

| Code | Meaning |
|---|---|
| `NOT_FOUND` | No transaction with that id |
| `FORBIDDEN` | Transaction belongs to a different user |

---

### Accounts

All account endpoints require `Authorization: Bearer <token>`.

#### `GET /api/accounts/:address`

Fetch the full Horizon account record for a Stellar public key.

```bash
curl http://localhost:8080/api/accounts/GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN \
  -H 'Authorization: Bearer <token>'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "id": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
    "account_id": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
    "sequence": "175420452143",
    "subentry_count": 2,
    "balances": [
      { "balance": "9999.9999900", "asset_type": "native", "asset_code": "", "asset_issuer": "" },
      { "balance": "100.0000000", "asset_type": "credit_alphanum4", "asset_code": "USDC", "asset_issuer": "GA5Z…", "limit": "922337203685.4775807" }
    ],
    "thresholds": { "low_threshold": 0, "med_threshold": 0, "high_threshold": 0 },
    "flags": { "auth_required": false, "auth_revocable": false, "auth_immutable": false },
    "last_modified_ledger": 48291000,
    "last_modified_time": "2024-01-15T09:50:00Z"
  }
}
```

---

#### `GET /api/accounts/:address/balances`

Return only the balance array — useful for quick wallet display.

```bash
curl http://localhost:8080/api/accounts/GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN/balances \
  -H 'Authorization: Bearer <token>'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
    "balances": [
      { "balance": "9999.9999900", "asset_type": "native", "asset_code": "", "asset_issuer": "" },
      { "balance": "100.0000000", "asset_type": "credit_alphanum4", "asset_code": "USDC", "asset_issuer": "GA5Z…", "limit": "…" }
    ]
  }
}
```

**Error codes**

| Code | Meaning |
|---|---|
| `BAD_REQUEST` | Address is not a valid 56-char G… key |
| `NOT_FOUND` | Account does not exist on Stellar |

---

### Rates

No authentication required.

#### `GET /api/rates?from=<asset>&to=<asset>`

Returns the current exchange rate between two assets, sourced from the Stellar DEX and cached server-side.

**Asset format for query params**

- Native XLM: `XLM`
- Issued asset: `CODE:ISSUER_PUBLIC_KEY`

```bash
# XLM to USDC
curl 'http://localhost:8080/api/rates?from=XLM&to=USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN'

# USDC to XLM
curl 'http://localhost:8080/api/rates?from=USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN&to=XLM'
```

**Response `200`**

```json
{
  "success": true,
  "data": {
    "from": "XLM",
    "to": "USDC:GA5Z…",
    "rate": 0.11,
    "timestamp": "2024-01-15T10:00:00Z"
  }
}
```

---

## Differentiator Features

These sit on top of the on-chain building blocks in [`StellarSend/contracts`](https://github.com/StellarSend/contracts) — this layer handles persistence, history, and (where a scheduled action is needed) submitting already-authorized on-chain calls. Every fund-moving path still follows the non-custodial pattern above: the client signs with its own wallet, and this backend either relays the signed XDR or, for subscriptions only, a keeper service account submits a call the payer pre-authorized on-chain.

### Scheduled & recurring payments

`migrations/006_add_subscriptions.sql` — mirrors `stellar_send::subscription`.

| Method | Path | Notes |
|---|---|---|
| `POST` | `/api/subscriptions` | Create a subscription record |
| `GET` | `/api/subscriptions` | List the caller's subscriptions |
| `GET` | `/api/subscriptions/:id` | Get one |
| `POST` | `/api/subscriptions/:id/cancel` | Cancel |

A background `tokio` task (toggle with `KEEPER_ENABLED`) — plus a manual `POST /api/keeper/run-subscriptions` trigger — finds subscriptions due for execution and submits `execute_subscription` via the keeper. A subscription is marked `failed` after 3 consecutive on-chain failures rather than retried forever.

### Split / batch payments

`migrations/009_add_batch_payments.sql` adds `batch_id`/`batch_index` to `transactions`.

| Method | Path | Notes |
|---|---|---|
| `POST` | `/api/payments/batch` | Relay one client-signed multi-recipient transaction; records one `transactions` row per leg |

### Payment requests / invoicing

`migrations/007_add_payment_requests.sql`.

| Method | Path | Notes |
|---|---|---|
| `POST` | `/api/payment-requests` | Create a request, returns a shareable id |
| `GET` | `/api/payment-requests` | List the caller's requests |
| `GET` | `/api/payment-requests/:id` | Public lookup (for the payer's UI) |
| `POST` | `/api/payment-requests/:id/fulfill` | Fulfill (client-signed, same as a regular send) |
| `POST` | `/api/payment-requests/:id/cancel` | Cancel |

### Escrow / conditional transfers

`migrations/008_add_escrows.sql`.

| Method | Path | Notes |
|---|---|---|
| `POST` | `/api/escrows` | Record an escrow the client has funded via its own signed `create_escrow` call |
| `GET` | `/api/escrows` | List escrows the caller is party to |
| `GET` | `/api/escrows/:id` | Get one (open to depositor/beneficiary/arbiter — they may not all be StellarSend accounts) |
| `POST` | `/api/escrows/:id/release/build` | Build an unsigned `release_escrow` invocation sourced from the caller's own account |
| `POST` | `/api/escrows/:id/release` | Relay the caller's signed release envelope |
| `POST` | `/api/escrows/:id/refund/build` | Build an unsigned `refund_escrow` invocation |
| `POST` | `/api/escrows/:id/refund` | Relay the caller's signed refund envelope |

**Why release/refund are client-signed, not keeper-executed:** the contract's `release_escrow`/`refund_escrow` require `caller.require_auth()` — only the actual beneficiary, depositor, or arbiter can authorize the call. A keeper service account can't sign on their behalf, so unlike subscriptions (which use a pre-granted token allowance) there's no sound way to automate this for a party the keeper isn't itself. The `/build` step returns an unsigned invocation for whichever party is acting to sign client-side, exactly like a regular payment.

### Keeper design

`src/services/soroban.rs` implements a genuine Soroban RPC client: it builds, simulates, signs (with the keeper's own ed25519 key, configured via `KEEPER_SECRET_KEY` — never a user's), and submits `invoke_host_function` calls. The keeper only ever pays its own network fee; it can't move user funds beyond what the contract itself, pre-authorized by the user on-chain, allows. Due subscriptions are selected with `FOR UPDATE SKIP LOCKED` so it's safe to run more than one backend instance.

> **Known gap:** the contracts, backend, and frontend for these four features were built in parallel against a shared spec rather than a single locked API contract. The escrow release/refund auth model above has been reconciled and fixed, but endpoint payload casing and a few paths (e.g. exact batch/subscription URLs) may still need a pass to line up frontend and backend exactly — check `src/lib/api.ts` in the frontend repo against `src/routes/mod.rs` here before wiring up a new client.

---

## Error Handling

Every error response follows the same envelope:

```json
{
  "success": false,
  "error": {
    "code": "MACHINE_READABLE_CODE",
    "message": "Human-readable description"
  }
}
```

| HTTP | Code | Trigger |
|---|---|---|
| 400 | `BAD_REQUEST` | Malformed request body / params |
| 401 | `UNAUTHORIZED` | Missing or invalid `Authorization` header |
| 401 | `INVALID_CREDENTIALS` | Wrong email or password |
| 401 | `TOKEN_EXPIRED` | JWT past its `exp` claim |
| 401 | `INVALID_TOKEN` | JWT signature invalid or malformed |
| 403 | `FORBIDDEN` | Authenticated but not allowed |
| 404 | `NOT_FOUND` | Resource not found |
| 409 | `CONFLICT` | Resource already exists |
| 422 | `VALIDATION_ERROR` | Field-level validation failure |
| 422 | `NO_PATH_FOUND` | No DEX path for the requested asset pair |
| 422 | `TRANSACTION_FAILED` | Stellar transaction rejected by network |
| 502 | `HORIZON_ERROR` | Horizon API returned an error |
| 500 | `INTERNAL_ERROR` | Unexpected server error (detail logged, not exposed) |

---

## Deployment Guide

### Production checklist

- [ ] Set `APP_ENV=production`
- [ ] Use a strong, random `JWT_SECRET` (at least 64 random bytes, base64-encoded)
- [ ] Point `HORIZON_URL` at `https://horizon.stellar.org`
- [ ] Update `STELLAR_NETWORK_PASSPHRASE` to the mainnet passphrase
- [ ] Set `ALLOWED_ORIGINS` to your frontend domain(s) — never `*` in production
- [ ] Use a managed PostgreSQL instance (AWS RDS, Supabase, etc.) and set `DATABASE_URL`
- [ ] Run behind a TLS-terminating reverse proxy (nginx, Caddy, AWS ALB)
- [ ] Set `RUST_LOG=warn` or `RUST_LOG=info` to reduce log volume

### Building a release binary

```bash
cargo build --release
./target/release/stellarsend
```

### Fly.io

```bash
fly launch --name stellarsend-api
fly secrets set DATABASE_URL="postgres://…"
fly secrets set JWT_SECRET="$(openssl rand -hex 64)"
fly secrets set HORIZON_URL="https://horizon.stellar.org"
fly deploy
```

### Railway

Push to GitHub and link the repo in Railway.  Set the same env vars via the Railway dashboard.  The `Cargo.toml` is auto-detected.

---

## Docker

### Dockerfile

```dockerfile
# ─── Build stage ──────────────────────────────────────────────────────────────
FROM rust:1.75-slim-bookworm AS builder

WORKDIR /app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations

RUN cargo build --release

# ─── Runtime stage ────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/stellarsend /app/stellarsend
COPY migrations ./migrations

EXPOSE 8080

CMD ["/app/stellarsend"]
```

### docker-compose.yml (development)

```yaml
version: "3.9"
services:
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: stellarsend
      POSTGRES_PASSWORD: password
      POSTGRES_DB: stellarsend
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data

  api:
    build: .
    ports:
      - "8080:8080"
    environment:
      DATABASE_URL: postgres://stellarsend:password@db:5432/stellarsend
      JWT_SECRET: dev-secret-change-in-production-must-be-32-chars-minimum
      HORIZON_URL: https://horizon-testnet.stellar.org
      STELLAR_NETWORK_PASSPHRASE: "Test SDF Network ; September 2015"
      RUST_LOG: debug
      APP_ENV: development
    depends_on:
      - db

volumes:
  pgdata:
```

```bash
# Start everything
docker-compose up -d

# Tail logs
docker-compose logs -f api

# Stop
docker-compose down
```

---

## License

MIT

# StellarSend Backend API

Base URL: `https://api.stellarsend.io` (or `http://localhost:8080` in local development)

## Authentication
All protected endpoints require `Authorization: Bearer <jwt>`.
Public endpoints: `/health`, `/api/auth/register`, `/api/auth/login`, `/api/rates`, `/api/payment-requests/:id`.

---

## Response Envelope

All API endpoints return JSON conforming to standard success and error response envelopes:

### Success Response
```json
{
  "success": true,
  "data": { ... }
}
```

### Error Response
```json
{
  "success": false,
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable description"
  }
}
```

---

## Endpoints Summary

| Method | Path | Auth | Description |
|---|---|---|---|
| `GET` | `/health` | No | Health check and background loop status |
| `POST` | `/api/auth/register` | No | Register new user account |
| `POST` | `/api/auth/login` | No | Authenticate user and receive JWT |
| `POST` | `/api/payments/quote` | Yes | Get path payment DEX quote |
| `POST` | `/api/payments/send` | Yes | Relay signed Stellar payment transaction |
| `POST` | `/api/payments/batch` | Yes | Relay signed batch payment transaction |
| `GET` | `/api/payments/:id` | Yes | Get internal payment record and Stellar details |
| `GET` | `/api/transactions` | Yes | List authenticated user transactions (paginated) |
| `GET` | `/api/transactions/:id` | Yes | Get single transaction by UUID |
| `GET` | `/api/accounts/:address` | Yes | Fetch Stellar account info from Horizon |
| `GET` | `/api/accounts/:address/balances` | Yes | Fetch Stellar account balances |
| `GET` | `/api/rates` | No | Get exchange rate (`?from=&to=`) |
| `GET` | `/api/subscriptions` | Yes | List authenticated user subscriptions |
| `POST` | `/api/subscriptions` | Yes | Create recurring payment subscription |
| `GET` | `/api/subscriptions/:id` | Yes | Get subscription by UUID |
| `POST` | `/api/subscriptions/:id/cancel` | Yes | Cancel recurring subscription |
| `GET` | `/api/payment-requests` | Yes | List user's created payment requests |
| `POST` | `/api/payment-requests` | Yes | Create payment request / invoice |
| `GET` | `/api/payment-requests/:id` | No | Public lookup for payment request |
| `POST` | `/api/payment-requests/:id/fulfill` | Yes | Fulfill payment request with signed XDR |
| `POST` | `/api/payment-requests/:id/cancel` | Yes | Cancel payment request |
| `GET` | `/api/escrows` | Yes | List escrows party to authenticated user |
| `POST` | `/api/escrows` | Yes | Record funded escrow |
| `GET` | `/api/escrows/:id` | Yes | Get escrow details |
| `POST` | `/api/escrows/:id/release/build` | Yes | Build unsigned invocation for escrow release |
| `POST` | `/api/escrows/:id/release` | Yes | Relay signed escrow release transaction |
| `POST` | `/api/escrows/:id/refund/build` | Yes | Build unsigned invocation for escrow refund |
| `POST` | `/api/escrows/:id/refund` | Yes | Relay signed escrow refund transaction |
| `POST` | `/api/keeper/run-subscriptions` | Yes | Manually trigger keeper subscription execution |

---

## Detailed Endpoint Reference

### Health

#### `GET /health`
Liveness probe returning service status and background loop timestamps.

**Response `200 OK`**
```json
{
  "success": true,
  "data": {
    "status": "ok",
    "service": "stellarsend-backend",
    "background_loops": {
      "keeper_last_tick_at": 1700000000,
      "reconciliation_last_tick_at": 1700000000
    }
  }
}
```

---

### Authentication

#### `POST /api/auth/register`
Register a new user account.

**Request Body**
```json
{
  "email": "user@example.com",
  "password": "securepassword123",
  "full_name": "Jane Doe",
  "stellar_address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN"
}
```

**Response `201 Created`**
```json
{
  "success": true,
  "data": {
    "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
    "user": {
      "id": "01920c1a-0000-7000-8000-000000000001",
      "email": "user@example.com",
      "full_name": "Jane Doe",
      "stellar_address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
      "is_active": true,
      "created_at": "2024-01-15T10:00:00Z"
    }
  }
}
```

#### `POST /api/auth/login`
Authenticate user with email and password.

**Request Body**
```json
{
  "email": "user@example.com",
  "password": "securepassword123"
}
```

**Response `200 OK`**
```json
{
  "success": true,
  "data": {
    "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
    "user": {
      "id": "01920c1a-0000-7000-8000-000000000001",
      "email": "user@example.com",
      "full_name": "Jane Doe",
      "stellar_address": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
      "is_active": true,
      "created_at": "2024-01-15T10:00:00Z"
    }
  }
}
```

---

### Payments

#### `POST /api/payments/quote`
Get quote for DEX path payment.

**Request Body**
```json
{
  "from_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
  "to_asset": { "code": "XLM", "issuer": null },
  "amount": "100",
  "destination": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
}
```

#### `POST /api/payments/send`
Relay client-signed XDR transaction to Horizon.

**Request Body**
```json
{
  "source_account": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
  "signed_xdr": "AAAAAgAAAABiXst2pnqBtsr...",
  "from_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
  "to_asset": { "code": "XLM", "issuer": null },
  "send_amount": "100",
  "destination_account": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
}
```

#### `POST /api/payments/batch`
Relay client-signed multi-recipient batch transaction.

**Request Body**
```json
{
  "source_account": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
  "signed_xdr": "AAAAAgAAAABiXst2pnqBtsr...",
  "from_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
  "legs": [
    {
      "destination_account": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
      "send_amount": "50",
      "to_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" }
    },
    {
      "destination_account": "GCXYZ7IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLB8",
      "send_amount": "50",
      "to_asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" }
    }
  ]
}
```

#### `GET /api/payments/:id`
Fetch internal payment record and on-chain status by UUID.

---

### Transactions

#### `GET /api/transactions`
List authenticated user transactions. Supports query parameters `status`, `from_asset`, `to_asset`, `page`, `per_page`, `cursor`.

#### `GET /api/transactions/:id`
Retrieve specific transaction by UUID.

---

### Accounts

#### `GET /api/accounts/:address`
Fetch full Horizon account object for Stellar public key.

#### `GET /api/accounts/:address/balances`
Fetch asset balances array for Stellar public key.

---

### Rates

#### `GET /api/rates?from=<asset>&to=<asset>`
Query exchange rate between two assets (e.g. `?from=XLM&to=USDC:GA5Z...`).

---

### Subscriptions

#### `GET /api/subscriptions`
List recurring payment subscriptions for user.

#### `POST /api/subscriptions`
Create a recurring subscription.

**Request Body**
```json
{
  "subscriber_account": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
  "recipient_account": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
  "asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
  "amount": "25.00",
  "interval_seconds": 2592000,
  "start_time": "2024-02-01T00:00:00Z"
}
```

#### `GET /api/subscriptions/:id`
Get subscription record by UUID.

#### `POST /api/subscriptions/:id/cancel`
Cancel an active subscription.

---

### Payment Requests

#### `GET /api/payment-requests`
List payment requests created by authenticated user.

#### `POST /api/payment-requests`
Create payment request invoice.

**Request Body**
```json
{
  "recipient_account": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
  "asset": { "code": "USDC", "issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN" },
  "amount": "100.00",
  "memo": "Invoice #1042",
  "expires_at": "2024-03-01T00:00:00Z"
}
```

#### `GET /api/payment-requests/:id`
Public endpoint to retrieve payment request details for payers.

#### `POST /api/payment-requests/:id/fulfill`
Fulfill payment request by submitting signed transaction XDR.

#### `POST /api/payment-requests/:id/cancel`
Cancel open payment request.

---

### Escrow

#### `GET /api/escrows`
List escrows where user is depositor, beneficiary, or arbiter.

#### `POST /api/escrows`
Record an on-chain escrow.

**Request Body**
```json
{
  "on_chain_id": 42,
  "depositor_account": "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN",
  "beneficiary_account": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
  "arbiter_account": "GCXYZ7IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLB8",
  "token_address": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
  "amount": "500",
  "unlock_time": 1715000000
}
```

#### `GET /api/escrows/:id`
Get escrow details.

#### `POST /api/escrows/:id/release/build`
Build unsigned invocation XDR for releasing escrow funds.

#### `POST /api/escrows/:id/release`
Relay signed release transaction XDR.

#### `POST /api/escrows/:id/refund/build`
Build unsigned invocation XDR for refunding escrow funds.

#### `POST /api/escrows/:id/refund`
Relay signed refund transaction XDR.

---

### Keeper

#### `POST /api/keeper/run-subscriptions`
Trigger immediate execution cycle for due recurring payments.

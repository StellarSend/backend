# StellarSend Backend API

Base URL: `https://api.stellarsend.io/v1`

## Authentication
All protected endpoints require `Authorization: Bearer <jwt>`.

## Endpoints

### Accounts
- `GET /accounts/:address` — Fetch Stellar account info

### Payments
- `POST /send` — Submit a payment
- `POST /payments/preview` — Preview fee breakdown
- `GET /transactions/:address` — List payment history

### Contacts
- `GET /contacts` — List saved contacts
- `POST /contacts` — Add a contact
- `DELETE /contacts/:id` — Remove a contact

### Rates
- `GET /rates/xlm` — Current XLM/USD rate

### Health
- `GET /health` — Liveness probe
- `GET /health/deep` — Readiness probe (DB + Horizon)

// Structured error codes for the StellarSend API
// Format: SS-{domain}-{code}
// SS-AUTH-001: Invalid token
// SS-AUTH-002: Token expired
// SS-PAY-001:  Invalid amount
// SS-PAY-002:  Invalid recipient address
// SS-PAY-003:  Stellar submission failed
// SS-HZN-001:  Horizon unreachable
// Added by Segun Bankole (#150)
pub const ERROR_CODE_VERSION: &str = "1.0";

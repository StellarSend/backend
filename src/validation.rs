use crate::error::{AppError, AppResult};

/// Validates that a string is a canonical Stellar Ed25519 public key (`G...` address).
///
/// Checks:
/// 1. Address is non-empty and starts with 'G' (public key) or 'M' (multiplexed).
/// 2. Address matches Stellar strkey encoding (valid Base32 and checksum).
///
/// Uses `stellar_strkey::ed25519::PublicKey` for standard `G...` keys and
/// `stellar_strkey::ed25519::Med25519PublicKey` for `M...` multiplexed keys.
pub fn validate_stellar_address(address: &str) -> AppResult<()> {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "Stellar address cannot be empty".into(),
        ));
    }

    if trimmed.starts_with('G') {
        stellar_strkey::ed25519::PublicKey::from_string(trimmed).map_err(|_| {
            AppError::Validation(
                "Invalid Stellar address: must be a valid 56-character base32 Ed25519 public key".into(),
            )
        })?;
        Ok(())
    } else if trimmed.starts_with('M') {
        stellar_strkey::ed25519::Med25519PublicKey::from_string(trimmed).map_err(|_| {
            AppError::Validation(
                "Invalid Stellar multiplexed address: invalid checksum or format".into(),
            )
        })?;
        Ok(())
    } else {
        Err(AppError::Validation(
            "Invalid Stellar address: must start with 'G' (or 'M' for multiplexed accounts)".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_stellar_address() {
        // Canonical valid Stellar public key
        assert!(validate_stellar_address("GBZXN7PIRZGNMHGA7MUUUF4GWPY5AYPV6LY4UV2GL6VJGIQRXFDNMADI").is_ok());
        assert!(validate_stellar_address("GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN").is_ok());
    }

    #[test]
    fn test_invalid_stellar_address() {
        // Wrong length
        assert!(validate_stellar_address("GBZXN").is_err());
        // Wrong prefix
        assert!(validate_stellar_address("SBZXN7PIRZGNMHGA7MUUUF4GWPY5AYPV6LY4UV2GL6VJGIQRXFDNMADI").is_err());
        // Invalid checksum
        assert!(validate_stellar_address("GBZXN7PIRZGNMHGA7MUUUF4GWPY5AYPV6LY4UV2GL6VJGIQRXFDNMAD0").is_err());
        // Empty
        assert!(validate_stellar_address("").is_err());
    }
}

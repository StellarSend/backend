use crate::error::{AppError, AppResult};
use sha2::{Digest, Sha256};
use stellar_xdr::curr::{
    Hash, Limits, ReadXdr, TransactionEnvelope, TransactionSignaturePayload,
    TransactionSignaturePayloadTaggedTransaction, WriteXdr,
};

/// Computes the canonical Stellar transaction hash for an already-signed
/// XDR envelope, entirely locally — no Horizon round-trip needed. This is
/// the same hash Horizon reports back after submission, so it can be
/// persisted *before* ever calling Horizon and used later by
/// `ReconciliationService` to look up the true on-chain outcome of a batch
/// whose submission response was lost to a crash or a client-side timeout
/// (#30).
///
/// Per the Stellar protocol the hash covers `network_id || tagged_transaction`
/// only — signatures are excluded — so this never needs (and never sees)
/// any private key; it works purely from the already-signed envelope the
/// client posted to us.
///
/// Supports the two envelope shapes any current Stellar SDK produces:
/// standard (`Tx`) and fee-bump (`TxFeeBump`). The legacy pre-Protocol-13
/// `TxV0` shape is rejected — no SDK still emits it, and mapping it to the
/// current `Transaction` shape just to hash it would be speculative,
/// untested complexity for a format nothing in this system ever sends us.
pub fn compute_transaction_hash(signed_xdr: &str, network_passphrase: &str) -> AppResult<String> {
    let envelope = TransactionEnvelope::from_xdr_base64(signed_xdr, Limits::none())
        .map_err(|e| AppError::BadRequest(format!("Invalid signed_xdr: {e}")))?;

    let tagged_transaction = match envelope {
        TransactionEnvelope::Tx(v1) => TransactionSignaturePayloadTaggedTransaction::Tx(v1.tx),
        TransactionEnvelope::TxFeeBump(fb) => {
            TransactionSignaturePayloadTaggedTransaction::TxFeeBump(fb.tx)
        }
        TransactionEnvelope::TxV0(_) => {
            return Err(AppError::BadRequest(
                "signed_xdr uses the legacy TxV0 envelope format, which is not supported".into(),
            ));
        }
    };

    let network_id = Hash(Sha256::digest(network_passphrase.as_bytes()).into());
    let payload = TransactionSignaturePayload {
        network_id,
        tagged_transaction,
    };

    let payload_bytes = payload
        .to_xdr(Limits::none())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Failed to encode tx payload: {e}")))?;

    let hash: [u8; 32] = Sha256::digest(&payload_bytes).into();
    Ok(hex::encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        Memo, MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions, Transaction,
        TransactionExt, TransactionV1Envelope, Uint256, VecM,
    };

    const NETWORK: &str = "Test SDF Network ; September 2015";
    const SOURCE: &str = "GBZXN7PIRZGNMHGA7MUUUF4GWPY5AYPV6LY4UV2GL6VJGIQRXFDNMADI";

    /// Builds a syntactically valid, unsigned `Transaction` with a single
    /// native payment operation — enough to exercise hashing without
    /// needing a real signing key. The destination reuses the same test
    /// keypair as the source; it's dummy data purely to exercise XDR
    /// encoding, not a real account.
    fn sample_transaction() -> Transaction {
        let source_id = stellar_strkey::ed25519::PublicKey::from_string(SOURCE)
            .expect("valid test source account");
        let source_account = MuxedAccount::Ed25519(Uint256(source_id.0));

        Transaction {
            source_account,
            fee: 100,
            seq_num: stellar_xdr::curr::SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256(source_id.0)),
                    asset: stellar_xdr::curr::Asset::Native,
                    amount: 100_000_000,
                }),
            }]
            .try_into()
            .expect("single operation fits VecM"),
            ext: TransactionExt::V0,
        }
    }

    fn envelope_base64(tx: Transaction) -> String {
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            // The hash excludes signatures entirely, so an empty signature
            // list is fine for this test.
            signatures: VecM::default(),
        });
        envelope
            .to_xdr_base64(Limits::none())
            .expect("encoding should succeed")
    }

    /// Independently recomputes the hash straight from the `Transaction` we
    /// built (bypassing envelope parsing) so the test isn't just comparing
    /// the function against itself.
    fn expected_hash(tx: Transaction, passphrase: &str) -> String {
        let network_id = Hash(Sha256::digest(passphrase.as_bytes()).into());
        let payload = TransactionSignaturePayload {
            network_id,
            tagged_transaction: TransactionSignaturePayloadTaggedTransaction::Tx(tx),
        };
        let bytes = payload.to_xdr(Limits::none()).unwrap();
        hex::encode(Sha256::digest(&bytes))
    }

    #[test]
    fn computes_the_same_hash_as_an_independent_calculation() {
        let tx = sample_transaction();
        let b64 = envelope_base64(tx.clone());

        let got = compute_transaction_hash(&b64, NETWORK).expect("should hash successfully");
        let want = expected_hash(tx, NETWORK);

        assert_eq!(got, want);
        assert_eq!(got.len(), 64, "hash must be 64 hex chars (32 bytes)");
    }

    #[test]
    fn different_network_passphrase_yields_a_different_hash() {
        let tx = sample_transaction();
        let b64 = envelope_base64(tx);

        let mainnet_hash =
            compute_transaction_hash(&b64, "Public Global Stellar Network ; September 2015")
                .unwrap();
        let testnet_hash = compute_transaction_hash(&b64, NETWORK).unwrap();

        assert_ne!(mainnet_hash, testnet_hash);
    }

    #[test]
    fn is_deterministic_for_the_same_input() {
        let tx = sample_transaction();
        let b64 = envelope_base64(tx);

        let first = compute_transaction_hash(&b64, NETWORK).unwrap();
        let second = compute_transaction_hash(&b64, NETWORK).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn rejects_garbage_xdr() {
        let err = compute_transaction_hash("not-valid-base64-xdr!!!", NETWORK).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_empty_string() {
        let err = compute_transaction_hash("", NETWORK).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn fee_bump_envelope_hashes_via_the_feebump_tagged_transaction() {
        use stellar_xdr::curr::{
            FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionInnerTx,
        };

        let inner_tx = sample_transaction();
        let inner_envelope = TransactionV1Envelope {
            tx: inner_tx,
            signatures: VecM::default(),
        };

        let source_id = stellar_strkey::ed25519::PublicKey::from_string(SOURCE).unwrap();
        let fee_bump_tx = FeeBumpTransaction {
            fee_source: MuxedAccount::Ed25519(Uint256(source_id.0)),
            fee: 1000,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner_envelope),
            ext: stellar_xdr::curr::FeeBumpTransactionExt::V0,
        };

        let envelope = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: fee_bump_tx.clone(),
            signatures: VecM::default(),
        });
        let b64 = envelope.to_xdr_base64(Limits::none()).unwrap();

        let got = compute_transaction_hash(&b64, NETWORK).unwrap();

        let network_id = Hash(Sha256::digest(NETWORK.as_bytes()).into());
        let payload = TransactionSignaturePayload {
            network_id,
            tagged_transaction: TransactionSignaturePayloadTaggedTransaction::TxFeeBump(
                fee_bump_tx,
            ),
        };
        let want = hex::encode(Sha256::digest(payload.to_xdr(Limits::none()).unwrap()));

        assert_eq!(got, want);
    }
}

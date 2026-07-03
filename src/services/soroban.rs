use crate::{
    error::{AppError, AppResult},
    services::stellar::StellarService,
};
use ed25519_dalek::{Signer, SigningKey};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stellar_xdr::curr::{
    DecoratedSignature, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Limits,
    Memo, MuxedAccount, Operation, OperationBody, Preconditions, ScAddress, ScSymbol, ScVal,
    SequenceNumber, Signature, SignatureHint, SorobanTransactionData, Transaction,
    TransactionEnvelope, TransactionExt, TransactionSignaturePayload,
    TransactionSignaturePayloadTaggedTransaction, TransactionV1Envelope, Uint256, VecM, WriteXdr,
};

/// Thin client for the Soroban RPC JSON-RPC API, plus helpers to build and
/// sign a *keeper-authored* contract invocation.
///
/// The keeper is a funded service account whose only job is to relay
/// already-authorized on-chain actions once conditions are met
/// (`execute_subscription`, `release_escrow`, `refund_escrow`) — the same
/// role a Chainlink Automation / Gelato keeper plays. It signs with its own
/// key, never a user's, and only pays the network fee: it cannot move a
/// user's funds beyond what the contract itself (authorized by the user,
/// on-chain, ahead of time) allows. This is what keeps StellarSend
/// non-custodial even though a background job is submitting transactions.
#[derive(Debug, Clone)]
pub struct SorobanService {
    client: Client,
    rpc_url: String,
}

/// Result of a `simulateTransaction` RPC call.
#[derive(Debug, Clone)]
pub struct SimulateResult {
    /// Minimum resource fee (in stroops) the network says this invocation
    /// will need, as estimated against the current ledger state.
    pub min_resource_fee: i64,
    /// Base64 XDR of the `SorobanTransactionData` (footprint + resource
    /// requirements) computed by simulation. `None` if simulation failed.
    pub transaction_data: Option<String>,
    pub error: Option<String>,
}

/// Result of a `sendTransaction` RPC call.
#[derive(Debug, Clone)]
pub struct SendResult {
    pub hash: String,
    pub status: String,
}

/// Result of a `getTransaction` RPC call (used to poll for confirmation).
#[derive(Debug, Clone)]
pub struct GetTransactionResult {
    pub status: String,
}

/// A contract function to invoke: `contract.function_name(args)`.
#[derive(Debug, Clone)]
pub struct ContractCallArgs {
    /// Strkey-encoded contract id, e.g. `"C...".`
    pub contract_id: String,
    pub function_name: String,
    pub args: Vec<ScVal>,
}

impl SorobanService {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            client,
            rpc_url: rpc_url.into(),
        }
    }

    async fn rpc_call(&self, method: &str, params: Value) -> AppResult<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let resp = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        let value: Value = resp.json().await.map_err(AppError::HttpClient)?;

        if let Some(err) = value.get("error") {
            return Err(AppError::SorobanError(err.to_string()));
        }

        value
            .get("result")
            .cloned()
            .ok_or_else(|| AppError::SorobanError("RPC response missing 'result'".into()))
    }

    /// Call `simulateTransaction` for an unsigned transaction envelope
    /// (base64 XDR) to learn the resource footprint and fee before signing.
    pub async fn simulate(&self, unsigned_envelope_b64: &str) -> AppResult<SimulateResult> {
        let result = self
            .rpc_call("simulateTransaction", json!({ "transaction": unsigned_envelope_b64 }))
            .await?;

        let error = result
            .get("error")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let min_resource_fee = result
            .get("minResourceFee")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        let transaction_data = result
            .get("transactionData")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(SimulateResult {
            min_resource_fee,
            transaction_data,
            error,
        })
    }

    /// Submit a signed transaction envelope (base64 XDR) to Soroban RPC.
    pub async fn send_transaction(&self, signed_envelope_b64: &str) -> AppResult<SendResult> {
        let result = self
            .rpc_call("sendTransaction", json!({ "transaction": signed_envelope_b64 }))
            .await?;

        Ok(SendResult {
            hash: result
                .get("hash")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            status: result
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string(),
        })
    }

    /// Poll `getTransaction` for the confirmation status of a previously
    /// submitted transaction hash.
    pub async fn get_transaction(&self, hash: &str) -> AppResult<GetTransactionResult> {
        let result = self.rpc_call("getTransaction", json!({ "hash": hash })).await?;

        Ok(GetTransactionResult {
            status: result
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string(),
        })
    }

    /// Build, simulate, sign, and submit a keeper-authored contract
    /// invocation end to end.
    ///
    /// `keeper_secret` is the keeper *service account's own* Stellar secret
    /// seed (`S...`) — configured via `KEEPER_SECRET_KEY` — never a user's
    /// key. The keeper only pays the transaction fee; whether the call
    /// actually succeeds (e.g. `execute_subscription` moving funds) is
    /// entirely gated by the on-chain authorization the payer already
    /// granted when the subscription/escrow was created.
    pub async fn invoke_contract_via_keeper(
        &self,
        stellar: &StellarService,
        network_passphrase: &str,
        keeper_secret: &str,
        call: &ContractCallArgs,
    ) -> AppResult<SendResult> {
        let seed = stellar_strkey::ed25519::PrivateKey::from_string(keeper_secret)
            .map_err(|_| AppError::KeeperUnavailable("KEEPER_SECRET_KEY is not a valid Stellar seed".into()))?;
        let signing_key = SigningKey::from_bytes(&seed.0);
        let keeper_public = stellar_strkey::ed25519::PublicKey(signing_key.verifying_key().to_bytes())
            .to_string();

        let account = stellar.get_account(&keeper_public).await?;
        let sequence: i64 = account
            .sequence
            .parse()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("Keeper account sequence was not numeric")))?;

        const BASE_FEE_STROOPS: u32 = 100;
        let mut tx = build_invoke_contract_tx(&keeper_public, sequence, BASE_FEE_STROOPS, call)?;

        // 1. Simulate against current ledger state to obtain the resource
        //    footprint & fee Soroban requires for this exact invocation.
        let unsigned = envelope_base64(&tx, Vec::new())?;
        let sim = self.simulate(&unsigned).await?;
        if let Some(err) = sim.error {
            return Err(AppError::SorobanError(err));
        }
        let tx_data_b64 = sim
            .transaction_data
            .ok_or_else(|| AppError::SorobanError("simulation returned no transactionData".into()))?;
        let soroban_data = <SorobanTransactionData as stellar_xdr::curr::ReadXdr>::from_xdr_base64(
            tx_data_b64,
            Limits::none(),
        )
        .map_err(|e| AppError::SorobanError(format!("invalid transactionData from simulation: {e}")))?;

        tx.ext = TransactionExt::V1(soroban_data);
        tx.fee = BASE_FEE_STROOPS.saturating_add(sim.min_resource_fee.clamp(0, u32::MAX as i64) as u32);

        // 2. Sign with the keeper's own key and submit.
        let signature = sign_payload(&tx, network_passphrase, &signing_key)?;
        let signed = envelope_base64(&tx, vec![signature])?;

        self.send_transaction(&signed).await
    }
}

/// Build the (unsigned) `Transaction` for invoking `call` from `source_account`,
/// consuming the next sequence number after `source_sequence`.
pub fn build_invoke_contract_tx(
    source_account: &str,
    source_sequence: i64,
    base_fee: u32,
    call: &ContractCallArgs,
) -> AppResult<Transaction> {
    let contract = stellar_strkey::Contract::from_string(&call.contract_id)
        .map_err(|_| AppError::BadRequest("Invalid contract id".into()))?;
    let source_pk = stellar_strkey::ed25519::PublicKey::from_string(source_account)
        .map_err(|_| AppError::BadRequest("Invalid keeper source account".into()))?;

    let function_name: ScSymbol = call
        .function_name
        .as_str()
        .try_into()
        .map_err(|_| AppError::BadRequest("Function name too long for ScSymbol".into()))?;

    let args_vec: VecM<ScVal> = call
        .args
        .clone()
        .try_into()
        .map_err(|_| AppError::BadRequest("Too many contract call arguments".into()))?;

    let invoke_args = InvokeContractArgs {
        contract_address: ScAddress::Contract(Hash(contract.0)),
        function_name,
        args: args_vec,
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth: VecM::default(),
        }),
    };

    let operations: VecM<Operation, 100> = vec![op]
        .try_into()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Too many operations")))?;

    Ok(Transaction {
        source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
        fee: base_fee,
        seq_num: SequenceNumber(source_sequence + 1),
        cond: Preconditions::None,
        memo: Memo::None,
        operations,
        ext: TransactionExt::V0,
    })
}

/// Encode a `Transaction` plus an optional set of decorated signatures as a
/// base64 `TransactionEnvelope` XDR string.
fn envelope_base64(tx: &Transaction, signatures: Vec<DecoratedSignature>) -> AppResult<String> {
    let signatures = signatures
        .try_into()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Too many signatures")))?;

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx: tx.clone(),
        signatures,
    });

    envelope
        .to_xdr_base64(Limits::none())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("XDR encode error: {e}")))
}

/// Sign the transaction's signature payload (SHA-256 of the tagged
/// transaction preceded by the network id) with the keeper's ed25519 key.
fn sign_payload(
    tx: &Transaction,
    network_passphrase: &str,
    signing_key: &SigningKey,
) -> AppResult<DecoratedSignature> {
    let network_id = Hash(Sha256::digest(network_passphrase.as_bytes()).into());
    let payload = TransactionSignaturePayload {
        network_id,
        tagged_transaction: TransactionSignaturePayloadTaggedTransaction::Tx(tx.clone()),
    };

    let payload_bytes = payload
        .to_xdr(Limits::none())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("XDR encode error: {e}")))?;
    let hash: [u8; 32] = Sha256::digest(&payload_bytes).into();

    let signature = signing_key.sign(&hash);
    let verifying_bytes = signing_key.verifying_key().to_bytes();
    let hint = SignatureHint([
        verifying_bytes[28],
        verifying_bytes[29],
        verifying_bytes[30],
        verifying_bytes[31],
    ]);

    Ok(DecoratedSignature {
        hint,
        signature: Signature(
            signature
                .to_bytes()
                .to_vec()
                .try_into()
                .map_err(|_| AppError::Internal(anyhow::anyhow!("Bad signature length")))?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A syntactically valid throwaway keypair/contract id used only to
    // exercise the pure XDR-building code below (no network access).
    const TEST_SOURCE: &str = "GBZXN7PIRZGNMHGA7MUUUF4GWPY5AYPV6LY4UV2GL6VJGIQRXFDNMADI";

    fn test_contract_id() -> String {
        stellar_strkey::Contract([7u8; 32]).to_string()
    }

    #[test]
    fn builds_invoke_contract_transaction_deterministically() {
        let call = ContractCallArgs {
            contract_id: test_contract_id(),
            function_name: "execute_subscription".to_string(),
            args: vec![ScVal::U64(42)],
        };

        let tx = build_invoke_contract_tx(TEST_SOURCE, 10, 100, &call)
            .expect("building the tx should succeed for valid inputs");

        assert_eq!(tx.seq_num.0, 11, "sequence number must be source_sequence + 1");
        assert_eq!(tx.operations.len(), 1);
        match &tx.operations[0].body {
            OperationBody::InvokeHostFunction(op) => match &op.host_function {
                HostFunction::InvokeContract(args) => {
                    assert_eq!(args.function_name.0.to_string(), "execute_subscription");
                    assert_eq!(args.args.len(), 1);
                }
                _ => panic!("expected InvokeContract host function"),
            },
            _ => panic!("expected InvokeHostFunction operation"),
        }
    }

    #[test]
    fn rejects_invalid_contract_id() {
        let call = ContractCallArgs {
            contract_id: "not-a-contract-id".to_string(),
            function_name: "execute_subscription".to_string(),
            args: vec![],
        };

        let err = build_invoke_contract_tx(TEST_SOURCE, 0, 100, &call).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_invalid_source_account() {
        let call = ContractCallArgs {
            contract_id: test_contract_id(),
            function_name: "execute_subscription".to_string(),
            args: vec![],
        };

        let err = build_invoke_contract_tx("not-an-account", 0, 100, &call).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn envelope_round_trips_through_xdr() {
        let call = ContractCallArgs {
            contract_id: test_contract_id(),
            function_name: "release_escrow".to_string(),
            args: vec![ScVal::U64(7)],
        };
        let tx = build_invoke_contract_tx(TEST_SOURCE, 0, 100, &call).unwrap();

        let b64 = envelope_base64(&tx, Vec::new()).expect("encoding should succeed");
        let decoded = <TransactionEnvelope as stellar_xdr::curr::ReadXdr>::from_xdr_base64(
            &b64,
            Limits::none(),
        )
        .expect("decoding should succeed");

        match decoded {
            TransactionEnvelope::Tx(env) => assert_eq!(env.tx.seq_num.0, tx.seq_num.0),
            _ => panic!("expected TransactionEnvelope::Tx"),
        }
    }
}

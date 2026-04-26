use async_trait::async_trait;
use reqwest::{Client, ClientBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use std::error::Error;
use std::time::Duration;
use tracing::{info, warn};

// Default timeout in seconds if none is specified
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const SATOSHIS_PER_SYS: f64 = 100_000_000.0;
const VBYTES_PER_KVB: f64 = 1000.0;
const NEVM_DATA_SCALE_FACTOR: f64 = 0.01;

/// Maximum payload accepted by the Syscoin PoDA endpoint (2 MiB).
pub const MAX_BLOB_SIZE: usize = 2 * 1024 * 1024;

/// Thread-safe error type
pub type SyscoinError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitcoinDaFinalityMode {
    Chainlock,
    Confirmations,
}

/// Response structure for JSON-RPC calls
#[derive(Deserialize, Debug)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<Value>,
}

/// Common trait for RPC clients to enable easy mocking
#[async_trait]
pub trait RpcClient {
    /// Make a generic RPC call with any method and parameters
    async fn call(&self, method: &str, params: &[Value]) -> Result<Value, SyscoinError>;

    async fn call_wallet(&self, method: &str, params: &[Value]) -> Result<Value, SyscoinError>;

    /// Get wallet balance with optional account and watchonly parameters
    async fn get_balance(
        &self,
        account: Option<&str>,
        include_watchonly: Option<bool>,
    ) -> Result<f64, SyscoinError>;

    /// Make an HTTP GET request to the specified URL
    async fn http_get(&self, url: &str) -> Result<Vec<u8>, SyscoinError>;
}

/// Production implementation of the RPC client for Syscoin
pub struct RealRpcClient {
    rpc_url: String,
    rpc_user: String,
    rpc_password: String,
    http_client: Client,
    timeout: Duration,
    wallet_name: String,
}

impl RealRpcClient {
    /// Create a new RPC client with default timeout
    pub fn new(
        rpc_url: &str,
        rpc_user: &str,
        rpc_password: &str,
        timeout: Option<Duration>,
        wallet_name: &str,
    ) -> Result<Self, SyscoinError> {
        Self::new_with_timeout(rpc_url, rpc_user, rpc_password, timeout, wallet_name)
    }

    /// Create a new RPC client with custom timeout
    pub fn new_with_timeout(
        rpc_url: &str,
        rpc_user: &str,
        rpc_password: &str,
        timeout: Option<Duration>,
        wallet_name: &str,
    ) -> Result<Self, SyscoinError> {
        let timeout = timeout.unwrap_or_else(|| Duration::from_secs(DEFAULT_TIMEOUT_SECS));

        // Bitcoin/Syscoin JSON-RPC over HTTP/1.1 often closes idle keep-alive connections on the
        // server side long before reqwest's default pool idle timeout (90s). We poll finality with
        // gaps (e.g. `bitcoin_da_finality_poll_interval`), then reuse a pooled socket that the server
        // already closed → reqwest yields `error sending request for url (...)` with no JSON body.
        // Drop idle sockets quickly so each poll tends to open a fresh connection.
        let http_client = ClientBuilder::new()
            .timeout(timeout)
            .pool_idle_timeout(Some(Duration::from_secs(2)))
            .tcp_keepalive(Some(Duration::from_secs(15)))
            .build()?;

        Ok(Self {
            rpc_url: rpc_url.to_string(),
            rpc_user: rpc_user.to_string(),
            rpc_password: rpc_password.to_string(),
            http_client,
            timeout,
            wallet_name: wallet_name.to_string(),
        })
    }

    /// Send a JSON-RPC request to the Syscoin node
    async fn rpc_request(&self, method: &str, params: &[Value]) -> Result<Value, SyscoinError> {
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        // fire the HTTP call
        let resp = self
            .http_client
            .post(&self.rpc_url)
            .basic_auth(&self.rpc_user, Some(&self.rpc_password))
            .json(&request_body)
            .timeout(self.timeout)
            .send()
            .await?;

        // pull the entire body into a String
        let status = resp.status();
        let body = resp.text().await?;

        // log whatever the node actually sent us
        info!("RPC `{}` → HTTP {}:\n{}", method, status, body);

        // if it wasn’t a 200, include the body in our Err
        if !status.is_success() {
            return Err(format!("HTTP error: {} returned body: {}", status, body).into());
        }

        // now parse the JSON-RPC envelope from the text
        let jr: JsonRpcResponse<Value> = serde_json::from_str(&body)?;
        if let Some(err) = jr.error {
            // you can pull out err["code"] and err["message"] here too
            return Err(format!("RPC error: {}", err).into());
        }

        jr.result
            .ok_or_else(|| "missing result in JSON-RPC response".into())
    }

    /// Like `rpc_request`, but points at `/wallet/{wallet_name}` on the node
    async fn wallet_rpc_request(
        &self,
        method: &str,
        params: &[Value],
    ) -> Result<Value, SyscoinError> {
        // build the JSON-RPC envelope
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        // compute the wallet-specific URL
        let base = self.rpc_url.trim_end_matches('/');
        let url = format!("{}/wallet/{}", base, self.wallet_name);

        // fire the HTTP call
        let resp = self
            .http_client
            .post(&url)
            .basic_auth(&self.rpc_user, Some(&self.rpc_password))
            .json(&request_body)
            .timeout(self.timeout)
            .send()
            .await?;

        // pull the entire body into a String
        let status = resp.status();
        let body = resp.text().await?;

        // log whatever the node actually sent us
        info!("WALLET RPC `{}` → HTTP {}:\n{}", method, status, body);

        // if it wasn’t a 200, include the body in our Err
        if !status.is_success() {
            return Err(format!("HTTP error: {} returned body: {}", status, body).into());
        }

        // now parse the JSON-RPC envelope
        let jr: JsonRpcResponse<Value> = serde_json::from_str(&body)?;

        // if the RPC server reported an application-level error, forward it
        if let Some(err) = jr.error {
            return Err(format!("RPC error: {}", err).into());
        }

        // otherwise grab the result or error out if missing
        jr.result
            .ok_or_else(|| "missing result in JSON-RPC response".into())
    }

    /// Create or load a wallet by name
    pub async fn create_or_load_wallet(&self, wallet_name: &str) -> Result<(), SyscoinError> {
        info!("create_or_load_wallet");
        match self.call("loadwallet", &[json!(wallet_name)]).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                info!("wallet error");
                let s = e.to_string();
                info!(s);
                // -18 = wallet not found → create it
                if s.contains("failed") {
                    info!("wallet not found, creating new one");
                    self.call("createwallet", &[json!(wallet_name)]).await?;
                    return Ok(());
                }
                // -4 = wallet already loaded → ignore
                if s.contains("already loaded") {
                    info!("wallet already loaded, continuing");
                    return Ok(());
                }
                // any other error is fatal
                return Err(e);
            }
        }
    }

    /// Expose the configured wallet name
    pub fn wallet_name(&self) -> &str {
        &self.wallet_name
    }
}

#[async_trait]
impl RpcClient for RealRpcClient {
    async fn call(&self, method: &str, params: &[Value]) -> Result<Value, SyscoinError> {
        self.rpc_request(method, params).await
    }

    async fn call_wallet(&self, method: &str, params: &[Value]) -> Result<Value, SyscoinError> {
        self.wallet_rpc_request(method, params).await
    }

    async fn get_balance(
        &self,
        account: Option<&str>,
        include_watchonly: Option<bool>,
    ) -> Result<f64, SyscoinError> {
        let mut params = Vec::new();
        if let Some(acct) = account {
            params.push(json!(acct));
            if let Some(w) = include_watchonly {
                params.push(json!(w));
            }
        }
        let v = self.wallet_rpc_request("getbalance", &params).await?;
        v.as_f64().ok_or_else(|| "Invalid balance format".into())
    }

    async fn http_get(&self, url: &str) -> Result<Vec<u8>, SyscoinError> {
        let response = self.http_client.get(url).send().await?;

        if !response.status().is_success() {
            return Err(format!("HTTP GET error: {}", response.status()).into());
        }

        Ok(response.bytes().await?.to_vec())
    }
}

pub struct SyscoinClient {
    rpc_client: RealRpcClient,
    poda_url: String,
}

fn parse_amount_value(value: &Value) -> Result<f64, SyscoinError> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse::<f64>().ok()))
        .ok_or_else(|| "Invalid amount format".into())
}

impl SyscoinClient {
    fn normalized_blob_id<'a>(&self, blob_id: &'a str) -> &'a str {
        blob_id.strip_prefix("0x").unwrap_or(blob_id)
    }

    fn poda_candidate_urls(&self, version_hash: &str) -> Vec<String> {
        let normalized_hash = self.normalized_blob_id(version_hash);
        let base = self.poda_url.trim_end_matches('/');

        if base.ends_with("/blob") || base.ends_with("/vh") {
            return vec![format!("{base}/{normalized_hash}")];
        }

        vec![
            format!("{base}/blob/{normalized_hash}"),
            format!("{base}/vh/{normalized_hash}"),
        ]
    }

    async fn blob_exists_in_cloud(&self, version_hash: &str) -> bool {
        for url in self.poda_candidate_urls(version_hash) {
            match self.rpc_client.http_get(&url).await {
                Ok(_) => {
                    info!("PODA fallback located blob at {}", url);
                    return true;
                }
                Err(err) => {
                    warn!("PODA fallback lookup failed at {}: {}", url, err);
                }
            }
        }

        false
    }

    /// Create a new Syscoin client
    pub fn new(
        rpc_url: &str,
        rpc_user: &str,
        rpc_password: &str,
        poda_url: &str,
        timeout: Option<Duration>,
        wallet_name: &str,
    ) -> Result<Self, SyscoinError> {
        info!("Initializing Client");
        let rpc_client =
            RealRpcClient::new_with_timeout(rpc_url, rpc_user, rpc_password, timeout, wallet_name)?;

        Ok(Self {
            rpc_client,
            poda_url: poda_url.to_string(),
        })
    }

    /// Create a blob in BitcoinDA(FKA Poda) storage
    pub async fn create_blob(&self, data: &[u8]) -> Result<String, SyscoinError> {
        if data.len() > MAX_BLOB_SIZE {
            return Err(format!(
                "blob size ({}) exceeds maximum allowed ({})",
                data.len(),
                MAX_BLOB_SIZE
            )
            .into());
        }

        let data_hex = hex::encode(data);
        // pass positional args: data hex, overwrite_existing, hash type.
        // Keep overwrite_existing=false to make repeated calls idempotent for identical data.
        // Force blake2s to keep blob IDs aligned with Syscoin / OS expectations.
       // let params = vec![json!(data_hex), json!(false), json!("blake2s")];
        let params = vec![json!(data_hex), json!(false), json!("keccak")];
        // SYSCOIN
        let response = self
            .rpc_client
            .call_wallet("syscoincreatenevmblob", &params)
            .await?;
        let hash = response
            .get("versionhash")
            .and_then(|v| v.as_str())
            .ok_or("Missing versionhash")?;
        Ok(hash.to_string())
    }

    /// Ensure there is a receive address for the provided label.
    /// If none exists, a new address is created and returned.
    pub async fn ensure_address_by_label(
        &self,
        address_label: &str,
    ) -> Result<String, SyscoinError> {
        match self.fetch_address_by_label(address_label).await? {
            Some(existing) => Ok(existing),
            None => self.get_new_address(address_label).await,
        }
    }

    /// Ensure the wallet is created/loaded and return a labeled funding address.
    /// This is idempotent and safe to call on startup.
    pub async fn ensure_wallet_and_address(
        &self,
        wallet_name: &str,
        address_label: &str,
    ) -> Result<String, SyscoinError> {
        self.create_or_load_wallet(wallet_name).await?;
        self.ensure_address_by_label(address_label).await
    }

    /// Ensure the internally-configured wallet is loaded and return a labeled address
    pub async fn ensure_own_wallet_and_address(
        &self,
        address_label: &str,
    ) -> Result<String, SyscoinError> {
        let wallet_name = self.rpc_client.wallet_name();
        self.ensure_wallet_and_address(wallet_name, address_label)
            .await
    }

    /// Get wallet balance
    pub async fn get_balance(&self) -> Result<f64, SyscoinError> {
        self.rpc_client.get_balance(None, None).await
    }

    /// Return the effective Syscoin blob base fee per blob byte, after applying
    /// the network minimum fee and the NEVM blob size discount factor.
    pub async fn get_blob_base_fee(&self, conf_target: u16) -> Result<u128, SyscoinError> {
        let estimate = self
            .rpc_client
            .call(
                "estimatesmartfee",
                &[json!(conf_target), json!("economical")],
            )
            .await?;
        let estimate_fee_per_kvb = estimate
            .get("feerate")
            .map(parse_amount_value)
            .transpose()?
            .unwrap_or(0.0);

        let mempool_info = self.rpc_client.call("getmempoolinfo", &[]).await?;
        let mempool_min_fee_per_kvb = mempool_info
            .get("mempoolminfee")
            .map(parse_amount_value)
            .transpose()?
            .unwrap_or(0.0);
        let min_relay_fee_per_kvb = mempool_info
            .get("minrelaytxfee")
            .map(parse_amount_value)
            .transpose()?
            .unwrap_or(0.0);

        let effective_fee_per_kvb = estimate_fee_per_kvb
            .max(mempool_min_fee_per_kvb)
            .max(min_relay_fee_per_kvb);
        if effective_fee_per_kvb <= 0.0 {
            return Err("Failed to determine Syscoin blob base fee".into());
        }

        let sat_per_kvb = effective_fee_per_kvb * SATOSHIS_PER_SYS;
        let sat_per_blob_byte = (sat_per_kvb / VBYTES_PER_KVB * NEVM_DATA_SCALE_FACTOR).ceil();
        Ok((sat_per_blob_byte as u128).max(1))
    }

    /// Fetch a blob; tries RPC first, then falls back to PoDA cloud
    pub async fn get_blob(&self, blob_id: &str) -> Result<Vec<u8>, SyscoinError> {
        match self.get_blob_from_rpc(blob_id).await {
            Ok(data) => Ok(data),
            Err(e) => {
                warn!("get_blob_from_rpc failed ({e}); falling back to cloud");
                self.get_blob_from_cloud(blob_id).await
            }
        }
    }

    /// Get a fresh address for a given label
    pub async fn get_new_address(&self, address_label: &str) -> Result<String, SyscoinError> {
        let resp = self
            .rpc_client
            .call_wallet("getnewaddress", &[json!(address_label)])
            .await?;
        resp.as_str()
            .map(|s| s.to_owned())
            .ok_or_else(|| "getnewaddress returned non-string".into())
    }

    /// Fetch an existing address by label, if any
    pub async fn fetch_address_by_label(
        &self,
        address_label: &str,
    ) -> Result<Option<String>, SyscoinError> {
        // — pass the label as a bare string —
        let resp = match self
            .rpc_client
            .call_wallet("getaddressesbylabel", &[json!(address_label)])
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                // if it's the "no addresses" error, swallow it as None
                if msg.contains("\"code\":-11") {
                    return Ok(None);
                }
                // otherwise re-propagate
                return Err(e);
            }
        };

        // parse returned map, take the first key if any
        if let Some(map) = resp.as_object() {
            if let Some((addr, _)) = map.iter().next() {
                return Ok(Some(addr.clone()));
            }
        }
        Ok(None)
    }

    /// Retrieve blob data from RPC node
    /// Retrieve blob data from RPC node
    async fn get_blob_from_rpc(&self, blob_id: &str) -> Result<Vec<u8>, SyscoinError> {
        // Strip any 0x prefix
        let actual_blob_id = blob_id.strip_prefix("0x").unwrap_or(blob_id);

        // Use positional parameters: (versionhash_or_txid: String, getdata: bool)
        let params = vec![json!(actual_blob_id), json!(true)];

        // 1) Call RPC
        let response = self.rpc_client.call("getnevmblobdata", &params).await?;

        let hex_data = response
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or("Missing data in getnevmblobdata response")?;

        if let Some(txid) = response.get("txid").and_then(|v| v.as_str()) {
            let tx_link = format!("https://explorer-blockbook.syscoin.org/tx/{}", txid);
            info!("🔗 View this transaction on Syscoin Explorer: {}", tx_link);
        } else {
            warn!("No txid field in getnevmblobdata response, cannot log explorer link");
        }

        // 5) Decode the hex (stripping an optional "0x")
        let data_to_decode = hex_data.strip_prefix("0x").unwrap_or(hex_data);
        Ok(hex::decode(data_to_decode)?)
    }

    /// Retrieve blob data from PODA cloud storage
    pub async fn get_blob_from_cloud(&self, version_hash: &str) -> Result<Vec<u8>, SyscoinError> {
        let mut last_err: Option<SyscoinError> = None;

        for url in self.poda_candidate_urls(version_hash) {
            match self.rpc_client.http_get(&url).await {
                Ok(bytes) => return Ok(bytes),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| "failed to build PODA URL".into()))
    }

    /// Check if a blob is final
    pub async fn check_blob_finality(&self, blob_id: &str) -> Result<bool, SyscoinError> {
        // Strip any 0x prefix
        let actual_blob_id = if let Some(stripped) = blob_id.strip_prefix("0x") {
            stripped
        } else {
            blob_id
        };

        // Use positional parameter: (versionhash_or_txid: String)
        let params = vec![json!(actual_blob_id)];

        // If the node does not know the blob yet, it may return an HTTP 500 with
        // a JSON-RPC error body like:
        // {"result":null,"error":{"code":-32602,"message":"Could not find blob information for versionhash ..."},"id":1}
        // Treat this as "not final yet" instead of a hard error so that the
        // dispatcher keeps polling for finality.
        let response = match self.rpc_client.call("getnevmblobdata", &params).await {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Could not find blob information for versionhash")
                    || msg.contains("\"code\":-32602")
                {
                    if self.blob_exists_in_cloud(actual_blob_id).await {
                        warn!(
                            "RPC finality lookup could not find blob {}; accepting PODA archive presence as finalized",
                            actual_blob_id
                        );
                        return Ok(true);
                    }
                    return Ok(false);
                }
                return Err(e);
            }
        };

        // Extract finality status from response
        let is_final = response
            .get("chainlock")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(is_final)
    }

    pub async fn check_blob_finality_with_mode(
        &self,
        blob_id: &str,
        mode: BitcoinDaFinalityMode,
        confirmations: u64,
    ) -> Result<bool, SyscoinError> {
        match mode {
            BitcoinDaFinalityMode::Chainlock => self.check_blob_finality(blob_id).await,
            BitcoinDaFinalityMode::Confirmations => {
                self.check_blob_finality_by_confirmations(blob_id, confirmations)
                    .await
            }
        }
    }

    /// Check if a blob is final based on a required number of confirmations.
    pub async fn check_blob_finality_by_confirmations(
        &self,
        blob_id: &str,
        confirmations: u64,
    ) -> Result<bool, SyscoinError> {
        let actual_blob_id = blob_id.strip_prefix("0x").unwrap_or(blob_id);
        let params = vec![json!(actual_blob_id)];

        let response = match self.rpc_client.call("getnevmblobdata", &params).await {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Could not find blob information for versionhash")
                    || msg.contains("\"code\":-32602")
                {
                    if self.blob_exists_in_cloud(actual_blob_id).await {
                        warn!(
                            "RPC confirmation lookup could not find blob {}; accepting PODA archive presence as finalized",
                            actual_blob_id
                        );
                        return Ok(true);
                    }
                    return Ok(false);
                }
                return Err(e);
            }
        };

        let Some(blob_height) = response.get("height").and_then(|v| v.as_u64()) else {
            // Unconfirmed blobs may not have a mined height yet; treat this as "not final"
            // so callers keep polling instead of crashing.
            return Ok(false);
        };

        let current_height = self
            .rpc_client
            .call("getblockcount", &[])
            .await?
            .as_u64()
            .ok_or("getblockcount returned non-u64 result")?;

        Ok(current_height.saturating_sub(blob_height) + 1 >= confirmations)
    }

    /// Create or load a wallet by name
    pub async fn create_or_load_wallet(&self, wallet_name: &str) -> Result<(), SyscoinError> {
        self.rpc_client.create_or_load_wallet(wallet_name).await
    }
}

/// Mock implementation for testing
#[cfg(test)]
pub struct MockRpcClient {
    // Add any fields needed for test state
}

#[cfg(test)]
#[async_trait]
impl RpcClient for MockRpcClient {
    async fn call(&self, method: &str, _params: &[Value]) -> Result<Value, SyscoinError> {
        // Return mock responses based on the method
        match method {
            "getbalance" => Ok(json!(10.5)),
            "syscoincreatenevmblob" => Ok(json!({ "versionhash": "mock_blob_hash" })),
            "getnevmblobdata" => Ok(json!({ "data": hex::encode(b"mock_data") })),
            "loadwallet" => Ok(json!(null)),
            "createwallet" => Ok(json!(null)),
            _ => Err("Unimplemented mock method".into()),
        }
    }

    async fn call_wallet(&self, method: &str, _params: &[Value]) -> Result<Value, SyscoinError> {
        // Return mock responses based on the method
        match method {
            "getbalance" => Ok(json!(10.5)),
            "syscoincreatenevmblob" => Ok(json!({ "versionhash": "mock_blob_hash" })),
            "getnevmblobdata" => Ok(json!({ "data": hex::encode(b"mock_data") })),
            "loadwallet" => Ok(json!(null)),
            "createwallet" => Ok(json!(null)),
            _ => Err("Unimplemented mock method".into()),
        }
    }

    async fn get_balance(
        &self,
        _account: Option<&str>,
        _include_watchonly: Option<bool>,
    ) -> Result<f64, SyscoinError> {
        Ok(10.5)
    }

    async fn http_get(&self, _url: &str) -> Result<Vec<u8>, SyscoinError> {
        Ok(b"mock_data".to_vec())
    }
}

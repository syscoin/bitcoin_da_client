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
pub const MAX_BLOB_EXISTENCE_BATCH: usize = 32;

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
    id: Option<Value>,
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

    async fn rpc_batch_request(
        &self,
        calls: &[(&str, Vec<Value>)],
    ) -> Result<Vec<Result<Value, SyscoinError>>, SyscoinError> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }

        let request_body: Vec<_> = calls
            .iter()
            .enumerate()
            .map(|(id, (method, params))| {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": params,
                })
            })
            .collect();

        let resp = self
            .http_client
            .post(&self.rpc_url)
            .basic_auth(&self.rpc_user, Some(&self.rpc_password))
            .json(&request_body)
            .timeout(self.timeout)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        info!("RPC batch → HTTP {}:\n{}", status, body);

        if !status.is_success() {
            return Err(format!("HTTP error: {} returned body: {}", status, body).into());
        }

        let responses: Vec<JsonRpcResponse<Value>> = serde_json::from_str(&body)?;
        let mut results = Vec::with_capacity(calls.len());
        results.resize_with(calls.len(), || None);

        for response in responses {
            let Some(id) = response.id.and_then(|value| value.as_u64()) else {
                return Err("missing id in JSON-RPC batch response".into());
            };
            let id = usize::try_from(id)?;
            if id >= calls.len() {
                return Err(format!("unexpected id {id} in JSON-RPC batch response").into());
            }

            let result = if let Some(err) = response.error {
                Err(format!("RPC error: {}", err).into())
            } else {
                response
                    .result
                    .ok_or_else(|| "missing result in JSON-RPC response".into())
            };
            results[id] = Some(result);
        }

        let ordered_results = results
            .into_iter()
            .enumerate()
            .map(|(id, result)| {
                result.unwrap_or_else(|| Err(format!("missing response for batch id {id}").into()))
            })
            .collect();
        Ok(ordered_results)
    }

    async fn http_post_json(&self, url: &str, body: &Value) -> Result<Vec<u8>, SyscoinError> {
        let response = self.http_client.post(url).json(body).send().await?;

        if !response.status().is_success() {
            return Err(format!("HTTP POST error: {}", response.status()).into());
        }

        Ok(response.bytes().await?.to_vec())
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

    fn poda_blob_url(&self, version_hash: &str) -> String {
        let normalized_hash = self.normalized_blob_id(version_hash);
        let base = self.poda_url.trim_end_matches('/');

        if base.ends_with("/vh") {
            return format!("{base}/{normalized_hash}");
        }

        format!("{base}/vh/{normalized_hash}")
    }

    fn poda_check_vh_batch_url(&self) -> String {
        let mut base = self.poda_url.trim_end_matches('/');

        for suffix in ["/vh", "/check_vh"] {
            if let Some(stripped) = base.strip_suffix(suffix) {
                base = stripped;
                break;
            }
        }

        format!("{base}/check_vh")
    }

    fn truthy_check_vh_response(response: &str) -> bool {
        !matches!(
            response.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "missing" | "not_found" | "not found" | "notfound"
        )
    }

    fn check_vh_value_exists(value: &Value) -> bool {
        match value {
            Value::Bool(exists) => *exists,
            Value::Number(number) => number.as_u64().is_some_and(|value| value != 0),
            Value::String(value) => Self::truthy_check_vh_response(value),
            Value::Object(object) => {
                if object.get("error").is_some_and(|error| !error.is_null()) {
                    return false;
                }

                ["exists", "found", "available", "result"]
                    .into_iter()
                    .find_map(|key| object.get(key))
                    .map_or(true, Self::check_vh_value_exists)
            }
            _ => true,
        }
    }

    fn check_vh_batch_response_exists(
        bytes: &[u8],
        version_hashes: &[String],
    ) -> Result<Vec<bool>, SyscoinError> {
        let value: Value = serde_json::from_slice(bytes)?;
        let aggregate_result =
            |value: &Value| vec![Self::check_vh_value_exists(value); version_hashes.len()];

        match value {
            Value::Array(values) => {
                if values.len() != version_hashes.len() {
                    return Err(format!(
                        "check_vh batch response length mismatch: expected {}, got {}",
                        version_hashes.len(),
                        values.len()
                    )
                    .into());
                }
                Ok(values.iter().map(Self::check_vh_value_exists).collect())
            }
            Value::Object(object) => {
                for key in ["results", "result"] {
                    if let Some(value) = object.get(key) {
                        if let Value::Array(values) = value {
                            if values.len() != version_hashes.len() {
                                return Err(format!(
                                    "check_vh batch response length mismatch: expected {}, got {}",
                                    version_hashes.len(),
                                    values.len()
                                )
                                .into());
                            }
                            return Ok(values.iter().map(Self::check_vh_value_exists).collect());
                        }
                        return Ok(aggregate_result(value));
                    }
                }

                for key in ["exists", "found", "available"] {
                    if let Some(value) = object.get(key) {
                        return Ok(aggregate_result(value));
                    }
                }

                version_hashes
                    .iter()
                    .map(|version_hash| {
                        object
                            .get(version_hash)
                            .map(Self::check_vh_value_exists)
                            .ok_or_else(|| {
                                format!("missing check_vh result for version hash {version_hash}")
                                    .into()
                            })
                    })
                    .collect()
            }
            other => Ok(aggregate_result(&other)),
        }
    }

    async fn blobs_exist_in_cloud(
        &self,
        version_hashes: &[String],
    ) -> Result<Vec<bool>, SyscoinError> {
        if version_hashes.is_empty() {
            return Ok(Vec::new());
        }

        let url = self.poda_check_vh_batch_url();
        let body = json!(version_hashes);
        let bytes = self.rpc_client.http_post_json(&url, &body).await?;
        Self::check_vh_batch_response_exists(&bytes, version_hashes)
    }

    async fn blob_exists_in_cloud(&self, version_hash: &str) -> bool {
        match self.blobs_exist_in_cloud(&[version_hash.to_string()]).await {
            Ok(mut exists) => exists.pop().unwrap_or(false),
            Err(err) => {
                warn!("PODA fallback check_vh lookup failed: {}", err);
                false
            }
        }
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
        let params = vec![json!(data_hex), json!(false), json!("blake2s")];
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
        self.rpc_client
            .http_get(&self.poda_blob_url(version_hash))
            .await
    }

    /// Check whether a blob is retrievable from the Syscoin node or PODA cloud storage.
    ///
    /// This is an availability check only. It deliberately does not imply chainlock or
    /// confirmation finality.
    pub async fn blob_exists(&self, blob_id: &str) -> Result<bool, SyscoinError> {
        self.blobs_exist([blob_id])
            .await?
            .pop()
            .ok_or_else(|| "missing blob existence result".into())
    }

    /// Check whether up to 32 blobs are retrievable from the Syscoin node or PODA cloud storage.
    ///
    /// The Syscoin node lookup is sent as a single JSON-RPC batch with `getdata=false`.
    /// Any hashes missing from the node are checked with a single PODA `check_vh` batch call.
    pub async fn blobs_exist<I, S>(&self, blob_ids: I) -> Result<Vec<bool>, SyscoinError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let actual_blob_ids: Vec<String> = blob_ids
            .into_iter()
            .map(|blob_id| self.normalized_blob_id(blob_id.as_ref()).to_string())
            .collect();

        if actual_blob_ids.len() > MAX_BLOB_EXISTENCE_BATCH {
            return Err(format!(
                "blob existence batch exceeds maximum of {}: got {}",
                MAX_BLOB_EXISTENCE_BATCH,
                actual_blob_ids.len()
            )
            .into());
        }
        if actual_blob_ids.is_empty() {
            return Ok(Vec::new());
        }

        let calls: Vec<_> = actual_blob_ids
            .iter()
            .map(|blob_id| ("getnevmblobdata", vec![json!(blob_id), json!(false)]))
            .collect();
        let rpc_results = self.rpc_client.rpc_batch_request(&calls).await?;

        let mut exists = vec![false; actual_blob_ids.len()];
        let mut missing = Vec::new();
        for (idx, result) in rpc_results.into_iter().enumerate() {
            match result {
                Ok(_) => exists[idx] = true,
                Err(err) => {
                    let msg = err.to_string();
                    if msg.contains("Could not find blob information for versionhash")
                        || msg.contains("\"code\":-32602")
                    {
                        missing.push((idx, actual_blob_ids[idx].clone()));
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        if missing.is_empty() {
            return Ok(exists);
        }

        let missing_hashes: Vec<_> = missing
            .iter()
            .map(|(_, version_hash)| version_hash.clone())
            .collect();
        let cloud_results = self.blobs_exist_in_cloud(&missing_hashes).await?;
        for ((idx, _), cloud_exists) in missing.into_iter().zip(cloud_results) {
            exists[idx] = cloud_exists;
        }

        Ok(exists)
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

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn blob_exists_does_not_imply_finality() {
        let mut server = Server::new_async().await;
        let _availability_lookup = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"[{"jsonrpc":"2.0","id":0,"method":"getnevmblobdata","params":["abc",false]}]"#
                    .to_string(),
            ))
            .with_status(200)
            .with_body(r#"[{"jsonrpc":"2.0","id":0,"result":{"data":"00"},"error":null}]"#)
            .expect(1)
            .create_async()
            .await;
        let _finality_lookup = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"jsonrpc":"2.0","id":1,"method":"getnevmblobdata","params":["abc"]}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"data":"00","chainlock":false}}"#)
            .expect(1)
            .create_async()
            .await;
        let client = SyscoinClient::new(&server.url(), "user", "password", &server.url(), None, "")
            .expect("client should initialize");

        assert!(client.blob_exists("abc").await.unwrap());
        assert!(!client.check_blob_finality("abc").await.unwrap());
    }
}

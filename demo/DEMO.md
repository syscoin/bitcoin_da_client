# BitcoinDA Rust Client Demo

A concise walkthrough of using the `bitcoin_da_client` Rust library to interact with Syscoin’s Data Availability (DA) layer. 
This document is organized so both technical and non-technical audiences can understand the ‘what’, ‘why’, and ‘how’.

---

## Table of Contents

1. [High-Level Summary](#high-level-summary)
2. [Overview (Technical)](#overview-technical)
3. [Why It Matters (Business)](#why-it-matters-business)
4. [Prerequisites](#prerequisites)
5. [Installation](#installation)
6. [Demo Code](#demo-code)
7. [Step-by-Step Explanation](#step-by-step-explanation)
8. [Running the Demo](#running-the-demo)
9. [Contact](#contact)

---

## High-Level Summary

* **Data Availability (DA) Layer:** Think of DA as a secure warehouse where you store proof that your data exists on the blockchain, without bloating the main ledger.
* **Purpose** Show how easy it is to save and retrieve any small piece of data (a “blob”) on Syscoin’s DA layer, using Rust.
* **Key Benefits for Non-Technical Stakeholders:**

    * **Reliability:** Data stored on PoDA (Proof-of-Data-Availability) remains accessible and verifiable.
    * **Scalability:** Keeps the main blockchain lean while offloading large or infrequently accessed data.
    * **Security:** Data integrity is guaranteed by the underlying blockchain consensus.

---

## Overview (Technical)

`bitcoin_da_client` is a Rust library that provides an ergonomic, async interface to:

* Connect to a Syscoin node via JSON-RPC
* Manage wallets
* Query balances
* Store and retrieve arbitrary data blobs on the DA layer (PoDA)
* Perform raw HTTP GET requests against PoDA endpoints

This demo bundles all core operations into a single `main.rs` example.

---

## Why It Matters (Business)

1. **Off-chain Storage with On-chain Integrity**
   Large files, audit logs, or proofs can live off the main chain, while still retaining a tamper-proof anchor on Syscoin.

2. **Cost Efficiency**
   On-chain transactions can be expensive and slow for large data. DA lets you pay only for proofs, not full data storage.

3. **Instant Verifiability**
   Any stakeholder, auditors, partners, or end users can verify data existence with a simple hash lookup, without running a full node.

---

## Prerequisites

* **Rust & Cargo** installed (rustc ≥ 1.60)
* A **running Syscoin node** with RPC enabled
* Access to a **PoDA gateway** URL (e.g. `http://poda.tanenbaum.io/ for testnet)

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
bitcoin_da_client = "0.1.8"
tokio             = { version = "1", features = ["full"] }
reqwest           = "0.12"
serde             = "1.0"
serde_json        = "1.0"
hex               = "0.4"
bitcoincore-rpc   = "0.19"
jsonrpc           = "0.18"
async-trait       = "0.1"
```

---

## Demo Code

Save as `src/main.rs`:

```rust
use syscoin_client::SyscoinClient;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let rpc_url      = "http://localhost:18370";
    let rpc_user     = "your_rpc_user";
    let rpc_password = "your_rpc_password";
    let poda_url     = "http://poda.example.com";

    // Initialize client
    println!("Initializing SyscoinClient...");
    let client = SyscoinClient::new(
        rpc_url,
        rpc_user,
        rpc_password,
        poda_url,
        Some(Duration::from_secs(30)),
    )?;
    println!("Client ready.\n");

    // Wallet management
    let wallet_name = "demo_wallet";
    println!("Loading wallet \"{}\"...", wallet_name);
    client.create_or_load_wallet(wallet_name).await?;
    println!("Wallet \"{}\" is active.\n", wallet_name);

    // Balance retrieval
    println!("Retrieving balance...");
    let balance = client.get_balance().await?;
    println!("→ Current UTXO balance: {} SYSC\n", balance);

    // Blob creation
    let payload = b"Hello from the bitcoin_da_client demo!";
    println!("Uploading blob of {} bytes...", payload.len());
    let version_hash = client.create_blob(payload).await?;
    println!("→ Blob stored; version hash = {}\n", version_hash);

    // Blob retrieval
    println!("Downloading blob for hash {}...", version_hash);
    let downloaded = client.get_blob_from_cloud(&version_hash).await?;
    println!("→ Downloaded {} bytes: \"{}\"\n",
             downloaded.len(),
             String::from_utf8_lossy(&downloaded)
    );


    println!("Demo complete! 🎉");
    Ok(())
}
```

---

## Step-by-Step Explanation

1. **Initialization** – Creates a `SyscoinClient` pointing at your RPC node and PoDA gateway with a 30-second timeout.
2. **Wallet Management** – Opens or creates the wallet named "demo\_wallet".
3. **Balance Retrieval** – Returns your UTXO balance in SYSC.
4. **Blob Creation** – Uploads your byte payload and returns a version hash.
5. **Blob Retrieval** – Downloads the exact bytes you uploaded.

---

## Running the Demo

1. Update credentials and PoDA URL in `main.rs`.
2. Ensure your Syscoin node is running with port 18370 opened.
3. From the project directory, run:

   ```bash
   cargo run --release
   ```
4. Observe the console output for each step.

---


---

## Contact

Maintainer: Abdul ([abdul@syscoin.org](mailto:abdul@syscoin.org))
GitHub: [https://github.com/syscoin/bitcoin\_da\_client](https://github.com/syscoin/bitcoin_da_client)

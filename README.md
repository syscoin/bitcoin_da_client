# BitcoinDA Rust Client

`bitcoin_da_client` is a Rust library designed to provide a seamless interface for interacting with the BitcoinDA data availability layer of the Syscoin blockchain. 
It leverages asynchronous programming to handle RPC calls efficiently, making it suitable for modern applications that require reliable and performant Data Availability.

## Features

- **RPC Client Integration:** Connect to a Syscoin node using RPC to perform various blockchain operations.
- **Wallet Management:** Create or load wallets to manage your Syscoin assets.
- **Balance Retrieval:** Fetch the balance of your Syscoin account with optional parameters for more refined queries.
- **Blob Operations:** Create blobs from data and retrieve blobs from the cloud using version hashes.
- **HTTP Requests:** Perform HTTP GET requests as needed for extended functionalities.

## Installation

Add `bitcoin_da_client` to your `Cargo.toml`:

```rust
[dependencies]
bitcoin_da_client = "0.1.8"
```

Ensure that you have the following dependencies in your project, as `syscoin_client` relies on them:

- `serde_json = "1.0.133"`
- `mockito = "1.6.1"`
- `reqwest = "0.12.12"`
- `serde = "1.0.215"`
- `tokio = "1.42.0"`
- `hex = "0.4.3"`
- `bitcoincore-rpc = "0.19.0"`
- `jsonrpc = "0.18.0"`
- `async-trait = "0.1.83"`

## Usage

### Creating a BitcoinDA Client

To interact with the Syscoin blockchain, initialize a new instance of `SyscoinClient`:

```rust
use syscoin_client::SyscoinClient;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc_url = "http://localhost:18370/wallet";
    let rpc_user = "your_rpc_user";
    let rpc_password = "your_rpc_password";
    let poda_url = "http://poda.example.com";

    let client = SyscoinClient::new(
        rpc_url,
        rpc_user,
        rpc_password,
        poda_url,
        Some(Duration::from_secs(30)),
    )?;

    // Your code here

    Ok(())
}
```

### Fetching Account Balance

Retrieve the balance of your Syscoin UTXO account:

```rust
let balance = client.get_balance().await?;
println!("Account Balance: {} SYSC", balance);
```

### Creating a Blob

Save Blob data in BitcoinDA:

```rust
let data = b"Sample data to be stored as a blob";
let blob = client.create_blob(data).await?;
println!("Created Blob: {}", blob);
```

### Retrieving a Blob from the Cloud

Fetch a blob using its version hash:

```rust
let version_hash = "your_version_hash_here";
let blob_data = client.get_blob_from_cloud(version_hash).await?;
println!("Retrieved Blob Data: {:?}", blob_data);
```

### Wallet Management

Create or load a wallet:

```rust
client.create_or_load_wallet("my_wallet").await?;
println!("Wallet created or loaded successfully.");
```

## Example

Here is a complete example demonstrating how to use the `syscoin_client` library:

```rust
use syscoin_client::SyscoinClient;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc_url = "http://localhost:8332";
    let rpc_user = "your_rpc_user";
    let rpc_password = "your_rpc_password";
    let poda_url = "http://poda.example.com";

    let client = SyscoinClient::new(
        rpc_url,
        rpc_user,
        rpc_password,
        poda_url,
        Some(Duration::from_secs(30)),
    )?;

    // Create or load a wallet
    client.create_or_load_wallet("my_wallet").await?;
    println!("Wallet is ready.");

    // Get account balance
    let balance = client.get_balance().await?;
    println!("Account Balance: {} SYSC", balance);

    // Create a blob from data
    let data = b"Hello, Syscoin!";
    let blob = client.create_blob(data).await?;
    println!("Created Blob: {}", blob);

    // Retrieve the blob from the cloud
    let retrieved_data = client.get_blob_from_cloud(&blob).await?;
    println!("Retrieved Blob Data: {:?}", retrieved_data);

    Ok(())
}
```

## Contributing

Contributions are welcome! Please open an issue or submit a pull request for any enhancements or bug fixes.

## License



## Contact

For any inquiries or feedback, please reach out to the maintainer at [sidhujag@syscoin.org](mailto:sidhujag@syscoin.org).

## Support

If you encounter any issues or have questions, feel free to open an issue on the [GitHub repository](https://github.com/syscoin/bitcoin_da_client) or contact the maintainer directly.

---

---

© 2024 Syscoin Client Maintainers
#[cfg(test)]
mod tests {
    use bitcoin_da_client::{BitcoinDaFinalityMode, SyscoinClient};
    use hex;
    use mockito::Server;
    use serde_json::json;
    use tokio;

    #[tokio::test]
    async fn test_syscoin_client_creation() {
        let timeout = Some(std::time::Duration::from_secs(30));
        let result = SyscoinClient::new(
            "http://localhost:8888",
            "user",
            "password",
            "http://poda.example.com",
            timeout,
            "test_wallet",
        );
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_balance() {
        // Create the mock server in a separate thread
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let expected_balance = 100.5;

        let mock_response = json!({
            "result": expected_balance,
            "error": null
        });

        // Set up mock response
        let _m = mock_server
            .mock("POST", "/wallet/test_wallet")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let balance = client.get_balance().await;

        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), expected_balance);
    }

    #[tokio::test]
    async fn test_create_blob() {
        // Create the mock server in a separate thread
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");
        let expected_hash = "deadbeef";

        // Mock RPC response
        let mock_response = json!({
            "result": {
                "versionhash": expected_hash
            },
            "error": null,
            "id": 1
        });

        let _m = mock_server
            .mock("POST", "/wallet/test_wallet")
            .match_body(mockito::Matcher::JsonString(
                r#"{"jsonrpc":"2.0","id":1,"method":"syscoincreatenevmblob","params":["01020304",false,"blake2s"]}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.create_blob(&[1, 2, 3, 4]).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_hash);
    }

    #[tokio::test]
    async fn test_get_blob_from_cloud() {
        // Create the mock server in a separate thread
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let expected_data = b"retrieved data".to_vec();
        let version_hash = "deadbeef";

        // Mock HTTP GET response
        let _m = mock_server
            .mock("GET", format!("/vh/{}", version_hash).as_str())
            .with_status(200)
            .with_body(&expected_data)
            .create();

        let client = SyscoinClient::new(
            "http://localhost:8888", // RPC URL (won't be used)
            "user",                  // Username
            "password",              // Password
            &mock_server.url(),      // PODA cloud URL
            None,                    // Timeout
            "test_wallet",
        )
        .unwrap();

        // Use get_blob with a non-existent RPC server to force fallback to cloud
        // First make sure RPC will fail by mocking it to return an error
        mock_server
            .mock("POST", "/")
            .with_status(500)
            .with_body("RPC error")
            .create();

        // Then call get_blob which should fall back to the cloud endpoint
        let result = client.get_blob(version_hash).await;

        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert_eq!(result.unwrap(), expected_data);
    }

    #[tokio::test]
    async fn test_get_blob_from_cloud_uses_vh() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let expected_data = Vec::new();
        let version_hash = "deadbeef";

        mock_server
            .mock("GET", format!("/vh/{}", version_hash).as_str())
            .with_status(200)
            .with_body(&expected_data)
            .create();

        let client = SyscoinClient::new(
            "http://localhost:8888",
            "user",
            "password",
            &mock_server.url(),
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.get_blob_from_cloud(version_hash).await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert_eq!(result.unwrap(), expected_data);
    }

    #[tokio::test]
    async fn test_blob_exists_uses_metadata_only_rpc() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "deadbeef";
        let mock_response = json!([
            {
                "jsonrpc": "2.0",
                "id": 0,
                "result": {
                    "versionhash": blob_id
                },
                "error": null
            }
        ]);

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"[{"jsonrpc":"2.0","id":0,"method":"getnevmblobdata","params":["deadbeef",false]}]"#
                    .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &mock_server.url(),
            None,
            "test_wallet",
        )
        .unwrap();

        assert!(client.blob_exists("0xdeadbeef").await.unwrap());
    }

    #[tokio::test]
    async fn test_blob_exists_cloud_fallback_uses_check_vh() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "deadbeef";
        let not_found_response = json!([
            {
                "jsonrpc": "2.0",
                "id": 0,
                "result": null,
                "error": {
                    "code": -32602,
                    "message": format!("Could not find blob information for versionhash {}", blob_id)
                }
            }
        ]);

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(not_found_response.to_string())
            .create();

        mock_server
            .mock("POST", "/check_vh")
            .match_body(mockito::Matcher::JsonString(format!(r#"["{blob_id}"]"#)))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([true]).to_string())
            .create();

        let poda_url = format!("{}/vh", mock_server.url());
        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &poda_url,
            None,
            "test_wallet",
        )
        .unwrap();

        assert!(client.blob_exists(blob_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_blobs_exist_batches_rpc_and_check_vh_fallback() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!([
                    {
                        "jsonrpc": "2.0",
                        "id": 0,
                        "result": {
                            "versionhash": "aaa"
                        },
                        "error": null
                    },
                    {
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": null,
                        "error": {
                            "code": -32602,
                            "message": "Could not find blob information for versionhash bbb"
                        }
                    }
                ])
                .to_string(),
            )
            .expect(1)
            .create();

        mock_server
            .mock("POST", "/check_vh")
            .match_body(mockito::Matcher::JsonString(r#"["bbb"]"#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([true]).to_string())
            .expect(1)
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &mock_server.url(),
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.blobs_exist(["0xaaa", "bbb"]).await.unwrap();
        assert_eq!(result, vec![true, true]);
    }

    #[tokio::test]
    async fn test_blobs_exist_accepts_aggregate_check_vh_response() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!([
                    {
                        "jsonrpc": "2.0",
                        "id": 0,
                        "result": null,
                        "error": {
                            "code": -32602,
                            "message": "Could not find blob information for versionhash aaa"
                        }
                    },
                    {
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": null,
                        "error": {
                            "code": -32602,
                            "message": "Could not find blob information for versionhash bbb"
                        }
                    }
                ])
                .to_string(),
            )
            .expect(1)
            .create();

        mock_server
            .mock("POST", "/check_vh")
            .match_body(mockito::Matcher::JsonString(r#"["aaa","bbb"]"#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!(false).to_string())
            .expect(1)
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &mock_server.url(),
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.blobs_exist(["aaa", "bbb"]).await.unwrap();
        assert_eq!(result, vec![false, false]);
    }

    #[tokio::test]
    async fn test_blobs_exist_rejects_more_than_32_hashes() {
        let client = SyscoinClient::new(
            "http://localhost:8888",
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let hashes: Vec<_> = (0..33).map(|idx| format!("{idx:064x}")).collect();
        let result = client.blobs_exist(hashes.iter()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_or_load_wallet() {
        // Create the mock server in a separate thread
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");
        let wallet_name = "test_wallet";

        // Mock successful wallet creation response
        let mock_response = json!({
            "result": {},
            "error": null,
            "id": 1
        });

        let _m = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.create_or_load_wallet(wallet_name).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_error_handling() {
        // Create the mock server in a separate thread
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        // Mock error response
        let mock_response = json!({
            "result": {},
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        });

        let _m = mock_server
            .mock("POST", "/")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.get_balance().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_request_invalid_json() {
        // Create the mock server in a separate thread
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let _m = mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_body("Not a JSON")
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();
        let result = client.create_blob(&[1, 2, 3, 4]).await;
        println!("Result: {:?}", result);
        // Expect an error because the response body is not valid JSON.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_blob() {
        use hex::encode;

        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let expected_data = b"hello world blob data".to_vec();
        let hex_data = encode(&expected_data);
        let blob_id = "deadbeef123";

        // Mock the RPC endpoint
        let mock_response = json!({
            "result": {
                "data": hex_data
            },
            "error": null,
            "id": 1
        });

        // Mock the JSON-RPC POST request
        mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        // ALSO mock the fallback cloud GET endpoint
        // The url format should match what's in get_blob_from_cloud
        mock_server
            .mock("GET", format!("/{}", blob_id).as_str())
            .with_status(200)
            .with_body(&expected_data)
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &mock_server.url(), // Same server for both
            None,
            "test_wallet",
        )
        .unwrap();

        // Add very detailed debug info
        println!("Server URL: {}", &mock_server.url());
        println!("Blob ID: {}", blob_id);

        let result = client.get_blob(blob_id).await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert_eq!(result.unwrap(), expected_data);
    }

    #[tokio::test]
    async fn test_check_blob_finality_true() {
        // Create mock server
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "deadbeef";

        // Mock a finalized blob response
        let mock_response = json!({
            "result": {
                "chainlock": true
            },
            "error": null,
            "id": 1
        });

        mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.check_blob_finality(blob_id).await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(result.unwrap(), "Expected blob to be final");
    }

    #[tokio::test]
    async fn test_check_blob_finality_false() {
        // Create mock server
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "deadbeef";

        // Mock a non-finalized blob response
        let mock_response = json!({
            "result": {
                "chainlock": false
            },
            "error": null,
            "id": 1
        });

        mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.check_blob_finality(blob_id).await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(!result.unwrap(), "Expected blob to not be final");
    }

    #[tokio::test]
    async fn test_check_blob_finality_with_0x_prefix() {
        // Create mock server
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "0xdeadbeef"; // Has 0x prefix

        // Mock a finalized blob response
        let mock_response = json!({
            "result": {
                "chainlock": true
            },
            "error": null,
            "id": 1
        });

        // Verify the request was made with the correct parameters
        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"jsonrpc":"2.0","id":1,"method":"getnevmblobdata","params":["deadbeef"]}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.check_blob_finality(blob_id).await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_check_blob_finality_error() {
        // Create mock server
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "invalid";

        // Mock an error response
        let mock_response = json!({
            "result": null,
            "error": {
                "code": -5,
                "message": "Blob not found"
            },
            "id": 1
        });

        mock_server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_response.to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.check_blob_finality(blob_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_check_blob_finality_falls_back_to_cloud() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "feedbeef";
        let not_found_response = json!({
            "result": null,
            "error": {
                "code": -32602,
                "message": format!("Could not find blob information for versionhash {}", blob_id)
            },
            "id": 1
        });

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(not_found_response.to_string())
            .create();

        mock_server
            .mock("POST", "/check_vh")
            .match_body(mockito::Matcher::JsonString(format!(r#"["{blob_id}"]"#)))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([true]).to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &mock_server.url(),
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client.check_blob_finality(blob_id).await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(
            result.unwrap(),
            "Expected PODA fallback to mark blob as final"
        );
    }

    #[tokio::test]
    async fn test_check_blob_finality_by_confirmations_true() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "feedbeef";

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "result": {
                        "versionhash": blob_id,
                        "height": 100
                    },
                    "error": null,
                    "id": 1
                })
                .to_string(),
            )
            .create();

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getblockcount".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": 104, "error": null, "id": 1}).to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client
            .check_blob_finality_by_confirmations(blob_id, 5)
            .await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(
            result.unwrap(),
            "Expected blob to satisfy confirmation threshold"
        );
    }

    #[tokio::test]
    async fn test_check_blob_finality_by_confirmations_false() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "feedbeef";

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "result": {
                        "versionhash": blob_id,
                        "height": 100
                    },
                    "error": null,
                    "id": 1
                })
                .to_string(),
            )
            .create();

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getblockcount".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": 103, "error": null, "id": 1}).to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client
            .check_blob_finality_by_confirmations(blob_id, 5)
            .await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(
            !result.unwrap(),
            "Expected blob to be below confirmation threshold"
        );
    }

    #[tokio::test]
    async fn test_check_blob_finality_by_confirmations_missing_height_is_not_final() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "feedbeef";

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "result": {
                        "versionhash": blob_id,
                        "txid": "abc123"
                    },
                    "error": null,
                    "id": 1
                })
                .to_string(),
            )
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client
            .check_blob_finality_by_confirmations(blob_id, 5)
            .await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(
            !result.unwrap(),
            "Expected missing height to be treated as not final"
        );
    }

    #[tokio::test]
    async fn test_check_blob_finality_by_confirmations_falls_back_to_cloud() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "feedbeef";
        let not_found_response = json!({
            "result": null,
            "error": {
                "code": -32602,
                "message": format!("Could not find blob information for versionhash {}", blob_id)
            },
            "id": 1
        });

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(not_found_response.to_string())
            .create();

        mock_server
            .mock("POST", "/check_vh")
            .match_body(mockito::Matcher::JsonString(format!(r#"["{blob_id}"]"#)))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!([true]).to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            &mock_server.url(),
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client
            .check_blob_finality_by_confirmations(blob_id, 5)
            .await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(
            result.unwrap(),
            "Expected PODA fallback to mark blob as final"
        );
    }

    #[tokio::test]
    async fn test_check_blob_finality_with_mode_confirmations_dispatches() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        let blob_id = "feedbeef";

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getnevmblobdata".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "result": {
                        "versionhash": blob_id,
                        "height": 100
                    },
                    "error": null,
                    "id": 1
                })
                .to_string(),
            )
            .create();

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getblockcount".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"result": 104, "error": null, "id": 1}).to_string())
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let result = client
            .check_blob_finality_with_mode(blob_id, BitcoinDaFinalityMode::Confirmations, 5)
            .await;
        assert!(result.is_ok(), "Error: {:?}", result.err());
        assert!(
            result.unwrap(),
            "Expected confirmation mode finality check to pass"
        );
    }

    #[tokio::test]
    async fn test_get_blob_base_fee_uses_network_minimum() {
        let mut mock_server = std::thread::spawn(|| Server::new())
            .join()
            .expect("Failed to create mock server");

        // Low smart fee alone would yield ceil(100/1000 * 0.01) = 1 sat per blob-byte;
        // mempool minimum is higher so we assert the client takes max(estimate, mempool, relay).
        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("estimatesmartfee".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "result": { "feerate": 0.000001, "blocks": 6 },
                    "error": null,
                    "id": 1
                })
                .to_string(),
            )
            .create();

        mock_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("getmempoolinfo".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "result": { "mempoolminfee": 0.002, "minrelaytxfee": 0.000015 },
                    "error": null,
                    "id": 1
                })
                .to_string(),
            )
            .create();

        let client = SyscoinClient::new(
            &mock_server.url(),
            "user",
            "password",
            "http://poda.example.com",
            None,
            "test_wallet",
        )
        .unwrap();

        let fee = client.get_blob_base_fee(6).await.unwrap();
        // 0.002 SYS/kvb -> 200_000 sat/kvb -> ceil(200_000/1000 * 0.01) = 2
        assert_eq!(fee, 2);
    }
}

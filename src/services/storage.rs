use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

// Port of storage.ts. Object storage abstraction (P1-010/P2-010) — an
// interface rather than a concrete R2 client so tests run fully offline
// against InMemoryStorage instead of needing real Cloudflare
// credentials, mirroring the PaymentProvider/AIProvider/MeetingProvider
// injection pattern already established in R7/R8.

#[derive(Debug, thiserror::Error)]
#[error("storage error: {0}")]
pub struct StorageError(pub String);

pub struct GetObjectResult {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[async_trait::async_trait]
pub trait AssetStorage: Send + Sync {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), StorageError>;
    // P6-002 (ADR-0010) — speaking evaluation needs actual audio bytes
    // server-side, not just a URL a client could fetch.
    async fn get(&self, key: &str) -> Result<GetObjectResult, StorageError>;
    async fn signed_url(&self, key: &str, expires_in_seconds: u64) -> Result<String, StorageError>;
    // P2-010 — a presigned PUT URL the client uploads directly to,
    // bypassing this backend entirely for the (potentially large) file
    // bytes themselves.
    async fn presigned_put_url(&self, key: &str, content_type: &str, expires_in_seconds: u64) -> Result<String, StorageError>;
    // P2-010 — verifies an object actually landed at `key` before the
    // backend trusts the client's "I uploaded it" claim.
    async fn exists(&self, key: &str) -> Result<bool, StorageError>;
}

// Real Cloudflare R2 client (S3-compatible). R2 recommends path-style
// addressing when using the default *.r2.cloudflarestorage.com endpoint
// (no custom domain configured).
pub struct R2Storage {
    client: Client,
    bucket: String,
    // Every key is written under this prefix — the bucket is shared with
    // an older, unrelated project, so titian's objects stay namespaced
    // away from whatever's already in there.
    key_prefix: &'static str,
}

impl R2Storage {
    pub fn new(account_id: &str, access_key_id: &str, secret_access_key: &str, bucket: String) -> Self {
        let credentials = Credentials::new(access_key_id, secret_access_key, None, None, "r2-static");
        let config = aws_sdk_s3::Config::builder()
            .region(Region::new("auto"))
            .endpoint_url(format!("https://{account_id}.r2.cloudflarestorage.com"))
            .credentials_provider(credentials)
            .force_path_style(true)
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .build();
        Self { client: Client::from_conf(config), bucket, key_prefix: "titian/assets" }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}/{key}", self.key_prefix)
    }
}

#[async_trait::async_trait]
impl AssetStorage for R2Storage {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), StorageError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .body(ByteStream::from(bytes))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<GetObjectResult, StorageError> {
        let result = self.client.get_object().bucket(&self.bucket).key(self.full_key(key)).send().await.map_err(|e| StorageError(e.to_string()))?;
        let content_type = result.content_type().unwrap_or("application/octet-stream").to_string();
        let bytes = result.body.collect().await.map_err(|e| StorageError(e.to_string()))?.to_vec();
        if bytes.is_empty() {
            return Err(StorageError("empty object body".to_string()));
        }
        Ok(GetObjectResult { bytes, content_type })
    }

    async fn signed_url(&self, key: &str, expires_in_seconds: u64) -> Result<String, StorageError> {
        let presigning_config = PresigningConfig::expires_in(Duration::from_secs(expires_in_seconds)).map_err(|e| StorageError(e.to_string()))?;
        let presigned = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .presigned(presigning_config)
            .await
            .map_err(|e| StorageError(e.to_string()))?;
        Ok(presigned.uri().to_string())
    }

    async fn presigned_put_url(&self, key: &str, content_type: &str, expires_in_seconds: u64) -> Result<String, StorageError> {
        let presigning_config = PresigningConfig::expires_in(Duration::from_secs(expires_in_seconds)).map_err(|e| StorageError(e.to_string()))?;
        let presigned = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .content_type(content_type)
            .presigned(presigning_config)
            .await
            .map_err(|e| StorageError(e.to_string()))?;
        Ok(presigned.uri().to_string())
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        match self.client.head_object().bucket(&self.bucket).key(self.full_key(key)).send().await {
            Ok(_) => Ok(true),
            Err(e) => {
                if let aws_sdk_s3::error::SdkError::ServiceError(service_err) = &e {
                    if service_err.err().is_not_found() {
                        return Ok(false);
                    }
                }
                if let Some(raw) = e.raw_response() {
                    if raw.status().as_u16() == 404 {
                        return Ok(false);
                    }
                }
                Err(StorageError(e.to_string()))
            }
        }
    }
}

// Test double — keeps bytes in memory, never touches the network. Used
// by every tests/*_test.rs helper instead of R2Storage, same as
// FakeAIProvider/StubMeetingProvider/StubQrisProvider. Note: unlike
// R2Storage, keys here are NOT namespaced under "titian/assets/" — the
// raw key is used directly, matching the Bun InMemoryStorage exactly.
#[derive(Default)]
pub struct InMemoryStorage {
    objects: Mutex<HashMap<String, GetObjectResultClone>>,
}

#[derive(Clone)]
struct GetObjectResultClone {
    bytes: Vec<u8>,
    content_type: String,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl AssetStorage for InMemoryStorage {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), StorageError> {
        self.objects.lock().unwrap().insert(key.to_string(), GetObjectResultClone { bytes, content_type: content_type.to_string() });
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<GetObjectResult, StorageError> {
        let objects = self.objects.lock().unwrap();
        let object = objects.get(key).ok_or_else(|| StorageError(format!("object not found: {key}")))?;
        Ok(GetObjectResult { bytes: object.bytes.clone(), content_type: object.content_type.clone() })
    }

    async fn signed_url(&self, key: &str, _expires_in_seconds: u64) -> Result<String, StorageError> {
        Ok(format!("https://fake-storage.test/{key}?signed=1"))
    }

    async fn presigned_put_url(&self, key: &str, _content_type: &str, _expires_in_seconds: u64) -> Result<String, StorageError> {
        Ok(format!("https://fake-storage.test/{key}?presigned-put=1"))
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.objects.lock().unwrap().contains_key(key))
    }
}

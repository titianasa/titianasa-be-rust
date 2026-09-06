use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use uuid::Uuid;

use crate::errors::AppError;

pub struct CreatePaymentResult {
    pub payment_id: String,
    pub qris_payload: String,
}

#[async_trait::async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn create_payment(&self, order_id: Uuid, amount_idr: i64) -> Result<CreatePaymentResult, AppError>;
}

// P9-007 — no real payment gateway integrated. The ONLY implementation;
// when a real gateway is integrated, a 2nd struct implementing this same
// trait plugs in without touching order.rs.
pub struct StubQrisProvider;

#[async_trait::async_trait]
impl PaymentProvider for StubQrisProvider {
    async fn create_payment(&self, order_id: Uuid, amount_idr: i64) -> Result<CreatePaymentResult, AppError> {
        let mut bytes = [0u8; 9];
        rand::thread_rng().fill_bytes(&mut bytes);
        let payment_id = format!("stub_{}", URL_SAFE_NO_PAD.encode(bytes));
        let qris_payload = format!("STUB-QRIS|order={order_id}|amount={amount_idr}|payment={payment_id}");
        Ok(CreatePaymentResult { payment_id, qris_payload })
    }
}

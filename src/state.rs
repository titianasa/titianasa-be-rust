use std::sync::Arc;

use anyhow::Context;
use sqlx::PgPool;

use crate::config::Config;
use crate::services::ai_provider::AIProvider;
use crate::services::canvas_hub::CanvasHub;
use crate::services::google_oauth::GoogleTokenVerifier;
use crate::services::meeting_provider::MeetingProvider;
use crate::services::payment_provider::PaymentProvider;
use crate::services::storage::AssetStorage;

// Mirrors parelabs-backend's AppState: a plain struct (not Clone) always
// held behind an Arc, so every handler takes `State<Arc<AppState>>`.
pub struct AppState {
    pub db: PgPool,
    pub redis: redis::Client,
    pub config: Config,
    pub google_verifier: GoogleTokenVerifier,
    pub payment_provider: Arc<dyn PaymentProvider>,
    pub ai_provider: Arc<dyn AIProvider>,
    pub meeting_provider: Arc<dyn MeetingProvider>,
    pub storage: Arc<dyn AssetStorage>,
    pub canvas_hub: Arc<CanvasHub>,
}

impl AppState {
    pub async fn init(config: Config) -> anyhow::Result<Self> {
        let db = crate::db::connect(&config.database_url).await?;
        // P1-012 deferral closed — Client::open is sync (parses the URL,
        // no network I/O), so this can't itself fail to boot the app on
        // a momentarily-unreachable Redis; individual connections are
        // opened per-use (see services/health.rs) and reconnect freely.
        let redis = redis::Client::open(config.redis_url.as_str()).context("invalid REDIS_URL")?;

        // Same "read directly from process env, not a Config field"
        // convention as titian-backend-bun's index.ts uses for R2/
        // OpenRouter credentials — these are process-level secrets, not
        // per-org/per-tutor DB rows.
        let openrouter_api_key = std::env::var("OPENROUTER_API_KEY").context("OPENROUTER_API_KEY is required")?;
        let ai_provider: Arc<dyn AIProvider> = Arc::new(crate::services::ai_provider::DeepSeekProvider::new(openrouter_api_key));

        // GoogleMeetProvider only when all 3 vars are set (a real
        // organizer account's credentials); StubMeetingProvider
        // otherwise — same "real provider only if configured, optional
        // instead of required" shape as OPENROUTER_API_KEY above.
        let meeting_provider: Arc<dyn MeetingProvider> = match (
            std::env::var("GOOGLE_MEET_CLIENT_ID"),
            std::env::var("GOOGLE_MEET_CLIENT_SECRET"),
            std::env::var("GOOGLE_MEET_REFRESH_TOKEN"),
        ) {
            (Ok(client_id), Ok(client_secret), Ok(refresh_token)) => {
                Arc::new(crate::services::meeting_provider::GoogleMeetProvider::new(client_id, client_secret, refresh_token))
            }
            _ => Arc::new(crate::services::meeting_provider::StubMeetingProvider),
        };

        // Same "read directly from process env, required, not a Config
        // field" convention as OPENROUTER_API_KEY above — R2Storage is
        // the only implementation used in production too (P1-010).
        let r2_account_id = std::env::var("R2_ACCOUNT_ID").context("R2_ACCOUNT_ID is required")?;
        let r2_access_key_id = std::env::var("R2_ACCESS_KEY_ID").context("R2_ACCESS_KEY_ID is required")?;
        let r2_secret_access_key = std::env::var("R2_SECRET_ACCESS_KEY").context("R2_SECRET_ACCESS_KEY is required")?;
        let r2_bucket = std::env::var("R2_BUCKET").context("R2_BUCKET is required")?;
        let storage: Arc<dyn AssetStorage> = Arc::new(crate::services::storage::R2Storage::new(&r2_account_id, &r2_access_key_id, &r2_secret_access_key, r2_bucket));

        Ok(Self {
            db,
            redis,
            google_verifier: GoogleTokenVerifier::new(),
            payment_provider: Arc::new(crate::services::payment_provider::StubQrisProvider),
            ai_provider,
            meeting_provider,
            storage,
            canvas_hub: Arc::new(CanvasHub::new()),
            config,
        })
    }
}

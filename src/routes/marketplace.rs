use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// Two routes here are deliberately PUBLIC (no auth_middleware): the
// certificate verify endpoint (anyone with the code can verify) and the
// payment webhook (a real gateway wouldn't carry a user's bearer token).
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/certificates/{code}/verify", get(handlers::certificate::get_verify))
        .route("/payments/{payment_id}/webhook", post(handlers::order::post_webhook))
        .route("/tutors/{id}/reputation", get(handlers::tutor_review::get_reputation))
}

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tutors/me/products", post(handlers::learning_product::post_product))
        .route("/tutors/{id}/products", get(handlers::learning_product::get_tutor_products))
        .route("/products/{id}", get(handlers::learning_product::get_product))
        .route("/products/{id}/publish", post(handlers::learning_product::post_publish))
        .route("/products", get(handlers::learning_product::get_products))
        .route(
            "/products/{id}/cohorts",
            post(handlers::cohort::post_cohort).get(handlers::cohort::get_cohorts),
        )
        .route("/cohorts/{id}/enrollments", post(handlers::enrollment::post_enrollment))
        .route("/cohorts/{id}/students", get(handlers::enrollment::get_students))
        .route("/cohorts/{id}/enrollments/{enrollment_id}/complete", post(handlers::enrollment::post_complete))
        .route("/cohorts/{id}/enrollments/{enrollment_id}/tutor-cancel", post(handlers::enrollment::post_tutor_cancel))
        .route("/cohorts/{id}/enrollments/{enrollment_id}/certificate", post(handlers::certificate::post_certificate))
        .route("/me/enrollments", get(handlers::enrollment::get_my_enrollments))
        .route("/enrollments/{id}/certificate", get(handlers::certificate::get_certificate_for_enrollment))
        .route("/enrollments/{id}/checkout", post(handlers::order::post_checkout))
        .route("/enrollments/{id}/cancel", post(handlers::enrollment::post_cancel))
        .route("/tutors/me/wallet", get(handlers::order::get_wallet))
        .route("/cohorts/{id}/assignments", post(handlers::assignment::post_assignment).get(handlers::assignment::get_assignments))
        .route("/assignments/{id}/submissions", post(handlers::assignment::post_submission).get(handlers::assignment::get_submissions))
        .route("/submissions/{id}/grade", post(handlers::assignment::post_grade))
        .route("/cohorts/{id}/gradebook", get(handlers::assignment::get_gradebook))
        .route("/tutors/{id}/reviews", post(handlers::tutor_review::post_review))
}

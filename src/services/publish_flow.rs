use crate::errors::AppError;

// Port of publish_flow.ts. draft -> in_review -> published state-machine
// rules shared by questions and lessons — both tables use the identical
// status enum (draft/in_review/published/archived, see the CHECK
// constraints in schema.ts), transition rules don't differ per entity.
pub fn validate_submit_for_review(current_status: &str) -> Result<(), AppError> {
    if current_status != "draft" {
        return Err(AppError::UnprocessableEntity(
            "invalid_status_transition",
            format!(r#"cannot submit for review from status "{current_status}" — must be "draft""#),
        ));
    }
    Ok(())
}

pub fn validate_publish(current_status: &str) -> Result<(), AppError> {
    if current_status != "in_review" {
        return Err(AppError::UnprocessableEntity(
            "invalid_status_transition",
            format!(r#"cannot publish from status "{current_status}" — must be "in_review""#),
        ));
    }
    Ok(())
}

pub fn validate_reject(current_status: &str) -> Result<(), AppError> {
    if current_status != "in_review" {
        return Err(AppError::UnprocessableEntity(
            "invalid_status_transition",
            format!(r#"cannot reject from status "{current_status}" — must be "in_review""#),
        ));
    }
    Ok(())
}

// One test binary for every integration test, instead of one per file.
//
// Cargo turns each top-level file in tests/ into its own executable, and
// each of those statically links this whole crate plus every dependency
// (~140 MB apiece). With twenty-odd files that was several GB rebuilt on
// every `cargo test`, and it filled the disk badly enough to take
// Postgres down mid-run. Files in a subdirectory are NOT auto-discovered
// as separate targets, so gathering them here as modules links once.
//
// Nothing about the tests themselves changes: each file is still its own
// module with its own imports and helpers, and `#[sqlx::test]` still
// gives every test its own database. Run one file's tests with
//   cargo test --test integration auth_test::
//
// A new integration test file goes in this directory AND needs a `mod`
// line below — a file left out of this list silently never runs.

mod admin_audit_test;
mod admin_metrics_test;
mod admin_participants_test;
mod ai_content_test;
mod ai_gateway_test;
mod ai_settings_test;
mod ai_retry_test;
mod attempt_submission_test;
mod attendance_test;
mod auth_test;
mod client_events_test;
mod content_factory_test;
mod drive_test;
mod economy_test;
mod gamification_test;
mod job_queue_test;
mod learning_event_test;
mod learning_test;
mod live_chat_test;
mod marketplace_test;
mod messaging_test;
mod metrics_rollup_test;
mod module_item_versions_test;
mod organization_test;
mod proctoring_test;
mod question_test;
mod quiz_batch_generation_test;
mod quiz_convert_group_type_test;
mod quiz_draft_gate_test;
mod quiz_generation_concurrency_test;
mod quiz_learning_events_test;
mod quiz_max_attempts_test;
mod quiz_paper_test;
mod quiz_parse_raw_test;
mod quiz_suggest_group_types_test;
mod section_checkpoint_test;
mod user_data_consent_test;

/// The guard for the comment above: a `*_test.rs` dropped into this
/// directory without a matching `mod` line compiles fine and runs zero
/// tests, which looks exactly like passing. This makes it fail instead.
#[test]
fn every_test_file_in_this_directory_is_listed_as_a_module() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/integration");
    let listed = include_str!("main.rs");
    let missing: Vec<String> = std::fs::read_dir(dir)
        .expect("integration test directory is readable")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix("_test.rs").map(|stem| format!("{stem}_test")))
        .filter(|module| !listed.contains(&format!("mod {module};")))
        .collect();
    assert!(missing.is_empty(), "add `mod <name>;` to tests/integration/main.rs for: {missing:?}");
}

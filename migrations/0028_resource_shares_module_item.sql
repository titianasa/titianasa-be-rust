-- Phase 31 (P31-008) — module_item collaborator sharing, reusing the
-- existing resource_shares table (Drive's asset/folder sharing) rather
-- than a dedicated table: same shape (principal user-or-role, viewer/
-- editor permission, upsert-on-conflict) fits exactly. The gating logic
-- stays separate (see services/resource_share.rs's module_item-specific
-- functions) — module_items have no Drive-style "owner"/ancestor-folder
-- concept, so drive_permissions.rs's require_owner/require_access stay
-- untouched, only this CHECK widens to admit the new resource_type.
alter table "resource_shares" drop constraint "resource_shares_resource_type_check";
alter table "resource_shares" add constraint "resource_shares_resource_type_check"
  check ("resource_shares"."resource_type" in ('asset', 'folder', 'module_item'));

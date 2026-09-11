-- Restores ADR-0006's original design: `teacher`/`tutor` were always
-- meant to be one permission-equivalent role, differentiated only by
-- UI label from organizations.type — never two distinct role values.
-- Phase 32 drifted from that by treating them as separate roles; this
-- collapses any existing `tutor` rows into `teacher` (the value the
-- org-admin "promote" UI now writes exclusively going forward).
--
-- `tutor` itself stays a legal value in the CHECK constraint below —
-- per the ADR's own precedent, removing an unused enum value is just
-- churn with no benefit. `tutor_profiles` (bio/specializations) is
-- untouched: it's keyed by user_id alone, still valid under the
-- person's new `teacher` row.
insert into user_organization_roles (user_id, organization_id, role)
select user_id, organization_id, 'teacher'
from user_organization_roles
where role = 'tutor'
on conflict (user_id, organization_id, role) do nothing;

delete from user_organization_roles where role = 'tutor';

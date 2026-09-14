-- An author, reviewer, admin or collaborator checking a quiz before (or
-- after) it is published sits it at /belajar/{id}/preview. That sitting
-- is a real attempt — the server draws the paper, and AI-rubric grading
-- needs an attempt id to hang its evaluation on — but it is not learner
-- history: it earns nothing, completes nothing, feeds no learning event,
-- never enters the manual grading queue, and never blocks deleting a
-- draft. Marked explicitly rather than inferred from the caller's role,
-- so the learner URL and the preview URL can never be confused.
alter table attempts add column if not exists is_preview boolean not null default false;
create index if not exists attempts_preview_item_idx on attempts (item_id) where is_preview;

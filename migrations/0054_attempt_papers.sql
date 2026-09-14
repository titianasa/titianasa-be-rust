-- The paper a learner actually sat: which questions were drawn, the
-- order they were shown in, the per-question label mapping after the
-- choices were shuffled, and the learner-safe config that was rendered.
-- Built server-side when the attempt starts (services/quiz_paper.rs) so
-- the answer key never has to leave the server, and grading can map a
-- displayed "B" back to the stored label it stood for.
--
-- NULL for every attempt created before this existed — those are graded
-- exactly as before (every question, keys as submitted).
alter table attempts add column paper jsonb;

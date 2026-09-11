-- Phase 37: collapse module_items.content_type from 6 English-learning
-- categories down to 2 generic ones (article/quiz), replicating the
-- registry-based, JSONB-driven quiz system in the sibling parelabs-backend
-- reference project. See services/quiz_subtype.rs for the ~40-value
-- subtype registry this quiz_config column composes from.
--
-- learn/practice/review -> article (already just render content_blocks,
-- lossless rename). assessment/writing/speaking -> quiz, each item's
-- existing content backfilled into quiz_config below so nothing is
-- lost; the migrated writing/speaking question keeps the existing
-- AI-rubric grading path (ai_writing_evaluation/ai_speaking_evaluation),
-- now addressed by subtype instead of item-level content_type.
--
-- content_type keeps no DB CHECK from here on — module_item.rs's
-- CONTENT_TYPES array is the sole validator, matching the no-DB-CHECK
-- idiom already used by content_blocks.type and questions.type.

alter table module_items add column quiz_config jsonb;
alter table module_items drop constraint module_items_content_type_check;

update module_items set content_type = 'article'
where content_type in ('learn', 'practice', 'review');

update module_items set
  content_type = 'quiz',
  quiz_config = jsonb_build_object('sections', '[]'::jsonb, 'question_groups', '[]'::jsonb)
where content_type = 'assessment';

-- Writing items become one `essay`-subtype question group; prompt text
-- is pulled from the item's first text/heading content_block, falling
-- back to the item's own title when no such block exists.
update module_items mi set
  content_type = 'quiz',
  quiz_config = jsonb_build_object(
    'sections', jsonb_build_array(jsonb_build_object('id', 'section-1', 'title', 'Menulis')),
    'question_groups', jsonb_build_array(jsonb_build_object(
      'id', 'group-1',
      'section_id', 'section-1',
      'subtype', 'essay',
      'prompt', coalesce(
        (select cb.data->>'text' from content_blocks cb where cb.item_id = mi.id and cb.type in ('text', 'heading') order by cb.order_index asc limit 1),
        mi.title
      )
    ))
  )
where content_type = 'writing';

-- Speaking items become one `voice_record`-subtype question group;
-- prompt text prefers the dedicated speaking_prompt block's script
-- (the same one already used for TTS today), falling back to the title.
update module_items mi set
  content_type = 'quiz',
  quiz_config = jsonb_build_object(
    'sections', jsonb_build_array(jsonb_build_object('id', 'section-1', 'title', 'Berbicara')),
    'question_groups', jsonb_build_array(jsonb_build_object(
      'id', 'group-1',
      'section_id', 'section-1',
      'subtype', 'voice_record',
      'prompt', coalesce(
        (select cb.data->>'text' from content_blocks cb where cb.item_id = mi.id and cb.type = 'speaking_prompt' order by cb.order_index asc limit 1),
        mi.title
      )
    ))
  )
where content_type = 'speaking';

-- questions.type vocabulary aligned to the new subtype registry (same
-- literal names as parelabs-backend's SubtypeId).
update questions set type = 'multiple_choice' where type = 'mcq';
update questions set type = 'gap_fill' where type = 'fill_blank';

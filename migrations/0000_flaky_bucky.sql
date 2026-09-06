CREATE TABLE "ai_tasks" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid,
	"task_type" text NOT NULL,
	"provider" text NOT NULL,
	"model" text NOT NULL,
	"prompt_id" text NOT NULL,
	"tokens_used" integer,
	"cost" numeric,
	"status" text DEFAULT 'queued' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "ai_tasks_status_check" CHECK ("ai_tasks"."status" in ('queued', 'running', 'done', 'failed'))
);
--> statement-breakpoint
CREATE TABLE "assessment_questions" (
	"assessment_id" uuid NOT NULL,
	"question_id" uuid NOT NULL,
	"order_index" integer NOT NULL,
	"points" double precision DEFAULT 1 NOT NULL,
	CONSTRAINT "assessment_questions_assessment_id_question_id_pk" PRIMARY KEY("assessment_id","question_id")
);
--> statement-breakpoint
CREATE TABLE "assessments" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"type" text NOT NULL,
	"title" text NOT NULL,
	"config" jsonb DEFAULT '{}'::jsonb NOT NULL,
	CONSTRAINT "assessments_type_check" CHECK ("assessments"."type" in ('unit_test', 'level_assessment', 'mock_exam', 'ielts', 'toefl', 'pte'))
);
--> statement-breakpoint
CREATE TABLE "assets" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid,
	"url" text NOT NULL,
	"type" text NOT NULL,
	"visibility" text DEFAULT 'private' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"filename" text,
	"folder_id" uuid,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	"deleted_at" timestamp with time zone,
	CONSTRAINT "assets_visibility_check" CHECK ("assets"."visibility" in ('public', 'private'))
);
--> statement-breakpoint
CREATE TABLE "attempts" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"assessment_id" uuid,
	"lesson_id" uuid,
	"started_at" timestamp with time zone DEFAULT now() NOT NULL,
	"submitted_at" timestamp with time zone,
	"answers" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"score" double precision,
	"status" text DEFAULT 'in_progress' NOT NULL,
	"question_snapshot" jsonb,
	CONSTRAINT "attempts_status_check" CHECK ("attempts"."status" in ('in_progress', 'submitted', 'evaluated')),
	CONSTRAINT "attempts_assessment_or_lesson_check" CHECK ("attempts"."assessment_id" is not null or "attempts"."lesson_id" is not null)
);
--> statement-breakpoint
CREATE TABLE "concept_prerequisites" (
	"concept_id" uuid NOT NULL,
	"prerequisite_concept_id" uuid NOT NULL,
	CONSTRAINT "concept_prerequisites_concept_id_prerequisite_concept_id_pk" PRIMARY KEY("concept_id","prerequisite_concept_id")
);
--> statement-breakpoint
CREATE TABLE "concepts" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"subject_id" uuid NOT NULL,
	"code" text NOT NULL,
	"name" text NOT NULL,
	"type" text NOT NULL,
	"parent_concept_id" uuid,
	"indonesian_difficulty_tag" text,
	CONSTRAINT "concepts_subject_id_code_key" UNIQUE("subject_id","code"),
	CONSTRAINT "concepts_type_check" CHECK ("concepts"."type" in ('grammar', 'vocabulary', 'skill', 'pronunciation'))
);
--> statement-breakpoint
CREATE TABLE "content_blocks" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"lesson_id" uuid NOT NULL,
	"type" text NOT NULL,
	"order_index" integer NOT NULL,
	"data" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"raw_source" text
);
--> statement-breakpoint
CREATE TABLE "credits" (
	"user_id" uuid PRIMARY KEY NOT NULL,
	"balance" bigint DEFAULT 0 NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "curricula" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"subject_id" uuid NOT NULL,
	"code" text NOT NULL,
	"name" text NOT NULL,
	"framework" text NOT NULL,
	"version" integer DEFAULT 1 NOT NULL,
	"status" text DEFAULT 'draft' NOT NULL,
	CONSTRAINT "curricula_code_unique" UNIQUE("code"),
	CONSTRAINT "curricula_framework_check" CHECK ("curricula"."framework" in ('cefr', 'cambridge', 'kurikulum_id', 'custom')),
	CONSTRAINT "curricula_status_check" CHECK ("curricula"."status" in ('draft', 'published', 'archived'))
);
--> statement-breakpoint
CREATE TABLE "evaluations" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"attempt_id" uuid NOT NULL,
	"evaluator_type" text NOT NULL,
	"rubric_id" uuid,
	"scores" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"evidence" jsonb,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "evaluations_evaluator_type_check" CHECK ("evaluations"."evaluator_type" in ('ai', 'human'))
);
--> statement-breakpoint
CREATE TABLE "exam_sessions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"assessment_id" uuid NOT NULL,
	"user_id" uuid NOT NULL,
	"status" text DEFAULT 'not_started' NOT NULL,
	"started_at" timestamp with time zone,
	"submitted_at" timestamp with time zone,
	CONSTRAINT "exam_sessions_status_check" CHECK ("exam_sessions"."status" in ('not_started', 'in_progress', 'submitted', 'timed_out'))
);
--> statement-breakpoint
CREATE TABLE "feedback" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"evaluation_id" uuid NOT NULL,
	"type" text NOT NULL,
	"content" text NOT NULL,
	"position" jsonb,
	"created_by" uuid,
	CONSTRAINT "feedback_type_check" CHECK ("feedback"."type" in ('annotation', 'comment', 'voice_note'))
);
--> statement-breakpoint
CREATE TABLE "folders" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"name" text NOT NULL,
	"parent_folder_id" uuid,
	"owner_id" uuid NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	"deleted_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE "frss_schedule" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"concept_id" uuid NOT NULL,
	"interval_days" double precision DEFAULT 1 NOT NULL,
	"ease_factor" double precision DEFAULT 2.5 NOT NULL,
	"due_at" timestamp with time zone DEFAULT now() NOT NULL,
	"last_result" text,
	CONSTRAINT "frss_schedule_user_id_concept_id_key" UNIQUE("user_id","concept_id"),
	CONSTRAINT "frss_schedule_last_result_check" CHECK ("frss_schedule"."last_result" in ('recalled', 'partial', 'forgot'))
);
--> statement-breakpoint
CREATE TABLE "learning_events" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"event_type" text NOT NULL,
	"entity_type" text NOT NULL,
	"entity_id" uuid NOT NULL,
	"payload" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "lesson_concepts" (
	"lesson_id" uuid NOT NULL,
	"concept_id" uuid NOT NULL,
	"weight" double precision DEFAULT 1 NOT NULL,
	CONSTRAINT "lesson_concepts_lesson_id_concept_id_pk" PRIMARY KEY("lesson_id","concept_id")
);
--> statement-breakpoint
CREATE TABLE "lessons" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"unit_id" uuid NOT NULL,
	"title" text NOT NULL,
	"type" text NOT NULL,
	"order_index" integer NOT NULL,
	"status" text DEFAULT 'draft' NOT NULL,
	"version" integer DEFAULT 1 NOT NULL,
	"superseded_by" uuid,
	"qa_report" jsonb,
	CONSTRAINT "lessons_type_check" CHECK ("lessons"."type" in ('learn', 'practice', 'speaking', 'writing', 'review', 'assessment')),
	CONSTRAINT "lessons_status_check" CHECK ("lessons"."status" in ('draft', 'in_review', 'published', 'archived'))
);
--> statement-breakpoint
CREATE TABLE "levels" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"curriculum_id" uuid NOT NULL,
	"code" text NOT NULL,
	"name" text NOT NULL,
	"order_index" integer NOT NULL
);
--> statement-breakpoint
CREATE TABLE "masteries" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"concept_id" uuid NOT NULL,
	"score" double precision DEFAULT 0 NOT NULL,
	"confidence" double precision DEFAULT 0 NOT NULL,
	"last_reviewed_at" timestamp with time zone,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "masteries_user_id_concept_id_key" UNIQUE("user_id","concept_id")
);
--> statement-breakpoint
CREATE TABLE "organizations" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"name" text NOT NULL,
	"slug" text NOT NULL,
	"type" text NOT NULL,
	"settings" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "organizations_slug_unique" UNIQUE("slug"),
	CONSTRAINT "organizations_type_check" CHECK ("organizations"."type" in ('platform', 'school', 'tutor_org'))
);
--> statement-breakpoint
CREATE TABLE "proctoring_events" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"proctoring_session_id" uuid NOT NULL,
	"type" text NOT NULL,
	"severity" text NOT NULL,
	"timestamp" timestamp with time zone DEFAULT now() NOT NULL,
	"metadata" jsonb,
	"evidence_id" uuid,
	CONSTRAINT "proctoring_events_severity_check" CHECK ("proctoring_events"."severity" in ('low', 'medium', 'high'))
);
--> statement-breakpoint
CREATE TABLE "proctoring_policies" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"exam_type" text NOT NULL,
	"camera" text DEFAULT 'off' NOT NULL,
	"microphone" text DEFAULT 'off' NOT NULL,
	"screen" text DEFAULT 'off' NOT NULL,
	"fullscreen_required" boolean DEFAULT false NOT NULL,
	"focus_monitoring" boolean DEFAULT false NOT NULL,
	CONSTRAINT "proctoring_policies_camera_check" CHECK ("proctoring_policies"."camera" in ('off', 'optional', 'on')),
	CONSTRAINT "proctoring_policies_microphone_check" CHECK ("proctoring_policies"."microphone" in ('off', 'optional', 'on')),
	CONSTRAINT "proctoring_policies_screen_check" CHECK ("proctoring_policies"."screen" in ('off', 'optional', 'on'))
);
--> statement-breakpoint
CREATE TABLE "proctoring_sessions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"exam_session_id" uuid NOT NULL,
	"policy_id" uuid NOT NULL,
	"device_info" jsonb
);
--> statement-breakpoint
CREATE TABLE "question_banks" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"subject_id" uuid NOT NULL,
	"name" text NOT NULL
);
--> statement-breakpoint
CREATE TABLE "question_concepts" (
	"question_id" uuid NOT NULL,
	"concept_id" uuid NOT NULL,
	CONSTRAINT "question_concepts_question_id_concept_id_pk" PRIMARY KEY("question_id","concept_id")
);
--> statement-breakpoint
CREATE TABLE "questions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"bank_id" uuid NOT NULL,
	"type" text NOT NULL,
	"difficulty" double precision DEFAULT 0.5 NOT NULL,
	"data" jsonb NOT NULL,
	"correct_answer" jsonb NOT NULL,
	"explanation" jsonb,
	"status" text DEFAULT 'draft' NOT NULL,
	"version" integer DEFAULT 1 NOT NULL,
	"superseded_by" uuid,
	"qa_report" jsonb,
	"cefr_tag" text,
	CONSTRAINT "questions_difficulty_check" CHECK ("questions"."difficulty" >= 0 and "questions"."difficulty" <= 1),
	CONSTRAINT "questions_status_check" CHECK ("questions"."status" in ('draft', 'in_review', 'published', 'archived')),
	CONSTRAINT "questions_cefr_tag_check" CHECK ("questions"."cefr_tag" in ('a1', 'a2', 'b1', 'b2', 'c1', 'c2'))
);
--> statement-breakpoint
CREATE TABLE "refresh_tokens" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"token_hash" text NOT NULL,
	"expires_at" timestamp with time zone NOT NULL,
	"revoked_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "refresh_tokens_token_hash_unique" UNIQUE("token_hash")
);
--> statement-breakpoint
CREATE TABLE "resource_activity" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"resource_type" text NOT NULL,
	"resource_id" uuid NOT NULL,
	"actor_id" uuid NOT NULL,
	"action" text NOT NULL,
	"detail" jsonb,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "resource_activity_resource_type_check" CHECK ("resource_activity"."resource_type" in ('asset', 'folder'))
);
--> statement-breakpoint
CREATE TABLE "resource_shares" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"resource_type" text NOT NULL,
	"resource_id" uuid NOT NULL,
	"principal_type" text NOT NULL,
	"principal_id" text NOT NULL,
	"permission" text NOT NULL,
	"granted_by" uuid NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "resource_shares_unique_grant" UNIQUE("resource_type","resource_id","principal_type","principal_id"),
	CONSTRAINT "resource_shares_resource_type_check" CHECK ("resource_shares"."resource_type" in ('asset', 'folder')),
	CONSTRAINT "resource_shares_principal_type_check" CHECK ("resource_shares"."principal_type" in ('user', 'role')),
	CONSTRAINT "resource_shares_permission_check" CHECK ("resource_shares"."permission" in ('viewer', 'editor'))
);
--> statement-breakpoint
CREATE TABLE "rubrics" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"name" text NOT NULL,
	"criteria" jsonb NOT NULL
);
--> statement-breakpoint
CREATE TABLE "subjects" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"code" text NOT NULL,
	"name" text NOT NULL,
	CONSTRAINT "subjects_code_unique" UNIQUE("code")
);
--> statement-breakpoint
CREATE TABLE "transactions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"type" text NOT NULL,
	"amount" bigint NOT NULL,
	"reference" text,
	"expires_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "transactions_type_check" CHECK ("transactions"."type" in ('earn', 'spend', 'purchase', 'refund', 'payout_earned', 'payout_withdrawn'))
);
--> statement-breakpoint
CREATE TABLE "units" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"level_id" uuid NOT NULL,
	"code" text NOT NULL,
	"title" text NOT NULL,
	"order_index" integer NOT NULL
);
--> statement-breakpoint
CREATE TABLE "user_organization_roles" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL,
	"organization_id" uuid NOT NULL,
	"role" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "user_organization_roles_user_org_role_key" UNIQUE("user_id","organization_id","role"),
	CONSTRAINT "user_organization_roles_role_check" CHECK ("user_organization_roles"."role" in ('platform_admin', 'org_owner', 'academic_director', 'curriculum_developer', 'reviewer', 'teacher', 'tutor', 'student', 'parent'))
);
--> statement-breakpoint
CREATE TABLE "users" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"email" text NOT NULL,
	"google_id" text,
	"name" text NOT NULL,
	"avatar_url" text,
	"locale" text DEFAULT 'id' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "users_email_unique" UNIQUE("email"),
	CONSTRAINT "users_google_id_unique" UNIQUE("google_id")
);
--> statement-breakpoint
ALTER TABLE "ai_tasks" ADD CONSTRAINT "ai_tasks_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "assessment_questions" ADD CONSTRAINT "assessment_questions_assessment_id_assessments_id_fk" FOREIGN KEY ("assessment_id") REFERENCES "public"."assessments"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "assessment_questions" ADD CONSTRAINT "assessment_questions_question_id_questions_id_fk" FOREIGN KEY ("question_id") REFERENCES "public"."questions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "assets" ADD CONSTRAINT "assets_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "assets" ADD CONSTRAINT "assets_folder_id_folders_id_fk" FOREIGN KEY ("folder_id") REFERENCES "public"."folders"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "attempts" ADD CONSTRAINT "attempts_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "attempts" ADD CONSTRAINT "attempts_assessment_id_assessments_id_fk" FOREIGN KEY ("assessment_id") REFERENCES "public"."assessments"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "attempts" ADD CONSTRAINT "attempts_lesson_id_lessons_id_fk" FOREIGN KEY ("lesson_id") REFERENCES "public"."lessons"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "concept_prerequisites" ADD CONSTRAINT "concept_prerequisites_concept_id_concepts_id_fk" FOREIGN KEY ("concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "concept_prerequisites" ADD CONSTRAINT "concept_prerequisites_prerequisite_concept_id_concepts_id_fk" FOREIGN KEY ("prerequisite_concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "concepts" ADD CONSTRAINT "concepts_subject_id_subjects_id_fk" FOREIGN KEY ("subject_id") REFERENCES "public"."subjects"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "concepts" ADD CONSTRAINT "concepts_parent_concept_id_concepts_id_fk" FOREIGN KEY ("parent_concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "content_blocks" ADD CONSTRAINT "content_blocks_lesson_id_lessons_id_fk" FOREIGN KEY ("lesson_id") REFERENCES "public"."lessons"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "credits" ADD CONSTRAINT "credits_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "curricula" ADD CONSTRAINT "curricula_subject_id_subjects_id_fk" FOREIGN KEY ("subject_id") REFERENCES "public"."subjects"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "evaluations" ADD CONSTRAINT "evaluations_attempt_id_attempts_id_fk" FOREIGN KEY ("attempt_id") REFERENCES "public"."attempts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "evaluations" ADD CONSTRAINT "evaluations_rubric_id_rubrics_id_fk" FOREIGN KEY ("rubric_id") REFERENCES "public"."rubrics"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "exam_sessions" ADD CONSTRAINT "exam_sessions_assessment_id_assessments_id_fk" FOREIGN KEY ("assessment_id") REFERENCES "public"."assessments"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "exam_sessions" ADD CONSTRAINT "exam_sessions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "feedback" ADD CONSTRAINT "feedback_evaluation_id_evaluations_id_fk" FOREIGN KEY ("evaluation_id") REFERENCES "public"."evaluations"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "feedback" ADD CONSTRAINT "feedback_created_by_users_id_fk" FOREIGN KEY ("created_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "folders" ADD CONSTRAINT "folders_parent_folder_id_folders_id_fk" FOREIGN KEY ("parent_folder_id") REFERENCES "public"."folders"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "folders" ADD CONSTRAINT "folders_owner_id_users_id_fk" FOREIGN KEY ("owner_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "frss_schedule" ADD CONSTRAINT "frss_schedule_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "frss_schedule" ADD CONSTRAINT "frss_schedule_concept_id_concepts_id_fk" FOREIGN KEY ("concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "learning_events" ADD CONSTRAINT "learning_events_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "lesson_concepts" ADD CONSTRAINT "lesson_concepts_lesson_id_lessons_id_fk" FOREIGN KEY ("lesson_id") REFERENCES "public"."lessons"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "lesson_concepts" ADD CONSTRAINT "lesson_concepts_concept_id_concepts_id_fk" FOREIGN KEY ("concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "lessons" ADD CONSTRAINT "lessons_unit_id_units_id_fk" FOREIGN KEY ("unit_id") REFERENCES "public"."units"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "lessons" ADD CONSTRAINT "lessons_superseded_by_lessons_id_fk" FOREIGN KEY ("superseded_by") REFERENCES "public"."lessons"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "levels" ADD CONSTRAINT "levels_curriculum_id_curricula_id_fk" FOREIGN KEY ("curriculum_id") REFERENCES "public"."curricula"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "masteries" ADD CONSTRAINT "masteries_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "masteries" ADD CONSTRAINT "masteries_concept_id_concepts_id_fk" FOREIGN KEY ("concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proctoring_events" ADD CONSTRAINT "proctoring_events_proctoring_session_id_proctoring_sessions_id_fk" FOREIGN KEY ("proctoring_session_id") REFERENCES "public"."proctoring_sessions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proctoring_events" ADD CONSTRAINT "proctoring_events_evidence_id_assets_id_fk" FOREIGN KEY ("evidence_id") REFERENCES "public"."assets"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD CONSTRAINT "proctoring_sessions_exam_session_id_exam_sessions_id_fk" FOREIGN KEY ("exam_session_id") REFERENCES "public"."exam_sessions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD CONSTRAINT "proctoring_sessions_policy_id_proctoring_policies_id_fk" FOREIGN KEY ("policy_id") REFERENCES "public"."proctoring_policies"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "question_banks" ADD CONSTRAINT "question_banks_subject_id_subjects_id_fk" FOREIGN KEY ("subject_id") REFERENCES "public"."subjects"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "question_concepts" ADD CONSTRAINT "question_concepts_question_id_questions_id_fk" FOREIGN KEY ("question_id") REFERENCES "public"."questions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "question_concepts" ADD CONSTRAINT "question_concepts_concept_id_concepts_id_fk" FOREIGN KEY ("concept_id") REFERENCES "public"."concepts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "questions" ADD CONSTRAINT "questions_bank_id_question_banks_id_fk" FOREIGN KEY ("bank_id") REFERENCES "public"."question_banks"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "questions" ADD CONSTRAINT "questions_superseded_by_questions_id_fk" FOREIGN KEY ("superseded_by") REFERENCES "public"."questions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "refresh_tokens" ADD CONSTRAINT "refresh_tokens_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "resource_activity" ADD CONSTRAINT "resource_activity_actor_id_users_id_fk" FOREIGN KEY ("actor_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "resource_shares" ADD CONSTRAINT "resource_shares_granted_by_users_id_fk" FOREIGN KEY ("granted_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "transactions" ADD CONSTRAINT "transactions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "units" ADD CONSTRAINT "units_level_id_levels_id_fk" FOREIGN KEY ("level_id") REFERENCES "public"."levels"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "user_organization_roles" ADD CONSTRAINT "user_organization_roles_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "user_organization_roles" ADD CONSTRAINT "user_organization_roles_organization_id_organizations_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organizations"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "idx_attempts_user" ON "attempts" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX "idx_folders_parent" ON "folders" USING btree ("parent_folder_id");--> statement-breakpoint
CREATE INDEX "idx_folders_owner" ON "folders" USING btree ("owner_id");--> statement-breakpoint
CREATE INDEX "idx_frss_due" ON "frss_schedule" USING btree ("user_id","due_at");--> statement-breakpoint
CREATE INDEX "idx_learning_events_user_time" ON "learning_events" USING btree ("user_id","created_at");--> statement-breakpoint
CREATE INDEX "idx_learning_events_entity" ON "learning_events" USING btree ("entity_type","entity_id");--> statement-breakpoint
CREATE INDEX "idx_refresh_tokens_user" ON "refresh_tokens" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX "idx_resource_activity_resource" ON "resource_activity" USING btree ("resource_type","resource_id","created_at" DESC NULLS LAST);--> statement-breakpoint
CREATE INDEX "idx_resource_shares_resource" ON "resource_shares" USING btree ("resource_type","resource_id");--> statement-breakpoint
CREATE INDEX "idx_resource_shares_principal" ON "resource_shares" USING btree ("principal_type","principal_id");--> statement-breakpoint
CREATE INDEX "idx_transactions_user" ON "transactions" USING btree ("user_id");
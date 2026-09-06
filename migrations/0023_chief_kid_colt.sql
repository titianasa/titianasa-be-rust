CREATE TABLE "canvas_events" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"session_id" uuid NOT NULL,
	"actor_id" uuid NOT NULL,
	"type" text NOT NULL,
	"payload" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "canvas_events_type_check" CHECK ("canvas_events"."type" in ('comment_created', 'comment_resolved', 'mode_changed'))
);
--> statement-breakpoint
CREATE TABLE "canvas_sessions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"conversation_id" uuid NOT NULL,
	"lesson_id" uuid NOT NULL,
	"mode" text DEFAULT 'learning' NOT NULL,
	"status" text DEFAULT 'active' NOT NULL,
	"content" text DEFAULT '' NOT NULL,
	"version" integer DEFAULT 0 NOT NULL,
	"submitted_attempt_id" uuid,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"closed_at" timestamp with time zone,
	CONSTRAINT "canvas_sessions_mode_check" CHECK ("canvas_sessions"."mode" in ('learning', 'assessment', 'exam')),
	CONSTRAINT "canvas_sessions_status_check" CHECK ("canvas_sessions"."status" in ('active', 'closed'))
);
--> statement-breakpoint
CREATE TABLE "canvas_snapshots" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"session_id" uuid NOT NULL,
	"content" text NOT NULL,
	"version" integer NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "canvas_events" ADD CONSTRAINT "canvas_events_session_id_canvas_sessions_id_fk" FOREIGN KEY ("session_id") REFERENCES "public"."canvas_sessions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "canvas_events" ADD CONSTRAINT "canvas_events_actor_id_users_id_fk" FOREIGN KEY ("actor_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "canvas_sessions" ADD CONSTRAINT "canvas_sessions_conversation_id_conversations_id_fk" FOREIGN KEY ("conversation_id") REFERENCES "public"."conversations"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "canvas_sessions" ADD CONSTRAINT "canvas_sessions_lesson_id_lessons_id_fk" FOREIGN KEY ("lesson_id") REFERENCES "public"."lessons"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "canvas_sessions" ADD CONSTRAINT "canvas_sessions_submitted_attempt_id_attempts_id_fk" FOREIGN KEY ("submitted_attempt_id") REFERENCES "public"."attempts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "canvas_snapshots" ADD CONSTRAINT "canvas_snapshots_session_id_canvas_sessions_id_fk" FOREIGN KEY ("session_id") REFERENCES "public"."canvas_sessions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "idx_canvas_events_session_time" ON "canvas_events" USING btree ("session_id","created_at");--> statement-breakpoint
CREATE INDEX "idx_canvas_snapshots_session_time" ON "canvas_snapshots" USING btree ("session_id","created_at");
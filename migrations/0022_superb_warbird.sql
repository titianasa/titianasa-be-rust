ALTER TABLE "lessons" ADD COLUMN "generated_by" text DEFAULT 'human' NOT NULL;--> statement-breakpoint
ALTER TABLE "questions" ADD COLUMN "generated_by" text DEFAULT 'human' NOT NULL;--> statement-breakpoint
ALTER TABLE "lessons" ADD CONSTRAINT "lessons_generated_by_check" CHECK ("lessons"."generated_by" in ('human', 'ai'));--> statement-breakpoint
ALTER TABLE "questions" ADD CONSTRAINT "questions_generated_by_check" CHECK ("questions"."generated_by" in ('human', 'ai'));
CREATE TABLE "subscriptions" (
	"user_id" uuid PRIMARY KEY NOT NULL,
	"tier" text NOT NULL,
	"status" text DEFAULT 'active' NOT NULL,
	"current_period_start" timestamp with time zone NOT NULL,
	"current_period_end" timestamp with time zone NOT NULL,
	CONSTRAINT "subscriptions_tier_check" CHECK ("subscriptions"."tier" in ('plus', 'pro')),
	CONSTRAINT "subscriptions_status_check" CHECK ("subscriptions"."status" in ('active', 'cancelled'))
);
--> statement-breakpoint
ALTER TABLE "transactions" DROP CONSTRAINT "transactions_type_check";--> statement-breakpoint
ALTER TABLE "subscriptions" ADD CONSTRAINT "subscriptions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "transactions" ADD CONSTRAINT "transactions_type_check" CHECK ("transactions"."type" in ('earn', 'spend', 'purchase', 'refund', 'payout_earned', 'payout_withdrawn', 'expire'));
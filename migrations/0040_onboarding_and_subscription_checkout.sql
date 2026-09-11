-- Onboarding funnel + paid subscriptions.
--
-- Three problems this solves:
--
-- 1. A visitor currently meets a Google login button before they have
--    any reason to trust the product. The new flow lets them state a
--    learning goal and see a real path built from the module library
--    FIRST; the profile they fill in is held in localStorage and
--    synced here once (and only once) they actually sign in — so a
--    visitor who never signs up leaves no row behind.
--
-- 2. Nothing tracked whether a signed-in user had been shown the
--    dashboard walkthrough, so it could not be resumed or dismissed.
--
-- 3. `orders` was hard-bound to an enrollment (marketplace class
--    checkout). Subscribing was therefore free — subscription.rs
--    activated the tier immediately with no payment step. Generalising
--    orders lets the SAME QRIS + webhook path settle both.

create table user_learning_profiles (
    user_id uuid primary key references users(id) on delete cascade,
    -- What the learner said they want, in their own terms. Free-form
    -- rather than FK'd to modules: this is stated intent captured
    -- before they have an account, not a curriculum reference.
    goal text,
    jenjang text,
    -- Subject slugs / interest tags they picked, and the exam labels
    -- those map to (`ujian:utbk-pm`, ...). Kept as jsonb so the
    -- onboarding wizard can add questions without a migration, the same
    -- reasoning as modules.metadata in 0039.
    interests jsonb not null default '[]'::jsonb,
    target_labels jsonb not null default '[]'::jsonb,
    daily_minutes integer,
    -- Dashboard walkthrough state. Null = never finished; the checklist
    -- is derived from real activity, this only records dismissal.
    onboarding_completed_at timestamp with time zone,
    created_at timestamp with time zone not null default now(),
    updated_at timestamp with time zone not null default now()
);

-- Subscription orders have no enrollment, so the column has to relax.
-- Existing rows are all class checkouts and keep their enrollment_id.
alter table orders alter column enrollment_id drop not null;

-- What this order is FOR. Defaulted to 'enrollment' so every existing
-- row keeps its current meaning without a backfill.
alter table orders add column kind text not null default 'enrollment';
alter table orders add constraint orders_kind_check
    check (kind in ('enrollment', 'subscription'));

-- Subscription orders need to know the buyer and tier directly; an
-- enrollment order gets both by walking enrollment_id.
alter table orders add column user_id uuid references users(id) on delete cascade;
alter table orders add column subscription_tier text;

-- Each kind must carry exactly the fields it needs. This is the
-- constraint that stops a subscription order from being created without
-- a tier, or an enrollment order without its enrollment.
alter table orders add constraint orders_kind_fields_check check (
    (kind = 'enrollment' and enrollment_id is not null)
    or (kind = 'subscription' and user_id is not null and subscription_tier is not null)
);

create index orders_user_id_idx on orders (user_id) where user_id is not null;

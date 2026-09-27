-- ── Per-channel write policy (upstream issue #2497) ──────────────────────────
-- Channel access is membership-binary today: `channel_members.member_role`
-- exists (owner/admin/member/guest/bot) but is NOT consulted in the message
-- write path, so any member may originate a message in any channel they
-- belong to. That makes announcement / read-only / human-only channels
-- unrepresentable, and leaves moderation as the only lever — which acts after
-- the post is already visible.
--
-- This adds the general policy axis rather than a single-purpose "human only"
-- flag: `human_only` is one value of it. Enforcement lives at relay ingest,
-- after the membership gate; this migration only supplies the storage.
--
-- Additive migration: previously applied files must not change checksum.
-- The default `any_member` reproduces today's membership-binary behavior
-- exactly, so existing rows and old binaries are unaffected (a relay that
-- does not know the column simply never reads it).
--
-- Values:
--   any_member  — today's behavior: any member may originate. DEFAULT.
--   admins_only — only Owner/Admin may originate (announcement channels).
--   human_only  — members whose role is not `bot` may originate; agents read
--                 and may still receive relay-authored digests.

CREATE TYPE channel_write_policy AS ENUM ('any_member', 'admins_only', 'human_only');

ALTER TABLE channels
    ADD COLUMN write_policy channel_write_policy NOT NULL DEFAULT 'any_member';

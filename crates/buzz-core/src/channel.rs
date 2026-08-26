//! Channel and membership enums shared across crates.
//!
//! These live in `buzz-core` (zero I/O deps) so both the SDK (client-side)
//! and the DB layer (server-side) can use the same types without pulling in
//! sqlx/tokio.

use std::fmt;
use std::str::FromStr;

/// Returns the canonical display name for a channel.
///
/// Channel names are rendered with a leading `#` by clients, so surrounding
/// whitespace and user-supplied hash prefixes are removed here to keep the
/// stored name prefix-free.
pub fn canonical_channel_name(name: &str) -> &str {
    name.trim_start_matches(|c: char| c == '#' || c.is_whitespace())
        .trim_end()
}

/// Whether a channel is publicly visible or invite-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelVisibility {
    /// Searchable; anyone can join without an invite.
    Open,
    /// Hidden; requires an invite to join.
    Private,
}

impl ChannelVisibility {
    /// Canonical string representation (matches DB enum and Nostr tags).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Private => "private",
        }
    }
}

impl fmt::Display for ChannelVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChannelVisibility {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "private" => Ok(Self::Private),
            other => Err(format!("unknown channel visibility: {other:?}")),
        }
    }
}

/// The functional type of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    /// Linear message stream (the default).
    Stream,
    /// Threaded forum-style discussion.
    Forum,
    /// Direct message conversation.
    Dm,
    /// Internal workflow execution channel.
    Workflow,
}

impl ChannelType {
    /// Canonical string representation (matches DB enum and Nostr tags).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Forum => "forum",
            Self::Dm => "dm",
            Self::Workflow => "workflow",
        }
    }
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChannelType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stream" => Ok(Self::Stream),
            "forum" => Ok(Self::Forum),
            "dm" => Ok(Self::Dm),
            "workflow" => Ok(Self::Workflow),
            other => Err(format!("unknown channel type: {other:?}")),
        }
    }
}

/// A member's role within a channel.
///
/// The hierarchy for permission checks is: Owner > Admin > Member > Guest.
/// Bot is a **separate designation** — it is not part of the linear hierarchy.
/// Use [`MemberRole::permission_level`] for numeric comparisons in authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRole {
    /// Full control — can manage members and delete the channel.
    Owner,
    /// Can manage members and channel settings.
    Admin,
    /// Standard participant.
    Member,
    /// Read-only external participant.
    Guest,
    /// Automated agent or integration (not in the role hierarchy).
    Bot,
}

impl MemberRole {
    /// Canonical string representation (matches DB enum and Nostr tags).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Guest => "guest",
            Self::Bot => "bot",
        }
    }

    /// Elevated roles that only existing owners/admins may grant.
    pub fn is_elevated(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Numeric permission level for authorization comparisons.
    ///
    /// Higher = more privileged. Bot returns 0 (must use explicit grants).
    /// Use `role.permission_level() >= required.permission_level()` for checks.
    pub fn permission_level(self) -> u8 {
        match self {
            Self::Owner => 4,
            Self::Admin => 3,
            Self::Member => 2,
            Self::Guest => 1,
            Self::Bot => 0,
        }
    }

    /// Returns true if this role meets or exceeds the required role's permission level.
    ///
    /// Bot never meets any requirement (returns false for all non-Bot requirements).
    pub fn has_at_least(self, required: MemberRole) -> bool {
        self.permission_level() >= required.permission_level()
    }
}

impl fmt::Display for MemberRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemberRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            "guest" => Ok(Self::Guest),
            "bot" => Ok(Self::Bot),
            other => Err(format!("unknown member role: {other:?}")),
        }
    }
}

/// Who may originate a message in a channel.
///
/// Channel access is otherwise membership-binary: a member of a channel may
/// post in it. This axis is the missing "who may originate" dimension that
/// [`ChannelType`] (functional shape) and [`MemberRole`] (a per-member grant)
/// deliberately do not carry — see upstream issue #2497.
///
/// Enforced at relay ingest **after** the membership gate: a policy can only
/// ever narrow who may post, never widen it. Reads and membership are
/// untouched by every value.
///
/// [`Self::AnyMember`] is the default and reproduces the historic
/// membership-binary behavior exactly, so a channel that never sets a policy
/// behaves as it always has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelWritePolicy {
    /// Any member may originate a message (historic behavior).
    #[default]
    AnyMember,
    /// Only Owner/Admin may originate — announcement channels.
    AdminsOnly,
    /// Members whose role is not [`MemberRole::Bot`] may originate. Agents
    /// keep read access, and relay-authored digests are unaffected because
    /// they are not authored by a Bot member.
    HumanOnly,
}

impl ChannelWritePolicy {
    /// Canonical string representation (matches DB enum and Nostr tags).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AnyMember => "any_member",
            Self::AdminsOnly => "admins_only",
            Self::HumanOnly => "human_only",
        }
    }

    /// Whether `role` may originate a message under this policy.
    ///
    /// This is the single decision function; ingest and any client preview
    /// must both route through it so they can never disagree.
    ///
    /// Deliberately mirrors the `git_perms` treatment of Bot/Guest as one
    /// "may not originate" class rather than minting new role semantics:
    /// [`MemberRole::Guest`] is documented read-only and [`MemberRole::Bot`]
    /// sits outside the hierarchy at level 0, so neither may originate under
    /// any policy — including the permissive default, which matches the
    /// existing `has_at_least(Member)` floor.
    pub fn allows_post(self, role: MemberRole) -> bool {
        match self {
            Self::AnyMember => role.has_at_least(MemberRole::Member),
            Self::AdminsOnly => role.has_at_least(MemberRole::Admin),
            Self::HumanOnly => {
                role.has_at_least(MemberRole::Member) && !matches!(role, MemberRole::Bot)
            }
        }
    }
}

impl fmt::Display for ChannelWritePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChannelWritePolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "any_member" => Ok(Self::AnyMember),
            "admins_only" => Ok(Self::AdminsOnly),
            "human_only" => Ok(Self::HumanOnly),
            other => Err(format!("unknown channel write policy: {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_channel_name;
    use super::{ChannelWritePolicy, MemberRole};
    use std::str::FromStr;

    /// The default MUST reproduce historic membership-binary behavior, or
    /// every existing channel silently changes semantics on deploy.
    #[test]
    fn default_policy_is_any_member() {
        assert_eq!(ChannelWritePolicy::default(), ChannelWritePolicy::AnyMember);
    }

    /// The full policy x role matrix. This is the authz contract; if a cell
    /// changes, that change must be deliberate.
    #[test]
    fn allows_post_matrix() {
        use ChannelWritePolicy::{AdminsOnly, AnyMember, HumanOnly};
        use MemberRole::{Admin, Bot, Guest, Member, Owner};

        // any_member: the historic floor is has_at_least(Member).
        assert!(AnyMember.allows_post(Owner));
        assert!(AnyMember.allows_post(Admin));
        assert!(AnyMember.allows_post(Member));
        assert!(!AnyMember.allows_post(Guest), "guest is read-only");
        assert!(!AnyMember.allows_post(Bot), "bot is level 0");

        // admins_only: announcement channels.
        assert!(AdminsOnly.allows_post(Owner));
        assert!(AdminsOnly.allows_post(Admin));
        assert!(!AdminsOnly.allows_post(Member));
        assert!(!AdminsOnly.allows_post(Guest));
        assert!(!AdminsOnly.allows_post(Bot));

        // human_only: the REG-8 headline. Humans post, agents do not.
        assert!(HumanOnly.allows_post(Owner));
        assert!(HumanOnly.allows_post(Admin));
        assert!(HumanOnly.allows_post(Member));
        assert!(!HumanOnly.allows_post(Guest));
        assert!(!HumanOnly.allows_post(Bot), "the whole point of the policy");
    }

    /// A policy may only ever NARROW who can post relative to the default.
    /// Stated as a property so a future value cannot accidentally widen.
    #[test]
    fn no_policy_widens_beyond_the_default() {
        for policy in [
            ChannelWritePolicy::AnyMember,
            ChannelWritePolicy::AdminsOnly,
            ChannelWritePolicy::HumanOnly,
        ] {
            for role in [
                MemberRole::Owner,
                MemberRole::Admin,
                MemberRole::Member,
                MemberRole::Guest,
                MemberRole::Bot,
            ] {
                if policy.allows_post(role) {
                    assert!(
                        ChannelWritePolicy::AnyMember.allows_post(role),
                        "{policy} allowed {role} whom the default denies — policies must only narrow"
                    );
                }
            }
        }
    }

    /// Bot is denied under EVERY policy, including the permissive default.
    /// REG-8's agent-summary carve-out therefore needs no allow-path grant:
    /// the digest writer is the relay, not a Bot member.
    #[test]
    fn bot_may_never_originate_under_any_policy() {
        for policy in [
            ChannelWritePolicy::AnyMember,
            ChannelWritePolicy::AdminsOnly,
            ChannelWritePolicy::HumanOnly,
        ] {
            assert!(!policy.allows_post(MemberRole::Bot), "{policy}");
        }
    }

    #[test]
    fn write_policy_round_trips_through_its_canonical_string() {
        for policy in [
            ChannelWritePolicy::AnyMember,
            ChannelWritePolicy::AdminsOnly,
            ChannelWritePolicy::HumanOnly,
        ] {
            assert_eq!(
                ChannelWritePolicy::from_str(policy.as_str()).unwrap(),
                policy
            );
            assert_eq!(policy.to_string(), policy.as_str());
        }
    }

    /// Unknown values are refused, never defaulted: a relay that silently
    /// downgraded an unrecognized policy to `any_member` would fail OPEN.
    #[test]
    fn unknown_write_policy_is_refused_not_defaulted() {
        for bad in ["", "AnyMember", "any member", "nobody", "human-only"] {
            assert!(ChannelWritePolicy::from_str(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn channel_names_trim_whitespace_and_drop_all_leading_hashes() {
        assert_eq!(canonical_channel_name("channel"), "channel");
        assert_eq!(canonical_channel_name("#channel"), "channel");
        assert_eq!(canonical_channel_name("###channel"), "channel");
        assert_eq!(canonical_channel_name("  ###channel  "), "channel");
        assert_eq!(canonical_channel_name("# channel"), "channel");
        assert_eq!(canonical_channel_name("### channel  "), "channel");
        assert_eq!(canonical_channel_name("  ###  "), "");
        assert_eq!(canonical_channel_name("# #"), "");
        assert_eq!(canonical_channel_name("### ###"), "");
        assert_eq!(canonical_channel_name("channel#topic"), "channel#topic");
    }
}

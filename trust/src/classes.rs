//! Data classes (arch §7).
//!
//! *"Every object carries classification: public · owner-private ·
//! sensitive · restricted · local-only · credential · derived ·
//! org-confidential."*
//!
//! The point is not the label. §6's routing begins with **eligibility
//! filtering** — *"private data → cloud eliminated"* — and a filter needs
//! something to filter on. Without a class on the object, every rule about
//! what may leave the contour has to be re-derived from context at the
//! moment it matters, which is the moment it gets skipped.
//!
//! The default is `OwnerPrivate`, not `Public`. Anything a person told
//! their robot is theirs until said otherwise, and a classification scheme
//! whose default is the permissive end protects nothing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    /// Freely shareable; nothing about the owner.
    Public,
    /// The default. Theirs, usable by their own robot, may travel to a
    /// model when the turn needs it.
    #[default]
    OwnerPrivate,
    /// Health, money, relationships, anything they would not want quoted.
    /// Still usable, but never volunteered into context it was not asked
    /// for.
    Sensitive,
    /// Must not leave the machine. Usable locally; never in an external
    /// call, whatever the turn needs.
    Restricted,
    /// As restricted, and additionally never synced or backed up off-box.
    LocalOnly,
    /// Keys and tokens. Never in model context under any circumstance --
    /// §7 states this separately and it is enforced separately.
    Credential,
    /// Produced by the robot from other objects; inherits the strictest
    /// class of its sources.
    Derived,
    /// Belongs to an organisation rather than a person.
    OrgConfidential,
}

impl DataClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            DataClass::Public => "public",
            DataClass::OwnerPrivate => "owner_private",
            DataClass::Sensitive => "sensitive",
            DataClass::Restricted => "restricted",
            DataClass::LocalOnly => "local_only",
            DataClass::Credential => "credential",
            DataClass::Derived => "derived",
            DataClass::OrgConfidential => "org_confidential",
        }
    }

    pub fn parse(s: &str) -> Option<DataClass> {
        Some(match s.trim().to_lowercase().as_str() {
            "public" => DataClass::Public,
            "owner_private" | "private" => DataClass::OwnerPrivate,
            "sensitive" => DataClass::Sensitive,
            "restricted" => DataClass::Restricted,
            "local_only" | "local" => DataClass::LocalOnly,
            "credential" => DataClass::Credential,
            "derived" => DataClass::Derived,
            "org_confidential" => DataClass::OrgConfidential,
            _ => return None,
        })
    }

    /// May an object of this class be put in front of an external model?
    ///
    /// The eligibility filter of §6, as one function. Everything that must
    /// stay on the machine answers `false` here, and the one place that
    /// builds model context is the only place that has to ask.
    pub fn may_leave_the_machine(&self) -> bool {
        !matches!(
            self,
            DataClass::Restricted | DataClass::LocalOnly | DataClass::Credential
        )
    }

    /// May it travel to another instance of this robot, or to a backup?
    ///
    /// Weaker than leaving for a model: the owner's own stick is still the
    /// owner's. Only `LocalOnly` and `Credential` refuse.
    pub fn may_travel_to_own_premises(&self) -> bool {
        !matches!(self, DataClass::LocalOnly | DataClass::Credential)
    }

    /// Derived objects inherit the strictest class they were made from --
    /// a summary of a restricted document is a restricted summary, and the
    /// alternative is laundering data through paraphrase.
    pub fn strictest(classes: &[DataClass]) -> DataClass {
        classes
            .iter()
            .copied()
            .max_by_key(|c| c.strictness())
            .unwrap_or_default()
    }

    fn strictness(&self) -> u8 {
        match self {
            DataClass::Public => 0,
            DataClass::Derived => 1,
            DataClass::OwnerPrivate => 2,
            DataClass::OrgConfidential => 3,
            DataClass::Sensitive => 4,
            DataClass::Restricted => 5,
            DataClass::LocalOnly => 6,
            DataClass::Credential => 7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default is the protective end. A scheme whose default is
    /// `public` protects nothing, because the objects nobody classified are
    /// always the majority.
    #[test]
    fn the_default_is_owner_private() {
        assert_eq!(DataClass::default(), DataClass::OwnerPrivate);
        assert!(DataClass::default().may_leave_the_machine());
    }

    /// Item 3's gate: a restricted object cannot enter an external call.
    #[test]
    fn some_classes_never_leave_the_machine() {
        for c in [
            DataClass::Restricted,
            DataClass::LocalOnly,
            DataClass::Credential,
        ] {
            assert!(!c.may_leave_the_machine(), "{}", c.as_str());
        }
        for c in [
            DataClass::Public,
            DataClass::OwnerPrivate,
            DataClass::Sensitive,
            DataClass::Derived,
            DataClass::OrgConfidential,
        ] {
            assert!(c.may_leave_the_machine(), "{}", c.as_str());
        }
    }

    /// The owner's own stick is still the owner's -- travelling to their
    /// own premises is a weaker bar than travelling to a model.
    #[test]
    fn own_premises_is_a_weaker_bar_than_an_external_model() {
        assert!(!DataClass::Restricted.may_leave_the_machine());
        assert!(
            DataClass::Restricted.may_travel_to_own_premises(),
            "restricted means no external call, not no backup"
        );
        assert!(!DataClass::LocalOnly.may_travel_to_own_premises());
        assert!(!DataClass::Credential.may_travel_to_own_premises());
    }

    /// A summary of a restricted document is a restricted summary.
    /// Otherwise paraphrase becomes a laundering channel.
    #[test]
    fn derived_objects_inherit_the_strictest_source() {
        assert_eq!(
            DataClass::strictest(&[DataClass::Public, DataClass::Restricted]),
            DataClass::Restricted
        );
        assert_eq!(
            DataClass::strictest(&[DataClass::OwnerPrivate, DataClass::Sensitive]),
            DataClass::Sensitive
        );
        assert_eq!(DataClass::strictest(&[]), DataClass::OwnerPrivate);
    }

    #[test]
    fn classes_round_trip_through_their_names() {
        for c in [
            DataClass::Public,
            DataClass::OwnerPrivate,
            DataClass::Sensitive,
            DataClass::Restricted,
            DataClass::LocalOnly,
            DataClass::Credential,
            DataClass::Derived,
            DataClass::OrgConfidential,
        ] {
            assert_eq!(DataClass::parse(c.as_str()), Some(c));
        }
    }
}

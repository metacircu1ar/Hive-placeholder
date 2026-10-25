//! Filing names and policies; recognition includes bare names, membership does not.
use super::{
    Slug,
    limits::{ORDINARY_HALF_LIFE_DAYS, SHORT_HALF_LIFE_DAYS, STABLE_HALF_LIFE_DAYS},
};
use crate::Result;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Knowledge,
    Capture,
    Review,
}
struct Policy {
    name: &'static str,
    description: &'static str,
    half_life_days: f64,
    purpose: Purpose,
}
macro_rules! prefixes {
    ($($variant:ident => ($name:literal,
         $description:literal,
         $half_life:expr,
         $purpose:ident)),
        + $(,
        )?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Prefix { $($variant),+ }
        impl Prefix {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            const fn policy(self) -> Policy {
                match self {
                    $(Self::$variant => Policy {
                        name: $name,
                        description: $description,
                        half_life_days: $half_life,
                        purpose: Purpose::$purpose,
                    }),+
                }
            }
        }
    };
}
prefixes! {
    Components => ("components",
         "A module, service, binary or file group",
         ORDINARY_HALF_LIFE_DAYS,
         Knowledge),

    Decisions => ("decisions",
         "One decision, its reasons and its alternatives",
         STABLE_HALF_LIFE_DAYS,
         Knowledge),

    Incidents => ("incidents",
         "A failure, its cause and its fix",
         ORDINARY_HALF_LIFE_DAYS,
         Knowledge),

    Concepts => ("concepts",
         "A reusable idea, pattern or technique",
         STABLE_HALF_LIFE_DAYS,
         Knowledge),

    People => ("people", "Humans and agent roles", ORDINARY_HALF_LIFE_DAYS, Knowledge),
    Operations => ("operations", "How agents should work: user preferences, conventions, \
        procedures", STABLE_HALF_LIFE_DAYS, Knowledge),
    Inbox => ("inbox", "Unprocessed captures", SHORT_HALF_LIFE_DAYS, Capture),
    Review => ("review", "Proposals waiting for a decision", ORDINARY_HALF_LIFE_DAYS, Review),
    Reports => ("reports", "Generated reports, such as curator runs and doctor output",
         SHORT_HALF_LIFE_DAYS,
         Knowledge),

}
impl Prefix {
    pub const fn as_str(self) -> &'static str {
        self.policy().name
    }
    pub const fn description(self) -> &'static str {
        self.policy().description
    }
    pub fn recognize(slug: &Slug) -> Option<Self> {
        let name = slug.as_str().split('/').next()?;
        Self::ALL
            .iter()
            .copied()
            .find(|prefix| prefix.as_str() == name)
    }
    pub fn contains_descendant(self, slug: &Slug) -> bool {
        slug.as_str()
            .strip_prefix(self.as_str())
            .is_some_and(|tail| tail.starts_with('/'))
    }
    pub fn half_life_days(slug: &Slug) -> f64 {
        Self::recognize(slug).map_or(ORDINARY_HALF_LIFE_DAYS, |prefix| {
            prefix.policy().half_life_days
        })
    }
    fn has_purpose(slug: &Slug, purpose: Purpose) -> bool {
        Self::recognize(slug).is_some_and(|prefix| {
            prefix.policy().purpose == purpose && prefix.contains_descendant(slug)
        })
    }
    pub fn is_inbox(slug: &Slug) -> bool {
        Self::has_purpose(slug, Purpose::Capture)
    }
    pub fn is_review(slug: &Slug) -> bool {
        Self::has_purpose(slug, Purpose::Review)
    }
    pub fn capture_slug(suffix: &str) -> Result<Slug> {
        format!("{}/{suffix}", Self::Inbox.as_str()).parse()
    }
}

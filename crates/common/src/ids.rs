//! Strongly-typed identifier newtypes. Leveraging the type system to
//! prevent passing a `UserId` where a `NoteId` is expected at compile time.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
            pub fn from_uuid(u: Uuid) -> Self {
                Self(u)
            }
            pub fn into_uuid(self) -> Uuid {
                self.0
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }
        impl From<$name> for Uuid {
            fn from(v: $name) -> Uuid {
                v.0
            }
        }
        async_graphql::scalar!($name);
    };
}

id_newtype!(UserId);
id_newtype!(NoteId);
id_newtype!(SessionId);
id_newtype!(EventId);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinct_types() {
        let u = UserId::new();
        let n = NoteId::new();
        assert_ne!(u.into_uuid(), n.into_uuid());
        // Compile-time guarantee: `let _: UserId = n;` would fail.
    }
}


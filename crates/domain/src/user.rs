use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tn_common::prelude::*;

/// Aggregate root for an authenticated user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub fn builder() -> UserBuilder {
        UserBuilder::default()
    }
}

#[derive(Default)]
pub struct UserBuilder {
    id: Option<UserId>,
    email: Option<String>,
    display_name: Option<String>,
    password_hash: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

impl UserBuilder {
    pub fn id(mut self, v: UserId) -> Self { self.id = Some(v); self }
    pub fn email(mut self, v: impl Into<String>) -> Self { self.email = Some(v.into()); self }
    pub fn display_name(mut self, v: impl Into<String>) -> Self { self.display_name = Some(v.into()); self }
    pub fn password_hash(mut self, v: impl Into<String>) -> Self { self.password_hash = Some(v.into()); self }
    pub fn created_at(mut self, v: DateTime<Utc>) -> Self { self.created_at = Some(v); self }
    pub fn build(self) -> AppResult<User> {
        let email = self.email.ok_or_else(|| AppError::Validation("email required".into()))?;
        if !email.contains('@') {
            return Err(AppError::Validation("invalid email".into()));
        }
        let display_name = self.display_name.ok_or_else(|| AppError::Validation("display_name required".into()))?;
        if display_name.trim().is_empty() {
            return Err(AppError::Validation("display_name empty".into()));
        }
        let password_hash = self.password_hash.ok_or_else(|| AppError::Validation("password_hash required".into()))?;
        Ok(User {
            id: self.id.unwrap_or_default(),
            email: email.to_lowercase(),
            display_name,
            password_hash,
            created_at: self.created_at.unwrap_or_else(Utc::now),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_validates_email() {
        let r = User::builder().email("noatsign").display_name("A").password_hash("h").build();
        assert!(r.is_err());
    }
    #[test]
    fn builder_lowercases_email() {
        let u = User::builder().email("Foo@Bar.com").display_name("A").password_hash("h").build().unwrap();
        assert_eq!(u.email, "foo@bar.com");
    }
}


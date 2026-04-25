//! JWT issuance and verification.

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use tn_common::error::{AppError, AppResult};
use tn_common::ids::UserId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
}

#[derive(Clone)]
pub struct JwtIssuer {
    secret: Vec<u8>,
    ttl_secs: i64,
}

impl JwtIssuer {
    pub fn new(secret: impl Into<Vec<u8>>, ttl_secs: i64) -> Self {
        Self {
            secret: secret.into(),
            ttl_secs,
        }
    }
    pub fn issue(&self, user: UserId) -> AppResult<String> {
        let now = Utc::now();
        let claims = Claims {
            sub: user.to_string(),
            iat: now.timestamp(),
            exp: (now + Duration::seconds(self.ttl_secs)).timestamp(),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|e| AppError::Internal(e.to_string()))
    }
    pub fn verify(&self, token: &str) -> AppResult<Claims> {
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(&self.secret),
            &Validation::default(),
        )
        .map_err(|e| AppError::Unauthorized(e.to_string()))?;
        Ok(data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn issue_and_verify() {
        let j = JwtIssuer::new(b"secret".to_vec(), 60);
        let u = UserId::new();
        let t = j.issue(u).unwrap();
        let c = j.verify(&t).unwrap();
        assert_eq!(c.sub, u.to_string());
    }
    #[test]
    fn rejects_bad_token() {
        let j = JwtIssuer::new(b"secret".to_vec(), 60);
        assert!(j.verify("not-a-token").is_err());
    }
}

//! Authentication service: registration, login, token verification.
//! Generic over `UserRepo` so the same code runs against in-memory or SQL.

use std::sync::Arc;
use tn_common::error::{AppError, AppResult};
use tn_common::ids::UserId;
use tn_domain::user::User;
use tn_infra::auth::JwtIssuer;
use tn_infra::password;
use tn_infra::repos::UserRepo;

#[derive(Clone)]
pub struct AuthService<R: UserRepo> {
    repo: Arc<R>,
    jwt: JwtIssuer,
}

#[derive(Debug, Clone)]
pub struct AuthOutcome {
    pub user: User,
    pub token: String,
}

impl<R: UserRepo + 'static> AuthService<R> {
    pub fn new(repo: Arc<R>, jwt: JwtIssuer) -> Self {
        Self { repo, jwt }
    }

    pub async fn register(
        &self,
        email: &str,
        display_name: &str,
        password: &str,
    ) -> AppResult<AuthOutcome> {
        if password.len() < 8 {
            return Err(AppError::Validation("password must be ≥ 8 chars".into()));
        }
        let hash = password::hash(password)?;
        let user = User::builder()
            .email(email)
            .display_name(display_name)
            .password_hash(hash)
            .build()?;
        self.repo.create(&user).await?;
        let token = self.jwt.issue(user.id)?;
        Ok(AuthOutcome { user, token })
    }

    pub async fn login(&self, email: &str, password: &str) -> AppResult<AuthOutcome> {
        let user = self
            .repo
            .by_email(email)
            .await?
            .ok_or_else(|| AppError::Unauthorized("invalid credentials".into()))?;
        if !password::verify(password, &user.password_hash)? {
            return Err(AppError::Unauthorized("invalid credentials".into()));
        }
        let token = self.jwt.issue(user.id)?;
        Ok(AuthOutcome { user, token })
    }

    pub fn verify(&self, token: &str) -> AppResult<UserId> {
        let claims = self.jwt.verify(token)?;
        let uuid = uuid::Uuid::parse_str(&claims.sub)
            .map_err(|e| AppError::Unauthorized(e.to_string()))?;
        Ok(UserId::from_uuid(uuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tn_infra::repos::InMemoryUserRepo;

    fn svc() -> AuthService<InMemoryUserRepo> {
        AuthService::new(Arc::new(InMemoryUserRepo::default()), JwtIssuer::new(b"k".to_vec(), 60))
    }

    #[tokio::test]
    async fn register_login_round_trip() {
        let s = svc();
        let r = s.register("a@b.com", "A", "passw0rd!").await.unwrap();
        let l = s.login("a@b.com", "passw0rd!").await.unwrap();
        assert_eq!(r.user.id, l.user.id);
        let uid = s.verify(&l.token).unwrap();
        assert_eq!(uid, l.user.id);
    }

    #[tokio::test]
    async fn rejects_short_password() {
        assert!(svc().register("a@b.com", "A", "short").await.is_err());
    }

    #[tokio::test]
    async fn login_rejects_wrong_password() {
        let s = svc();
        s.register("a@b.com", "A", "passw0rd!").await.unwrap();
        assert!(s.login("a@b.com", "nope nope").await.is_err());
    }

    #[tokio::test]
    async fn login_rejects_unknown_user() {
        assert!(svc().login("nobody@x.com", "passw0rd!").await.is_err());
    }
}


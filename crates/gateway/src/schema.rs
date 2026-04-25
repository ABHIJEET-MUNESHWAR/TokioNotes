//! GraphQL schema. Query, Mutation and Subscription operations are all
//! exposed here; subscriptions stream live CRDT updates to clients.

use async_graphql::futures_util::Stream;
use async_graphql::*;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use tn_common::ids::{NoteId, UserId};
use tn_domain::note::Role;
use tn_infra::repos::{AclRepo, UserRepo};

use crate::app::AppState;

// ---------- GraphQL output types --------------------------------------------

#[derive(SimpleObject, Clone)]
pub struct UserDto {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(SimpleObject, Clone)]
pub struct CollaboratorDto {
    pub user_id: UserId,
    pub role: Role,
    pub display_name: String,
    pub email: String,
}

#[derive(SimpleObject, Clone)]
pub struct NoteDto {
    pub id: NoteId,
    pub owner_id: UserId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
    /// Caller's effective role on this note (Owner / Editor / Viewer).
    pub my_role: Role,
    /// Base64-encoded Y-CRDT state (latest snapshot).
    pub snapshot_b64: String,
}

#[derive(SimpleObject, Clone)]
pub struct AuthPayload {
    pub user: UserDto,
    pub token: String,
}

#[derive(SimpleObject, Clone)]
pub struct OpEvent {
    pub note_id: NoteId,
    /// Base64-encoded Y-CRDT update.
    pub update_b64: String,
}

#[derive(SimpleObject, Clone)]
pub struct AgentReportDto {
    pub summary: String,
    pub tags: Vec<String>,
}

// ---------- Auth helper -----------------------------------------------------

fn current_user(ctx: &Context<'_>) -> Result<UserId> {
    let token = ctx
        .data_opt::<AuthToken>()
        .ok_or_else(|| Error::new("missing Authorization header"))?;
    let st = ctx.data::<AppState>()?;
    st.auth
        .verify(&token.0)
        .map_err(|e| Error::new(format!("auth: {e}")))
}

#[derive(Clone)]
pub struct AuthToken(pub String);

// ---------- Query -----------------------------------------------------------

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn health(&self) -> &'static str {
        "ok"
    }

    async fn me(&self, ctx: &Context<'_>) -> Result<UserDto> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let u = st.auth_user(uid).await?;
        Ok(u)
    }

    async fn note(&self, ctx: &Context<'_>, id: NoteId) -> Result<NoteDto> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let n = st.notes.note(uid, id).await.map_err(to_gql)?;
        Ok(st.note_dto(n, uid).await)
    }

    async fn my_notes(&self, ctx: &Context<'_>) -> Result<Vec<NoteDto>> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let notes = st.notes.list_for(uid).await.map_err(to_gql)?;
        let mut out = Vec::with_capacity(notes.len());
        for n in notes {
            out.push(st.note_dto(n, uid).await);
        }
        Ok(out)
    }

    async fn collaborators(&self, ctx: &Context<'_>, id: NoteId) -> Result<Vec<CollaboratorDto>> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let acls = st.notes.collaborators(uid, id).await.map_err(to_gql)?;
        let mut out = Vec::with_capacity(acls.len());
        for a in acls {
            out.push(st.collaborator_dto(a).await);
        }
        Ok(out)
    }

    async fn ai_summary(&self, ctx: &Context<'_>, id: NoteId) -> Result<String> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let _ = st.notes.note(uid, id).await.map_err(to_gql)?;
        let body = match st.rooms.get(id) {
            Some(r) => r.body_text().await,
            None => String::new(),
        };
        Ok(st.ai.summarize(&body).await.map_err(to_gql)?.text)
    }

    async fn ai_report(
        &self,
        ctx: &Context<'_>,
        id: NoteId,
        instruction: String,
    ) -> Result<AgentReportDto> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let _ = st.notes.note(uid, id).await.map_err(to_gql)?;
        let body = match st.rooms.get(id) {
            Some(r) => r.body_text().await,
            None => String::new(),
        };
        let r = st
            .ai
            .agent_report(&body, &instruction)
            .await
            .map_err(to_gql)?;
        Ok(AgentReportDto {
            summary: r.summary.text,
            tags: r.tags,
        })
    }
}

// ---------- Mutation --------------------------------------------------------

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn register(
        &self,
        ctx: &Context<'_>,
        email: String,
        display_name: String,
        password: String,
    ) -> Result<AuthPayload> {
        let st = ctx.data::<AppState>()?;
        let r = st
            .auth
            .register(&email, &display_name, &password)
            .await
            .map_err(to_gql)?;
        Ok(AuthPayload {
            user: UserDto {
                id: r.user.id,
                email: r.user.email,
                display_name: r.user.display_name,
                created_at: r.user.created_at,
            },
            token: r.token,
        })
    }

    async fn login(
        &self,
        ctx: &Context<'_>,
        email: String,
        password: String,
    ) -> Result<AuthPayload> {
        let st = ctx.data::<AppState>()?;
        let r = st.auth.login(&email, &password).await.map_err(to_gql)?;
        Ok(AuthPayload {
            user: UserDto {
                id: r.user.id,
                email: r.user.email,
                display_name: r.user.display_name,
                created_at: r.user.created_at,
            },
            token: r.token,
        })
    }

    async fn create_note(&self, ctx: &Context<'_>, title: String) -> Result<NoteDto> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let n = st.notes.create(uid, title).await.map_err(to_gql)?;
        let _ = st.rooms.get_or_create(n.id);
        Ok(st.note_dto(n, uid).await)
    }

    async fn rename_note(&self, ctx: &Context<'_>, id: NoteId, title: String) -> Result<NoteDto> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let n = st.notes.rename(uid, id, title).await.map_err(to_gql)?;
        Ok(st.note_dto(n, uid).await)
    }

    async fn delete_note(&self, ctx: &Context<'_>, id: NoteId) -> Result<bool> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        st.notes.delete(uid, id).await.map_err(to_gql)?;
        st.rooms.close(id);
        Ok(true)
    }

    async fn share_note(
        &self,
        ctx: &Context<'_>,
        id: NoteId,
        email: String,
        role: Role,
    ) -> Result<CollaboratorDto> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let acl = st
            .notes
            .share(uid, id, &email, role)
            .await
            .map_err(to_gql)?;
        Ok(st.collaborator_dto(acl).await)
    }

    async fn revoke_share(&self, ctx: &Context<'_>, id: NoteId, user_id: UserId) -> Result<bool> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        st.notes.revoke(uid, id, user_id).await.map_err(to_gql)?;
        Ok(true)
    }

    /// Apply a base64-encoded Y-CRDT update to a note. Returns the new
    /// snapshot (base64). Requires at least `Editor` role; viewers are
    /// rejected with `Forbidden`.
    async fn apply_ops(
        &self,
        ctx: &Context<'_>,
        note_id: NoteId,
        update_b64: String,
    ) -> Result<String> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        // Permission gate — must be editor or owner.
        let _ = st.notes.note_for_edit(uid, note_id).await.map_err(to_gql)?;
        let bytes = B64
            .decode(update_b64.as_bytes())
            .map_err(|e| Error::new(e.to_string()))?;
        let room = st.rooms.get_or_create(note_id);
        room.apply(&bytes).await.map_err(to_gql)?;
        Ok(B64.encode(room.snapshot().await))
    }
}

// ---------- Subscription ----------------------------------------------------

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Stream Y-CRDT updates as they are applied to a note's room.
    async fn note_ops(
        &self,
        ctx: &Context<'_>,
        note_id: NoteId,
    ) -> Result<impl Stream<Item = OpEvent>> {
        let uid = current_user(ctx)?;
        let st = ctx.data::<AppState>()?;
        let _ = st.notes.note(uid, note_id).await.map_err(to_gql)?;
        let room = st.rooms.get_or_create(note_id);
        let rx = room.subscribe();
        let s = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|r| async move {
            r.ok().map(|f| OpEvent {
                note_id: f.note,
                update_b64: B64.encode(&f.update),
            })
        });
        Ok(s)
    }
}

// ---------- Helpers ---------------------------------------------------------

impl AppState {
    pub async fn note_dto(&self, n: tn_domain::note::Note, actor: UserId) -> NoteDto {
        let snap = match self.rooms.get(n.id) {
            Some(r) => r.snapshot().await,
            None => Vec::new(),
        };
        let my_role = if n.owner_id == actor {
            Role::Owner
        } else {
            self.notes
                .acls
                .role_of(n.id, actor)
                .await
                .ok()
                .flatten()
                .unwrap_or(Role::Viewer)
        };
        NoteDto {
            id: n.id,
            owner_id: n.owner_id,
            title: n.title,
            created_at: n.created_at,
            updated_at: n.updated_at,
            version: n.version,
            my_role,
            snapshot_b64: B64.encode(snap),
        }
    }
    pub async fn auth_user(&self, uid: UserId) -> Result<UserDto> {
        let u = self
            .users
            .by_id(uid)
            .await
            .map_err(to_gql)?
            .ok_or_else(|| Error::new("user not found"))?;
        Ok(UserDto {
            id: u.id,
            email: u.email,
            display_name: u.display_name,
            created_at: u.created_at,
        })
    }

    /// Build a `CollaboratorDto` enriched with the user's display name and
    /// email. Falls back to empty strings if the user can no longer be
    /// resolved (e.g. they were deleted but the ACL row still exists).
    pub async fn collaborator_dto(&self, acl: tn_domain::note::NoteAcl) -> CollaboratorDto {
        let (display_name, email) = match self.users.by_id(acl.user_id).await {
            Ok(Some(u)) => (u.display_name, u.email),
            _ => (String::new(), String::new()),
        };
        CollaboratorDto {
            user_id: acl.user_id,
            role: acl.role,
            display_name,
            email,
        }
    }
}

fn to_gql(e: tn_common::error::AppError) -> Error {
    Error::new(e.to_string())
}

pub type AppSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub fn build_schema(state: AppState) -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .data(state)
        .finish()
}

use futures::StreamExt;

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::Request;
    use serde_json::json;

    fn schema_and_state() -> (AppSchema, AppState) {
        let st = AppState::bootstrap("test-secret");
        (build_schema(st.clone()), st)
    }

    async fn token_for(schema: &AppSchema, email: &str) -> String {
        let req = Request::new(
            r#"mutation($e:String!){register(email:$e,displayName:"x",password:"passw0rd!"){token}}"#
        ).variables(Variables::from_json(json!({"e": email})));
        let r = schema.execute(req).await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let v: serde_json::Value = serde_json::to_value(&r.data).unwrap();
        v["register"]["token"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn end_to_end_create_share_collaborate() {
        let (schema, _st) = schema_and_state();
        let alice = token_for(&schema, "alice@x.com").await;
        let _bob = token_for(&schema, "bob@x.com").await;

        // Alice creates a note
        let req = Request::new(r#"mutation{createNote(title:"hello"){id title}}"#)
            .data(AuthToken(alice.clone()));
        let r = schema.execute(req).await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let v = serde_json::to_value(&r.data).unwrap();
        let note_id = v["createNote"]["id"].as_str().unwrap().to_string();

        // Alice shares with Bob
        let req = Request::new(format!(
            r#"mutation{{shareNote(id:"{nid}",email:"bob@x.com",role:EDITOR){{userId role}}}}"#,
            nid = note_id
        ))
        .data(AuthToken(alice.clone()));
        let r = schema.execute(req).await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);

        // Alice lists notes
        let req = Request::new(r#"{ myNotes { id title } }"#).data(AuthToken(alice));
        let r = schema.execute(req).await;
        assert!(r.errors.is_empty());
        let v = serde_json::to_value(&r.data).unwrap();
        assert_eq!(v["myNotes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unauthenticated_query_fails() {
        let (schema, _) = schema_and_state();
        let r = schema.execute(Request::new(r#"{ myNotes { id } }"#)).await;
        assert!(!r.errors.is_empty());
    }

    #[tokio::test]
    async fn apply_ops_streams_via_subscription() {
        use futures::StreamExt;
        let (schema, _) = schema_and_state();
        let alice = token_for(&schema, "alice2@x.com").await;
        let req =
            Request::new(r#"mutation{createNote(title:"t"){id}}"#).data(AuthToken(alice.clone()));
        let r = schema.execute(req).await;
        let nid = serde_json::to_value(&r.data).unwrap()["createNote"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Pre-create the room so the broadcast channel exists before the
        // subscription resolver attaches its receiver.
        {
            let st_ref = schema
                .execute(
                    Request::new(format!(r#"{{ note(id:"{}"){{ id }} }}"#, nid))
                        .data(AuthToken(alice.clone())),
                )
                .await;
            assert!(st_ref.errors.is_empty());
        }

        let sub_req = Request::new(format!(
            r#"subscription{{ noteOps(noteId:"{}") {{ noteId updateB64 }} }}"#,
            nid
        ))
        .data(AuthToken(alice.clone()));
        let mut stream = schema.execute_stream(sub_req);

        // Drive the subscription so the receiver is registered before we publish.
        let upd = tn_collab_service::local_insert_update(&[], 0, "hi").unwrap();
        let upd_b64 = B64.encode(&upd);
        let schema2 = schema.clone();
        let alice2 = alice.clone();
        let nid2 = nid.clone();
        let upd2 = upd_b64.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let m = Request::new(format!(
                r#"mutation($u:String!){{ applyOps(noteId:"{}",updateB64:$u) }}"#,
                nid2
            ))
            .variables(Variables::from_json(json!({"u": upd2})))
            .data(AuthToken(alice2));
            let _ = schema2.execute(m).await;
        });

        let next = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap();
        let resp = next.expect("stream item");
        assert!(resp.errors.is_empty(), "{:?}", resp.errors);
    }
}

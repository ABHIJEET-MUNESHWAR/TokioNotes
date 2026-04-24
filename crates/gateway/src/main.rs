use actix_cors::Cors;
use actix_web::{guard, web, App, HttpRequest, HttpResponse, HttpServer};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql::Data;
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use tn_gateway::app::AppState;
use tn_gateway::schema::{build_schema, AppSchema, AuthToken};

async fn graphql(
    schema: web::Data<AppSchema>,
    http_req: HttpRequest,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let mut req = req.into_inner();
    if let Some(h) = http_req.headers().get("authorization") {
        if let Ok(s) = h.to_str() {
            let token = s.trim_start_matches("Bearer ").trim().to_string();
            req = req.data(AuthToken(token));
        }
    }
    schema.execute(req).await.into()
}

/// Extract a bearer token from a `graphql-transport-ws` `connection_init`
/// payload. The frontend sends `connectionParams: { authorization: "Bearer …" }`
/// so we accept either the `authorization` or `Authorization` key.
fn token_from_init(value: &serde_json::Value) -> Option<String> {
    let raw = value
        .get("authorization")
        .or_else(|| value.get("Authorization"))?
        .as_str()?;
    Some(raw.trim_start_matches("Bearer ").trim().to_string())
}

async fn graphql_ws(
    schema: web::Data<AppSchema>,
    req: HttpRequest,
    payload: web::Payload,
) -> actix_web::Result<HttpResponse> {
    GraphQLSubscription::new(schema.get_ref().clone())
        .on_connection_init(|value| async move {
            let mut data = Data::default();
            match token_from_init(&value) {
                Some(tok) => {
                    tracing::debug!("ws connection_init received token");
                    data.insert(AuthToken(tok));
                }
                None => {
                    tracing::warn!("ws connection_init had no authorization payload");
                }
            }
            Ok(data)
        })
        .start(&req, payload)
}

async fn playground() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(playground_source(
            GraphQLPlaygroundConfig::new("/graphql").subscription_endpoint("/graphql"),
        ))
}

async fn health() -> &'static str { "ok" }

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tn_common::telemetry::init("tn-gateway");
    let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret-change-me".into());
    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:9090".into());
    let state = AppState::bootstrap(&secret);
    let schema = build_schema(state);

    tracing::info!(%bind, "starting tn-gateway");

    HttpServer::new(move || {
        App::new()
            .wrap(Cors::permissive())
            .app_data(web::Data::new(schema.clone()))
            .route("/health", web::get().to(health))
            .route("/", web::get().to(playground))
            // Single `/graphql` resource that handles both HTTP queries
            // (POST) and WebSocket subscription upgrades (GET + Upgrade
            // header). Registering them as two separate `service(...)`
            // resources leaves the GET route unreachable in Actix.
            .service(
                web::resource("/graphql")
                    .route(web::post().to(graphql))
                    .route(
                        web::get()
                            .guard(guard::Header("upgrade", "websocket"))
                            .to(graphql_ws),
                    ),
            )
    })
    .bind(bind)?
    .run()
    .await
}


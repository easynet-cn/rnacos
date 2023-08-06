use std::sync::Arc;

use actix_web::{web::{Data, Json}, Responder, HttpResponse, http};

use crate::{common::appdata::AppShareData};

use super::{model::{RouterRequest}, handle_route};


pub async fn route_request(app: Data<Arc<AppShareData>>,req: Json<RouterRequest>) -> impl Responder{
    match handle_route(app.as_ref(),req.0).await {
        Ok(res) => {
            let v = serde_json::to_string(&res).unwrap();
            HttpResponse::Ok()
                .insert_header(http::header::ContentType(mime::APPLICATION_JSON))
                .body(v)
        },
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use crate::common::appdata::AppShareData;
use crate::grpc::nacos_proto::Payload;
use crate::grpc::{PayloadHandler, PayloadUtils, RequestMeta};

pub struct RaftVoteRequestHandler {
    app_data: Arc<AppShareData>, 
}

impl RaftVoteRequestHandler{
    pub fn new(app_data: Arc<AppShareData>) -> Self {
        Self { app_data }
    }
}

#[async_trait]
impl PayloadHandler for RaftVoteRequestHandler {
    async fn handle(&self, request_payload: Payload, _request_meta: RequestMeta) -> anyhow::Result<Payload> {
        let body_vec = request_payload.body.unwrap_or_default().value;
        let request: async_raft::raft::VoteRequest = serde_json::from_slice(&body_vec)?;
        let res = self.app_data.raft.vote(request).await?;
        let value = serde_json::to_string(&res)?;
        //log::info!("RaftVoteRequestHandler result:{}",&value);
        let payload = PayloadUtils::build_payload("RaftVoteResponse",value );
        Ok(payload)
    }
}
use std::{collections::HashMap, convert::TryInto, path::Path, sync::Arc};

use actix::prelude::*;
use quick_protobuf::{BytesReader, Writer};
use tokio::{
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use crate::common::{
    byte_utils::{bin_to_id, id_to_bin},
    protobuf_utils::FileMessageReader,
};

use super::{
    log::{LogRange, RaftIndex, SnapshotRange},
    model::RaftIndexDto,
};

pub struct RaftIndexInnerManager {
    file: tokio::fs::File,
    pub(crate) raft_index: RaftIndexDto,
    pub(crate) last_applied_log: u64,
}

impl RaftIndexInnerManager {
    pub async fn init(path: &str) -> anyhow::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .await?;
        let meta = file.metadata().await?;
        let (last_applied_log, raft_index) = if meta.len() == 0 {
            log::info!("RaftIndexInnerManager init index file");
            //init write
            let mut index = RaftIndex::default();
            index.logs.push(LogRange {
                id: 0,
                start_index: 0,
                pre_term: 0,
                record_count: 0,
                is_close: false,
                mark_remove: false,
            });
            let mut buf = Vec::new();
            let mut writer = Writer::new(&mut buf);
            writer.write_message(&index)?;
            let header_buf = id_to_bin(0);
            file.write(&header_buf).await?;
            file.write(&buf).await?;
            file.flush().await?;
            let raft_index: RaftIndexDto = index.try_into()?;
            (0, raft_index)
        } else {
            log::info!("RaftIndexInnerManager load index file");
            //read
            let mut header_buf = vec![0u8; 8];
            file.read(&mut header_buf).await?;
            let last_applied_log = bin_to_id(&mut header_buf);
            let mut file_reader = FileMessageReader::new(file.try_clone().await?, 8);
            let buf = file_reader.read_next().await?;
            let mut reader = BytesReader::from_bytes(&buf);
            let index: RaftIndex = reader.read_message(&buf)?;
            let raft_index: RaftIndexDto = index.try_into()?;
            (last_applied_log, raft_index)
        };
        log::info!("load_raft_index,logs len:{}", raft_index.logs.len());
        Ok(Self {
            file,
            raft_index,
            last_applied_log,
        })
    }

    pub async fn write_last_applied_log(&mut self, last_applied_log: u64) -> anyhow::Result<()> {
        self.last_applied_log = last_applied_log;
        self.file.seek(std::io::SeekFrom::Start(0)).await?;
        self.file.write(&id_to_bin(last_applied_log)).await?;
        Ok(())
    }

    pub async fn write_index(&mut self, index: RaftIndexDto) -> anyhow::Result<()> {
        self.raft_index = index;
        self.file.seek(std::io::SeekFrom::Start(8)).await?;
        let mut buf = Vec::new();
        let mut writer = Writer::new(&mut buf);
        let index_do = self.raft_index.to_record_do();
        writer.write_message(&index_do)?;
        self.file.write(&buf).await?;
        self.file.flush().await?;
        Ok(())
    }
}

pub struct RaftIndexManager {
    path: Arc<String>,
    inner: Option<Box<RaftIndexInnerManager>>,
}

impl RaftIndexManager {
    pub fn new(path: Arc<String>) -> Self {
        Self { path, inner: None }
    }

    pub fn init(&mut self, ctx: &mut Context<Self>) {
        let path = Path::new(self.path.as_str())
            .join("index")
            .to_string_lossy()
            .into_owned();
        async move { RaftIndexInnerManager::init(&path).await }
            .into_actor(self)
            .map(|r, act, ctx| match r {
                Ok(v) => {
                    act.inner = Some(Box::new(v));
                }
                Err(e) => {
                    log::error!("RaftIndexManager init error,{}", e);
                    ctx.stop();
                }
            })
            .wait(ctx);
    }

    fn inner_is_empty_error() -> anyhow::Error {
        anyhow::anyhow!("inner is empty")
    }

    pub fn load_index_info(&self) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_ref() {
            Ok(RaftIndexResponse::RaftIndexInfo {
                raft_index: inner.raft_index.clone(),
                last_applied_log: inner.last_applied_log,
            })
        } else {
            Err(Self::inner_is_empty_error())
        }
    }

    pub fn write_last_applied_log(
        &mut self,
        ctx: &mut Context<Self>,
        last_applied_log: u64,
    ) -> anyhow::Result<RaftIndexResponse> {
        if self.inner.is_none() {
            return Err(Self::inner_is_empty_error());
        }
        let mut inner = self.inner.take();
        async move {
            if let Some(v) = &mut inner {
                match v.write_last_applied_log(last_applied_log).await {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("write_last_applied_log error,{}", err)
                    }
                }
            }
            inner
        }
        .into_actor(self)
        .map(|v, act, _ctx| {
            act.inner = v;
        })
        .wait(ctx);
        Ok(RaftIndexResponse::None)
    }

    pub fn write_index(
        &mut self,
        ctx: &mut Context<Self>,
        index: RaftIndexDto,
    ) -> anyhow::Result<RaftIndexResponse> {
        if self.inner.is_none() {
            return Err(Self::inner_is_empty_error());
        }
        let mut inner = self.inner.take();
        async move {
            if let Some(v) = &mut inner {
                match v.write_index(index).await {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("write_index error,{}", err)
                    }
                }
            }
            inner
        }
        .into_actor(self)
        .map(|v, act, _ctx| {
            act.inner = v;
        })
        .wait(ctx);
        Ok(RaftIndexResponse::None)
    }

    pub fn write_logs(
        &mut self,
        ctx: &mut Context<Self>,
        logs: Vec<LogRange>,
    ) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_mut() {
            inner.raft_index.logs = logs;
            let index_info = inner.raft_index.clone();
            self.write_index(ctx, index_info)
        } else {
            Err(Self::inner_is_empty_error())
        }
    }

    pub fn write_snapshots(
        &mut self,
        ctx: &mut Context<Self>,
        snapshots: Vec<SnapshotRange>,
    ) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_mut() {
            inner.raft_index.snapshots = snapshots;
            let index_info = inner.raft_index.clone();
            self.write_index(ctx, index_info)
        } else {
            Err(Self::inner_is_empty_error())
        }
    }

    pub fn write_member(
        &mut self,
        ctx: &mut Context<Self>,
        member: Vec<u64>,
        member_after_consensus: Vec<u64>,
        node_addr: Option<HashMap<u64, Arc<String>>>,
    ) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_mut() {
            inner.raft_index.member = member;
            inner.raft_index.member_after_consensus = member_after_consensus;
            if let Some(node_addr) = node_addr {
                inner.raft_index.node_addrs = node_addr;
            }
            let index_info = inner.raft_index.clone();
            self.write_index(ctx, index_info)
        } else {
            Err(Self::inner_is_empty_error())
        }
    }

    pub fn write_node_addr(
        &mut self,
        ctx: &mut Context<Self>,
        node_addr: HashMap<u64, Arc<String>>,
    ) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_mut() {
            inner.raft_index.node_addrs = node_addr;
            let index_info = inner.raft_index.clone();
            self.write_index(ctx, index_info)
        } else {
            Err(Self::inner_is_empty_error())
        }
    }

    pub fn add_node_addr(
        &mut self,
        ctx: &mut Context<Self>,
        id: u64,
        node_addr: Arc<String>,
    ) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_mut() {
            inner.raft_index.node_addrs.insert(id, node_addr);
            let index_info = inner.raft_index.clone();
            self.write_index(ctx, index_info)
        } else {
            Err(Self::inner_is_empty_error())
        }
    }

    pub fn write_hard_state(
        &mut self,
        ctx: &mut Context<Self>,
        current_term: u64,
        voted_for: u64,
    ) -> anyhow::Result<RaftIndexResponse> {
        if let Some(inner) = self.inner.as_mut() {
            inner.raft_index.current_term = current_term;
            inner.raft_index.voted_for = voted_for;
            let index_info = inner.raft_index.clone();
            self.write_index(ctx, index_info)
        } else {
            Err(Self::inner_is_empty_error())
        }
    }
}

impl Actor for RaftIndexManager {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        log::info!("RaftIndexManager started");
        self.init(ctx);
    }
}

#[derive(Message, Debug)]
#[rtype(result = "anyhow::Result<RaftIndexResponse>")]
pub enum RaftIndexRequest {
    LoadIndexInfo,
    LoadHardState,
    LoadMember,
    GetTargetAddr(u64),
    SaveLogs(Vec<LogRange>),
    SaveSnapshots(Vec<SnapshotRange>),
    SaveLastAppliedLog(u64),
    SaveMember {
        member: Vec<u64>,
        member_after_consensus: Vec<u64>,
        node_addr: Option<HashMap<u64, Arc<String>>>,
    },
    SaveNodeAddr(HashMap<u64, Arc<String>>),
    AddNodeAddr(u64, Arc<String>),
    SaveHardState {
        current_term: u64,
        voted_for: u64,
    },
}

pub enum RaftIndexResponse {
    None,
    RaftIndexInfo {
        raft_index: RaftIndexDto,
        last_applied_log: u64,
    },
    HardState {
        current_term: u64,
        voted_for: u64,
    },
    MemberShip {
        member: Vec<u64>,
        member_after_consensus: Vec<u64>,
        node_addrs: HashMap<u64, Arc<String>>,
    },
    TargetAddr(Option<Arc<String>>),
}

impl Handler<RaftIndexRequest> for RaftIndexManager {
    type Result = anyhow::Result<RaftIndexResponse>;

    fn handle(&mut self, msg: RaftIndexRequest, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            RaftIndexRequest::LoadIndexInfo => self.load_index_info(),
            RaftIndexRequest::SaveSnapshots(snapshots) => self.write_snapshots(ctx, snapshots),
            RaftIndexRequest::SaveLastAppliedLog(last_applied_log) => {
                self.write_last_applied_log(ctx, last_applied_log)
            }
            RaftIndexRequest::SaveLogs(logs) => self.write_logs(ctx, logs),
            RaftIndexRequest::SaveMember {
                member,
                member_after_consensus,
                node_addr,
            } => self.write_member(ctx, member, member_after_consensus, node_addr),
            RaftIndexRequest::SaveNodeAddr(node_addr) => self.write_node_addr(ctx, node_addr),
            RaftIndexRequest::AddNodeAddr(id, node_addr) => self.add_node_addr(ctx, id, node_addr),
            RaftIndexRequest::SaveHardState {
                current_term,
                voted_for,
            } => self.write_hard_state(ctx, current_term, voted_for),
            RaftIndexRequest::LoadHardState => {
                if let Some(inner) = &self.inner {
                    Ok(RaftIndexResponse::HardState {
                        current_term: inner.raft_index.current_term,
                        voted_for: inner.raft_index.voted_for,
                    })
                } else {
                    Ok(RaftIndexResponse::None)
                }
            }
            RaftIndexRequest::LoadMember => {
                if let Some(inner) = &self.inner {
                    Ok(RaftIndexResponse::MemberShip {
                        member: inner.raft_index.member.clone(),
                        member_after_consensus: inner.raft_index.member_after_consensus.clone(),
                        node_addrs: inner.raft_index.node_addrs.clone(),
                    })
                } else {
                    Ok(RaftIndexResponse::None)
                }
            }
            RaftIndexRequest::GetTargetAddr(id) => {
                let addr = if let Some(inner) = &self.inner {
                    inner.raft_index.node_addrs.get(&id).cloned()
                } else {
                    None
                };
                Ok(RaftIndexResponse::TargetAddr(addr))
            }
        }
    }
}

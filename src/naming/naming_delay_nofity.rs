#![allow(unused_imports)]

use std::{collections::HashSet, sync::Arc, time::Duration};

use actix::prelude::*;
use bean_factory::{bean, Inject};

use crate::{
    common::delay_notify::{DelayNotify, NotifyEvent},
    grpc::bistream_manage::{BiStreamManage, BiStreamManageCmd},
    now_millis,
};

use super::{
    core::{NamingActor, NamingCmd, NamingResult},
    model::{ServiceInfo, ServiceKey},
};

#[derive(Clone, Default)]
pub struct NamingDelayEvent {
    pub key: ServiceKey,
    pub client_id_set: HashSet<Arc<String>>,
    pub service_info: Option<ServiceInfo>,
    pub conn_manage: Option<Addr<BiStreamManage>>,
}

impl NotifyEvent for NamingDelayEvent {
    fn on_event(self) -> anyhow::Result<()> {
        if let (Some(conn_manage), Some(service_info)) =
            (self.conn_manage.as_ref(), self.service_info)
        {
            conn_manage.do_send(BiStreamManageCmd::NotifyNaming(
                self.key,
                self.client_id_set,
                service_info,
            ));
        }
        Ok(())
    }

    fn merge(&mut self, other: Self) -> anyhow::Result<()> {
        self.service_info = other.service_info;
        self.client_id_set = other.client_id_set;
        self.conn_manage = other.conn_manage;
        Ok(())
    }
}

#[bean(inject)]
pub struct DelayNotifyActor {
    inner_delay_notify: DelayNotify<ServiceKey, NamingDelayEvent>,
    conn_manage: Option<Addr<BiStreamManage>>,
    naming_addr: Option<Addr<NamingActor>>,
    delay: u64,
}

impl Default for DelayNotifyActor {
    fn default() -> Self {
        Self::new()
    }
}

impl DelayNotifyActor {
    pub fn new() -> Self {
        Self {
            inner_delay_notify: Default::default(),
            conn_manage: None,
            naming_addr: None,
            delay: 500,
        }
    }

    pub fn notify_heartbeat(&self, ctx: &mut actix::Context<Self>) {
        ctx.run_later(Duration::from_millis(500), |act, ctx| {
            let events = act.inner_delay_notify.timeout().unwrap_or_default();
            let naming_addr = act.naming_addr.clone();
            async move {
                Self::fill_event_data_and_notify(naming_addr, events).await;
            }
            .into_actor(act)
            .map(|_, act, ctx| {
                act.notify_heartbeat(ctx);
            })
            .wait(ctx);
        });
    }

    async fn fill_event_data_and_notify(
        naming_addr: Option<Addr<NamingActor>>,
        events: Vec<NamingDelayEvent>,
    ) {
        if let Some(naming_addr) = naming_addr {
            for mut event in events {
                //println!("fill_event_data_and_notify, {:?}",&event.key);
                let cmd = NamingCmd::QueryServiceInfo(event.key.clone(), "".to_owned(), true);
                match naming_addr.send(cmd).await {
                    Ok(res) => {
                        if let Ok(NamingResult::ServiceInfo(service_info)) = res {
                            event.service_info = Some(service_info);
                        } else {
                            log::error!("fill_event_data_and_notify service_info is empty");
                        }
                    }
                    Err(_err) => {
                        log::error!("fill_event_data_and_notify error");
                    }
                };
                event.on_event().ok();
            }
        }
    }
}

impl Actor for DelayNotifyActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        log::info!(" DelayNotifyActor started");
        self.notify_heartbeat(ctx);
    }
}

impl Inject for DelayNotifyActor {
    type Context = Context<Self>;

    fn inject(
        &mut self,
        factory_data: bean_factory::FactoryData,
        _factory: bean_factory::BeanFactory,
        _ctx: &mut Self::Context,
    ) {
        self.conn_manage = factory_data.get_actor();
        self.naming_addr = factory_data.get_actor();
        log::info!(" DelayNotifyActor inject complete");
    }
}

impl Supervised for DelayNotifyActor {
    fn restarting(&mut self, _ctx: &mut <Self as Actor>::Context) {
        log::warn!("DelayNotifyActor restart ...");
    }
}

#[derive(Message)]
#[rtype(result = "anyhow::Result<DelayNotifyResult>")]
pub enum DelayNotifyCmd {
    Notify(ServiceKey, HashSet<Arc<String>>),
}

pub enum DelayNotifyResult {
    None,
}

impl Handler<DelayNotifyCmd> for DelayNotifyActor {
    type Result = anyhow::Result<DelayNotifyResult>;

    fn handle(&mut self, msg: DelayNotifyCmd, _ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            DelayNotifyCmd::Notify(key, client_id_set) => {
                let event = NamingDelayEvent {
                    key,
                    client_id_set,
                    service_info: None,
                    conn_manage: self.conn_manage.to_owned(),
                };
                self.inner_delay_notify
                    .add_event(self.delay, event.key.clone(), event)?;
            }
        }
        Ok(DelayNotifyResult::None)
    }
}

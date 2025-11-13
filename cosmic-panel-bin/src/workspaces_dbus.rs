use futures::stream::Stream;
use ordered_stream::OrderedStreamExt;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[zbus::proxy(interface = "com.system76.CosmicWorkspaces")]
trait CosmicWorkspaces {
    async fn show(&self) -> zbus::Result<()>;
    async fn hide(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn shown(&self);
    #[zbus(signal)]
    async fn hidden(&self);
}

#[derive(Clone, Debug)]
pub struct CosmicWorkspaces {
    proxy: CosmicWorkspacesProxy<'static>,
    runtime: Arc<Runtime>,
}

impl CosmicWorkspaces {
    pub fn new() -> zbus::Result<Self> {
        let runtime = Runtime::new().expect("failed to create tokio runtime");
        let conn = runtime.block_on(zbus::Connection::session())?;
        let proxy = runtime.block_on(CosmicWorkspacesProxy::new(
            &conn,
            "com.system76.CosmicWorkspaces",
            "/com/system76/CosmicWorkspaces",
        ))?;
        Ok(Self { proxy, runtime: Arc::new(runtime) })
    }

    #[allow(dead_code)]
    pub async fn is_shown_stream(&self) -> zbus::Result<impl Stream<Item = bool> + 'static> {
        let shown_stream = self.proxy.receive_shown().await?;
        let hidden_stream = self.proxy.receive_hidden().await?;
        // Also check if the name owner is lost (cosmic-workspaces stopped or restarted)
        let owner_stream = self.proxy.0.receive_owner_changed().await?;
        Ok(ordered_stream::join(
            ordered_stream::join(shown_stream.map(|_| true), hidden_stream.map(|_| false)),
            owner_stream.filter_map(|owner| if owner.is_none() { Some(false) } else { None }),
        )
        .into_stream())
    }

    pub fn hide(&self) {
        let proxy = self.proxy.clone();
        self.runtime.spawn(async move {
            let _ = proxy.hide().await;
        });
    }
}

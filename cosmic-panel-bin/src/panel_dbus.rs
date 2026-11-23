use crate::PanelCalloopMsg;
use calloop::channel::Sender;
use zbus::{connection::Builder, fdo};

const BUS_NAME: &str = "com.system76.CosmicPanel";
const OBJECT_PATH: &str = "/com/system76/CosmicPanel";

struct PanelControl {
    sender: Sender<PanelCalloopMsg>,
}

#[zbus::interface(name = "com.system76.CosmicPanel")]
impl PanelControl {
    fn toggle_panel_pin(&self) -> fdo::Result<()> {
        self.sender
            .send(PanelCalloopMsg::TogglePanelPin)
            .map_err(|_| fdo::Error::Failed("The panel event loop is not running".into()))
    }
}

pub async fn serve(sender: Sender<PanelCalloopMsg>) -> zbus::Result<()> {
    let _connection = Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, PanelControl { sender })?
        .build()
        .await?;
    // Keep the bus name and exported interface alive while the panel runs.
    std::future::pending().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queues_each_toggle() {
        let (sender, receiver) = calloop::channel::channel();
        let panel = PanelControl { sender };

        for _ in 0..2 {
            panel.toggle_panel_pin().unwrap();
            assert!(matches!(receiver.try_recv(), Ok(PanelCalloopMsg::TogglePanelPin)));
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn reports_a_disconnected_event_loop() {
        let (sender, receiver) = calloop::channel::channel();
        drop(receiver);

        let panel = PanelControl { sender };
        assert!(panel.toggle_panel_pin().is_err());
    }
}

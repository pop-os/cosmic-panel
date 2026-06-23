//! space container is a container for all running panels, each panel space is a
//! separate panel space container implements the WrapperSpace abstraction,
//! calling handle events and other methods of its PanelSpaces as necessary

use crate::space::PanelSpace;

mod space_container;
pub(crate) mod toplevel;
pub(crate) mod workspace;
mod wrapper_space;

pub use space_container::*;

fn space_for_client_mut(
    space_list: &mut [PanelSpace],
    client: Option<wayland_backend::server::ClientId>,
) -> Option<&mut PanelSpace> {
    space_list.iter_mut().find(|space| {
        space
            .clients_center
            .lock()
            .unwrap()
            .iter()
            .chain(space.clients_left.lock().unwrap().iter())
            .chain(space.clients_right.lock().unwrap().iter())
            .any(|c| c.client.as_ref().zip(client.as_ref()).is_some_and(|c| c.0.id() == *c.1))
    })
}

//! space container is a container for all running panels, each panel space is a
//! separate panel space container implements the WrapperSpace abstraction,
//! calling handle events and other methods of its PanelSpaces as necessary

use crate::space::PanelSpace;

#[allow(clippy::module_inception)] // Renaming would disrupt the public module path.
mod space_container;
pub(crate) mod toplevel;
pub(crate) mod workspace;
mod wrapper_space;

pub use space_container::*;

fn space_has_client(
    space: &PanelSpace,
    client: Option<&wayland_backend::server::ClientId>,
) -> bool {
    space
        .clients_center
        .lock()
        .unwrap()
        .iter()
        .chain(space.clients_left.lock().unwrap().iter())
        .chain(space.clients_right.lock().unwrap().iter())
        .any(|c| c.client.as_ref().zip(client.as_ref()).is_some_and(|c| c.0.id() == **c.1))
}

fn space_for_client_mut<'a>(
    space_list: &'a mut [PanelSpace],
    client: Option<&wayland_backend::server::ClientId>,
) -> Option<&'a mut PanelSpace> {
    space_list.iter_mut().find(|space| space_has_client(space, client))
}

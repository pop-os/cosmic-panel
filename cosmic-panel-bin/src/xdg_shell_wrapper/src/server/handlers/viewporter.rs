use smithay::delegate_viewporter;

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

delegate_viewporter!(GlobalState);

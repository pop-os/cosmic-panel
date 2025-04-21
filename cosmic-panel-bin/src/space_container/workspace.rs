use crate::xdg_shell_wrapper::space::WorkspaceHandlerSpace;
use itertools::Itertools;

use super::SpaceContainer;

impl WorkspaceHandlerSpace for SpaceContainer {
    fn update<'a>(
        &mut self,
        groups: impl Iterator<Item = &'a cctk::workspace::WorkspaceGroup>,
        workspaces: impl Iterator<Item = &'a cctk::workspace::Workspace>,
    ) {
        // detect workspace changes
        // for now this is limited to changes
        // to / from workspaces with maximized toplevels
        let pre_maximixed_outputs = self.maximized_outputs();
        self.workspace_groups = groups.cloned().collect();
        self.workspaces = workspaces.cloned().collect();
        let post_maximized_outputs = self.maximized_outputs();
        let outputs = self.outputs.clone();
        for (o, ..) in &outputs {
            let max_pre = pre_maximixed_outputs.iter().contains(o);
            let max_post = post_maximized_outputs.iter().contains(o);
            if max_post && !max_pre {
                self.apply_maximized(o, true);
            } else if !max_post && max_pre {
                self.apply_maximized(o, false);
            }
        }
        self.apply_toplevel_changes()
    }
}

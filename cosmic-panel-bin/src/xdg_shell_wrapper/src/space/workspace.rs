use cctk::workspace::WorkspaceGroup;

pub trait WorkspaceHandlerSpace {
    /// A workspace was updated
    fn update(&mut self, workspace_state: &[WorkspaceGroup]);
}

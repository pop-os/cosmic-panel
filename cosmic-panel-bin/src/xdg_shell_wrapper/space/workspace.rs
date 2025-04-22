pub trait WorkspaceHandlerSpace {
    /// A workspace was updated
    fn update<'a>(
        &mut self,
        groups: impl Iterator<Item = &'a cctk::workspace::WorkspaceGroup>,
        workspaces: impl Iterator<Item = &'a cctk::workspace::Workspace>,
    );
}

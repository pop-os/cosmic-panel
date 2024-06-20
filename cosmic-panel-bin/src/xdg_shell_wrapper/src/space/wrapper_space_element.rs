use smithay::desktop::{space::space_elements, Window};

space_elements! {
    /// space elements for the wrapper
    pub WrapperSpaceElement;
    /// window
    Window=Window,
}

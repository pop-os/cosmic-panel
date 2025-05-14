use smithay::{
    desktop::space::SpaceElement,
    utils::{IsAlive, Logical},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spacer {
    pub name: String,
    pub bbox: smithay::utils::Rectangle<i32, Logical>,
}

impl std::hash::Hash for Spacer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.bbox.loc.x, self.bbox.loc.y, self.bbox.size.w, self.bbox.size.h, self.name.clone())
            .hash(state);
    }
}

impl SpaceElement for Spacer {
    fn bbox(&self) -> smithay::utils::Rectangle<i32, Logical> {
        self.bbox.clone()
    }

    fn is_in_input_region(
        &self,
        _point: &smithay::utils::Point<f64, smithay::utils::Logical>,
    ) -> bool {
        false
    }

    fn set_activate(&self, _activated: bool) {}

    fn output_enter(
        &self,
        _output: &smithay::output::Output,
        _overlap: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) {
    }

    fn output_leave(&self, _output: &smithay::output::Output) {}
}

impl IsAlive for Spacer {
    fn alive(&self) -> bool {
        true
    }
}

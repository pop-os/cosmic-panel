use std::rc::Rc;

use anyhow::bail;
use cctk::{
    sctk::{
        compositor::Region,
        shell::xdg::{
            popup::{self, Popup},
            XdgPositioner,
        },
    },
    wayland_client::{protocol::wl_seat::WlSeat, Proxy, QueueHandle},
};
use cosmic::iced::id;

use cosmic_panel_config::PanelAnchor;
use smithay::{
    backend::{
        egl::EGLSurface,
        renderer::{damage::OutputDamageTracker, element},
    },
    desktop::space::SpaceElement,
    utils::{Logical, Rectangle, Size},
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        shell::xdg::SurfaceCachedState,
    },
};
use wayland_protocols::{
    wp::{
        fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
        viewporter::client::wp_viewport::WpViewport,
    },
    xdg::shell::client::xdg_positioner::{self, Anchor, Gravity},
};

use crate::{
    iced::elements::{CosmicMappedInternal, PopupMappedInternal},
    xdg_shell_wrapper::{
        shared_state::GlobalState,
        space::{PanelPopup, WrapperPopupState},
        wp_fractional_scaling::FractionalScalingManager,
        wp_viewporter::ViewporterState,
    },
};

use super::{layout::OverflowSection, PanelSpace};

impl PanelSpace {
    pub fn toggle_overflow_popup(
        &mut self,
        element_id: id::Id,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        qh: &QueueHandle<GlobalState>,
        xdg_shell_state: &mut sctk::shell::xdg::XdgShell,
        seat: (u32, WlSeat),
    ) -> anyhow::Result<()> {
        self.popups.clear();
        if self.overflow_popup.is_some() {
            self.overflow_popup = None;
            return Ok(());
        }
        // get popup location and anchor based on element_id and panel
        // anchor create popup using sctk
        let c_wl_surface = compositor_state.create_surface(qh);

        let (Some((element, section)), Some(popup_element)) =
            self.overflow_elements_for_id(&element_id)
        else {
            bail!("No element found with id: {:?}", element_id);
        };
        let loc = self.space.element_location(&element).unwrap_or_default();
        let bbox = element.bbox();
        let positioner = XdgPositioner::new(xdg_shell_state).unwrap();
        let popup_bbox = popup_element.bbox();
        positioner.set_anchor_rect(loc.x, loc.y, bbox.size.w, bbox.size.h);
        let pixel_offset = 8;
        let (offset, anchor, gravity) = match self.config.anchor {
            PanelAnchor::Left => ((pixel_offset, 0), Anchor::Right, Gravity::Right),
            PanelAnchor::Right => ((-pixel_offset, 0), Anchor::Left, Gravity::Left),
            PanelAnchor::Top => ((0, pixel_offset), Anchor::Bottom, Gravity::Bottom),
            PanelAnchor::Bottom => ((0, -pixel_offset), Anchor::Top, Gravity::Top),
        };
        positioner.set_anchor(anchor);
        positioner.set_gravity(gravity);
        positioner.set_constraint_adjustment(
            xdg_positioner::ConstraintAdjustment::FlipY
                | xdg_positioner::ConstraintAdjustment::FlipX
                | xdg_positioner::ConstraintAdjustment::SlideX
                | xdg_positioner::ConstraintAdjustment::SlideY,
        );
        positioner.set_offset(offset.0, offset.1);

        positioner.set_size(popup_bbox.size.w, popup_bbox.size.h);
        let c_popup = popup::Popup::from_surface(
            None,
            &positioner,
            qh,
            c_wl_surface.clone(),
            xdg_shell_state,
        )?;

        c_popup.xdg_popup().grab(&seat.1, seat.0);

        c_popup.xdg_surface().set_window_geometry(
            popup_bbox.loc.x,
            popup_bbox.loc.y,
            popup_bbox.size.w.max(1),
            popup_bbox.size.h.max(1),
        );
        self.layer.as_ref().unwrap().get_popup(c_popup.xdg_popup());

        let fractional_scale =
            fractional_scale_manager.map(|f| f.fractional_scaling(&c_wl_surface, &qh));

        let viewport = viewport.map(|v| {
            let viewport = v.get_viewport(&c_wl_surface, &qh);
            viewport.set_destination(popup_bbox.size.w.max(1), popup_bbox.size.h.max(1));
            viewport
        });
        if fractional_scale.is_none() {
            c_wl_surface.set_buffer_scale(self.scale as i32);
        }

        // must be done after role is assigned as popup
        c_wl_surface.commit();

        self.overflow_popup = Some((
            PanelPopup {
                damage_tracked_renderer: OutputDamageTracker::new(
                    popup_bbox.size.to_f64().to_physical(self.scale).to_i32_round(),
                    1.0,
                    smithay::utils::Transform::Flipped180,
                ),
                c_popup,
                egl_surface: None,
                dirty: false,
                rectangle: Rectangle::from_loc_and_size((0, 0), popup_bbox.size),
                state: Some(WrapperPopupState::WaitConfigure),
                wrapper_rectangle: Rectangle::from_loc_and_size((0, 0), popup_bbox.size),
                positioner,
                has_frame: true,
                fractional_scale,
                viewport,
                scale: self.scale,
                input_region: None,
            },
            section,
        ));

        Ok(())
    }

    fn overflow_elements_for_id(
        &self,
        element_id: &id::Id,
    ) -> (Option<(CosmicMappedInternal, OverflowSection)>, Option<(PopupMappedInternal)>) {
        let element = self.space.elements().find_map(|e| match e {
            CosmicMappedInternal::OverflowButton(b) => b.with_program(|p| {
                (&p.id == element_id).then_some((
                    e.clone(),
                    if &self.left_overflow_button_id == &p.id {
                        OverflowSection::Left
                    } else if &self.right_overflow_button_id == &p.id {
                        OverflowSection::Right
                    } else {
                        OverflowSection::Center
                    },
                ))
            }),
            _ => None,
        });
        let popup_element = element
            .as_ref()
            .map(|(_, section)| match section {
                OverflowSection::Left => self.overflow_left.elements(),
                OverflowSection::Right => self.overflow_right.elements(),
                OverflowSection::Center => self.overflow_center.elements(),
            })
            .and_then(|mut elements| elements.find(|e| matches!(e, PopupMappedInternal::Popup(_))));
        (element, popup_element.cloned())
    }

    pub fn handle_overflow_popup_events(&mut self) {
        self.overflow_popup = self
            .overflow_popup
            .take()
            .into_iter()
            .filter_map(|(mut p, section)| {
                if let Some(WrapperPopupState::Rectangle { width, height, x, y }) = p.state {
                    p.dirty = true;
                    p.rectangle = Rectangle::from_loc_and_size((x, y), (width, height));
                    let scaled_size: Size<i32, _> =
                        p.rectangle.size.to_f64().to_physical(p.scale).to_i32_round();
                    if let Some(s) = p.egl_surface.as_ref() {
                        s.resize(scaled_size.w.max(1), scaled_size.h.max(1), 0, 0);
                    }
                    if let Some(viewport) = p.viewport.as_ref() {
                        viewport
                            .set_destination(p.rectangle.size.w.max(1), p.rectangle.size.h.max(1));
                    }
                    p.damage_tracked_renderer = OutputDamageTracker::new(
                        scaled_size,
                        1.0,
                        smithay::utils::Transform::Flipped180,
                    );
                    p.c_popup.wl_surface().commit();

                    p.state = None;
                }
                Some((p, section))
            })
            .next();
    }
}

use super::{overflow_button::OverflowButtonElement, CosmicMappedInternal, PopupMappedInternal};
use crate::xdg_shell_wrapper::shared_state::GlobalState;

use anyhow::bail;
use smithay::{
    input::{keyboard::KeyboardTarget, pointer::PointerTarget, touch::TouchTarget},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::IsAlive,
    wayland::seat::WaylandFocus,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SpaceTarget {
    Surface(WlSurface),
    OverflowButton(OverflowButtonElement),
}

impl SpaceTarget {
    fn inner_keyboard_target(&self) -> &dyn KeyboardTarget<GlobalState> {
        match self {
            SpaceTarget::Surface(s) => s,
            SpaceTarget::OverflowButton(b) => b,
        }
    }

    fn inner_pointer_target(&self) -> &dyn PointerTarget<GlobalState> {
        match self {
            SpaceTarget::Surface(s) => s,
            SpaceTarget::OverflowButton(b) => b,
        }
    }

    fn inner_touch_target(&self) -> &dyn TouchTarget<GlobalState> {
        match self {
            SpaceTarget::Surface(s) => s,
            SpaceTarget::OverflowButton(b) => b,
        }
    }
}

impl TryFrom<CosmicMappedInternal> for SpaceTarget {
    type Error = anyhow::Error;

    fn try_from(value: CosmicMappedInternal) -> Result<Self, Self::Error> {
        match value {
            CosmicMappedInternal::Window(w) => {
                Ok(SpaceTarget::Surface(w.toplevel().unwrap().wl_surface().clone()))
            },
            CosmicMappedInternal::OverflowButton(b) => Ok(SpaceTarget::OverflowButton(b)),
            CosmicMappedInternal::Background(_) => bail!("Cannot convert background"),
            CosmicMappedInternal::Spacer(_) => bail!("Cannot convert spacer"),
            CosmicMappedInternal::_GenericCatcher(_) => unreachable!(),
        }
    }
}

impl TryFrom<PopupMappedInternal> for SpaceTarget {
    type Error = anyhow::Error;

    fn try_from(value: PopupMappedInternal) -> Result<Self, Self::Error> {
        match value {
            PopupMappedInternal::Window(w) => {
                Ok(SpaceTarget::Surface(w.toplevel().unwrap().wl_surface().clone()))
            },
            PopupMappedInternal::Popup(p) => bail!("Cannot convert popup"),
            PopupMappedInternal::_GenericCatcher(_) => unreachable!(),
        }
    }
}

impl From<WlSurface> for SpaceTarget {
    fn from(surface: WlSurface) -> Self {
        SpaceTarget::Surface(surface)
    }
}

impl From<OverflowButtonElement> for SpaceTarget {
    fn from(button: OverflowButtonElement) -> Self {
        SpaceTarget::OverflowButton(button)
    }
}

impl IsAlive for SpaceTarget {
    fn alive(&self) -> bool {
        match self {
            SpaceTarget::Surface(s) => s.alive(),
            SpaceTarget::OverflowButton(b) => b.alive(),
        }
    }
}

impl PointerTarget<GlobalState> for SpaceTarget {
    fn enter(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        self.inner_pointer_target().enter(seat, data, event)
    }

    fn motion(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        self.inner_pointer_target().motion(seat, data, event)
    }

    fn relative_motion(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::RelativeMotionEvent,
    ) {
        self.inner_pointer_target().relative_motion(seat, data, event)
    }

    fn button(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::ButtonEvent,
    ) {
        self.inner_pointer_target().button(seat, data, event)
    }

    fn axis(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        frame: smithay::input::pointer::AxisFrame,
    ) {
        self.inner_pointer_target().axis(seat, data, frame)
    }

    fn frame(&self, seat: &smithay::input::Seat<GlobalState>, data: &mut GlobalState) {
        self.inner_pointer_target().frame(seat, data)
    }

    fn gesture_swipe_begin(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_begin(seat, data, event)
    }

    fn gesture_swipe_update(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_update(seat, data, event)
    }

    fn gesture_swipe_end(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_end(seat, data, event)
    }

    fn gesture_pinch_begin(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_begin(seat, data, event)
    }

    fn gesture_pinch_update(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_update(seat, data, event)
    }

    fn gesture_pinch_end(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_end(seat, data, event)
    }

    fn gesture_hold_begin(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        self.inner_pointer_target().gesture_hold_begin(seat, data, event)
    }

    fn gesture_hold_end(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        self.inner_pointer_target().gesture_hold_end(seat, data, event)
    }

    fn leave(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        self.inner_pointer_target().leave(seat, data, serial, time)
    }
}

impl KeyboardTarget<GlobalState> for SpaceTarget {
    fn enter(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        keys: Vec<smithay::input::keyboard::KeysymHandle<'_>>,
        serial: smithay::utils::Serial,
    ) {
        self.inner_keyboard_target().enter(seat, data, keys, serial)
    }

    fn leave(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        serial: smithay::utils::Serial,
    ) {
        self.inner_keyboard_target().leave(seat, data, serial)
    }

    fn key(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        key: smithay::input::keyboard::KeysymHandle<'_>,
        state: smithay::backend::input::KeyState,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        self.inner_keyboard_target().key(seat, data, key, state, serial, time)
    }

    fn modifiers(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        modifiers: smithay::input::keyboard::ModifiersState,
        serial: smithay::utils::Serial,
    ) {
        self.inner_keyboard_target().modifiers(seat, data, modifiers, serial)
    }
}

// TODO Iced touch events
impl TouchTarget<GlobalState> for SpaceTarget {
    fn down(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::touch::DownEvent,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().down(seat, data, event, serial)
    }

    fn up(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::touch::UpEvent,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().up(seat, data, event, serial)
    }

    fn motion(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::touch::MotionEvent,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().motion(seat, data, event, serial)
    }

    fn frame(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().frame(seat, data, serial)
    }

    fn cancel(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().cancel(seat, data, serial)
    }

    fn shape(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::touch::ShapeEvent,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().shape(seat, data, event, serial)
    }

    fn orientation(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::touch::OrientationEvent,
        serial: smithay::utils::Serial,
    ) {
        self.inner_touch_target().orientation(seat, data, event, serial)
    }
}

impl WaylandFocus for SpaceTarget {
    fn wl_surface(
        &self,
    ) -> Option<
        std::borrow::Cow<'_, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
    > {
        match self {
            SpaceTarget::Surface(s) => Some(std::borrow::Cow::Borrowed(s)),
            SpaceTarget::OverflowButton(b) => b.wl_surface(),
        }
    }
}

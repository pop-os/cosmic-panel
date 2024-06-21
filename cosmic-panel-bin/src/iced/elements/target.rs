use super::{overflow_button::OverflowButtonElement, CosmicMappedInternal};
use crate::xdg_shell_wrapper::shared_state::GlobalState;

use smithay::{
    input::{keyboard::KeyboardTarget, pointer::PointerTarget},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::IsAlive,
    wayland::seat::WaylandFocus,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SpaceTarget {
    Surface(WlSurface),
    OverflowButton(OverflowButtonElement),
}

impl From<CosmicMappedInternal> for SpaceTarget {
    fn from(internal: CosmicMappedInternal) -> Self {
        match internal {
            CosmicMappedInternal::Window(w) => {
                SpaceTarget::Surface(w.toplevel().unwrap().wl_surface().clone())
            },
            CosmicMappedInternal::OverflowButton(b) => SpaceTarget::OverflowButton(b),
            CosmicMappedInternal::_GenericCatcher(_) => unreachable!(),
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
        match self {
            SpaceTarget::Surface(s) => PointerTarget::enter(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => PointerTarget::enter(b, seat, data, event),
        }
    }

    fn motion(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::motion(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.motion(seat, data, event),
        }
    }

    fn relative_motion(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::RelativeMotionEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::relative_motion(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.relative_motion(seat, data, event),
        }
    }

    fn button(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::ButtonEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::button(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.button(seat, data, event),
        }
    }

    fn axis(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        frame: smithay::input::pointer::AxisFrame,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::axis(s, seat, data, frame),
            SpaceTarget::OverflowButton(b) => b.axis(seat, data, frame),
        }
    }

    fn frame(&self, seat: &smithay::input::Seat<GlobalState>, data: &mut GlobalState) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::frame(s, seat, data),
            SpaceTarget::OverflowButton(b) => b.frame(seat, data),
        }
    }

    fn gesture_swipe_begin(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_swipe_begin(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_swipe_begin(seat, data, event),
        }
    }

    fn gesture_swipe_update(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_swipe_update(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_swipe_update(seat, data, event),
        }
    }

    fn gesture_swipe_end(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_swipe_end(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_swipe_end(seat, data, event),
        }
    }

    fn gesture_pinch_begin(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_pinch_begin(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_pinch_begin(seat, data, event),
        }
    }

    fn gesture_pinch_update(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_pinch_update(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_pinch_update(seat, data, event),
        }
    }

    fn gesture_pinch_end(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_pinch_end(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_pinch_end(seat, data, event),
        }
    }

    fn gesture_hold_begin(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_hold_begin(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_hold_begin(seat, data, event),
        }
    }

    fn gesture_hold_end(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::gesture_hold_end(s, seat, data, event),
            SpaceTarget::OverflowButton(b) => b.gesture_hold_end(seat, data, event),
        }
    }

    fn leave(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        match self {
            SpaceTarget::Surface(s) => PointerTarget::leave(s, seat, data, serial, time),
            SpaceTarget::OverflowButton(b) => PointerTarget::leave(b, seat, data, serial, time),
        }
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
        match self {
            SpaceTarget::Surface(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
            SpaceTarget::OverflowButton(b) => KeyboardTarget::enter(b, seat, data, keys, serial),
        }
    }

    fn leave(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        serial: smithay::utils::Serial,
    ) {
        match self {
            SpaceTarget::Surface(s) => KeyboardTarget::leave(s, seat, data, serial),
            SpaceTarget::OverflowButton(b) => KeyboardTarget::leave(b, seat, data, serial),
        }
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
        match self {
            SpaceTarget::Surface(s) => KeyboardTarget::key(s, seat, data, key, state, serial, time),
            SpaceTarget::OverflowButton(b) => {
                KeyboardTarget::key(b, seat, data, key, state, serial, time)
            },
        }
    }

    fn modifiers(
        &self,
        seat: &smithay::input::Seat<GlobalState>,
        data: &mut GlobalState,
        modifiers: smithay::input::keyboard::ModifiersState,
        serial: smithay::utils::Serial,
    ) {
        match self {
            SpaceTarget::Surface(s) => KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
            SpaceTarget::OverflowButton(b) => {
                KeyboardTarget::modifiers(b, seat, data, modifiers, serial)
            },
        }
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

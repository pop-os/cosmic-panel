use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt,
    hash::{Hash, Hasher},
    sync::{mpsc::Receiver, Arc, Mutex},
    time::{Duration, Instant},
};

use crate::xdg_shell_wrapper::shared_state::GlobalState;
use cosmic::{
    iced::{
        advanced::widget::Tree,
        event::Event,
        futures::{FutureExt, StreamExt},
        keyboard::{Event as KeyboardEvent, Modifiers as IcedModifiers},
        mouse::{Button as MouseButton, Cursor, Event as MouseEvent, ScrollDelta},
        window::Event as WindowEvent,
        Limits, Point as IcedPoint, Rectangle as IcedRectangle, Size as IcedSize, Task,
    },
    iced_core::{clipboard::Null as NullClipboard, renderer::Style, Color, Font, Length, Pixels},
    iced_renderer::Renderer as IcedRenderer,
    iced_runtime::{
        program::{Program as IcedProgram, State},
        task::into_stream,
        Action, Debug,
    },
    widget::Id,
    Theme,
};
use iced_tiny_skia::{
    graphics::{damage, Viewport},
    Primitive,
};
use once_cell::sync::Lazy;
use ordered_float::OrderedFloat;
use smithay::{
    backend::{
        allocator::Fourcc,
        input::{ButtonState, KeyState},
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                AsRenderElements, Kind,
            },
            ImportMem, Renderer,
        },
    },
    desktop::space::{RenderZindex, SpaceElement},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerTarget, RelativeMotionEvent,
        },
        Seat,
    },
    output::Output,
    reexports::calloop::{self, futures::Scheduler, LoopHandle, RegistrationToken},
    utils::{
        Buffer as BufferCoords, IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial, Size,
        Transform,
    },
    wayland::seat::WaylandFocus,
};

pub mod elements;
pub mod panel_message;

static ID: Lazy<Id> = Lazy::new(|| Id::new("Program"));

pub type Element<'a, Message> = cosmic::iced::Element<'a, Message, cosmic::Theme, cosmic::Renderer>;

pub struct IcedElement<P: Program + Send + 'static>(Arc<Mutex<IcedElementInternal<P>>>);

impl<P: Program + Send + 'static> fmt::Debug for IcedElement<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

// SAFETY: We cannot really be sure about `iced_native::program::State` sadly,
// but the rest should be fine.
unsafe impl<P: Program + Send + 'static> Send for IcedElementInternal<P> {}

impl<P: Program + Send + 'static> Clone for IcedElement<P> {
    fn clone(&self) -> Self {
        IcedElement(self.0.clone())
    }
}

impl<P: Program + Send + 'static> PartialEq for IcedElement<P> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<P: Program + Send + 'static> Eq for IcedElement<P> {}

impl<P: Program + Send + 'static> Hash for IcedElement<P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state)
    }
}

pub trait Program {
    type Message: std::fmt::Debug + Send;
    fn update(
        &mut self,
        message: Self::Message,
        loop_handle: &LoopHandle<'static, GlobalState>,
    ) -> Task<Self::Message> {
        let _ = (message, loop_handle);
        Task::none()
    }
    fn view(&self) -> Element<'_, Self::Message>;

    fn background_color(&self) -> Color {
        Color::TRANSPARENT
    }

    fn foreground(
        &self,
        _pixels: &mut tiny_skia::PixmapMut<'_>,
        _damage: &[Rectangle<i32, BufferCoords>],
        _scale: f32,
    ) {
    }
}

struct ProgramWrapper<P: Program>(P, LoopHandle<'static, GlobalState>);
impl<P: Program> IcedProgram for ProgramWrapper<P> {
    type Message = <P as Program>::Message;
    type Renderer = cosmic::Renderer;
    type Theme = cosmic::Theme;

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        self.0.update(message, &self.1)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        self.0.view()
    }
}

struct IcedElementInternal<P: Program + Send + 'static> {
    // draw buffer
    outputs: HashSet<Output>,
    buffers: HashMap<OrderedFloat<f64>, (MemoryRenderBuffer, Color)>,
    pending_update: Option<Instant>,
    request_redraws: bool,

    // state
    size: Size<i32, Logical>,
    cursor_pos: Option<Point<f64, Logical>>,
    panel_id: usize,

    // iced
    theme: Theme,
    renderer: cosmic::Renderer,
    state: State<ProgramWrapper<P>>,
    debug: Debug,

    // futures
    handle: LoopHandle<'static, GlobalState>,
    scheduler: Scheduler<Option<<P as Program>::Message>>,
    executor_token: Option<RegistrationToken>,
    rx: Receiver<Option<<P as Program>::Message>>,
}

impl<P: Program + Send + Clone + 'static> Clone for IcedElementInternal<P> {
    fn clone(&self) -> Self {
        let handle = self.handle.clone();
        let (executor, scheduler) = calloop::futures::executor().expect("Out of file descriptors");
        let (tx, rx) = std::sync::mpsc::channel();
        let executor_token = handle
            .insert_source(executor, move |message, _, _| {
                let _ = tx.send(message);
            })
            .ok();

        if !self.state.is_queue_empty() {
            tracing::warn!("Missing force_update call");
        }
        let mut renderer = IcedRenderer::new(Font::default(), Pixels(16.0));
        let mut debug = Debug::new();
        let state = State::new(
            ID.clone(),
            ProgramWrapper(self.state.program().0.clone(), handle.clone()),
            IcedSize::new(self.size.w as f32, self.size.h as f32),
            &mut renderer,
            &mut debug,
        );
        let request_redraws = self.request_redraws;

        IcedElementInternal {
            outputs: self.outputs.clone(),
            buffers: self.buffers.clone(),
            pending_update: self.pending_update,
            size: self.size,
            cursor_pos: self.cursor_pos,
            theme: self.theme.clone(),
            panel_id: self.panel_id,
            renderer,
            state,
            debug,
            handle,
            scheduler,
            executor_token,
            rx,
            request_redraws,
        }
    }
}

impl<P: Program + Send + 'static> fmt::Debug for IcedElementInternal<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IcedElementInternal")
            .field("buffers", &"...")
            .field("size", &self.size)
            .field("pending_update", &self.pending_update)
            .field("cursor_pos", &self.cursor_pos)
            .field("theme", &"...")
            .field("renderer", &"...")
            .field("state", &"...")
            .field("debug", &self.debug)
            .field("handle", &self.handle)
            .field("scheduler", &self.scheduler)
            .field("executor_token", &self.executor_token)
            .field("rx", &self.rx)
            .finish()
    }
}

impl<P: Program + Send + 'static> Drop for IcedElementInternal<P> {
    fn drop(&mut self) {
        self.handle.remove(self.executor_token.take().unwrap());
    }
}

impl<P: Program + Send + 'static> IcedElement<P> {
    pub fn new(
        program: P,
        size: impl Into<Size<i32, Logical>>,
        handle: LoopHandle<'static, GlobalState>,
        theme: cosmic::Theme,
        panel_id: usize,
        request_redraws: bool,
    ) -> IcedElement<P> {
        let size = size.into();
        let mut renderer = IcedRenderer::new(Font::default(), Pixels(16.0));
        let mut debug = Debug::new();

        let state = State::new(
            ID.clone(),
            ProgramWrapper(program, handle.clone()),
            IcedSize::new(size.w as f32, size.h as f32),
            &mut renderer,
            &mut debug,
        );

        let (executor, scheduler) = calloop::futures::executor().expect("Out of file descriptors");
        let (tx, rx) = std::sync::mpsc::channel();
        let executor_token = handle
            .insert_source(executor, move |message, _, _| {
                let _ = tx.send(message);
            })
            .ok();

        let mut internal = IcedElementInternal {
            outputs: HashSet::new(),
            buffers: HashMap::new(),
            pending_update: None,
            size,
            cursor_pos: None,
            theme,
            renderer,
            state,
            debug,
            handle,
            scheduler,
            executor_token,
            rx,
            panel_id,
            request_redraws,
        };
        let _ = internal.update(true);

        IcedElement(Arc::new(Mutex::new(internal)))
    }

    pub fn with_program<R>(&self, func: impl FnOnce(&P) -> R) -> R {
        let internal = self.0.lock().unwrap();
        func(&internal.state.program().0)
    }

    pub fn minimum_size(&self) -> Size<i32, Logical> {
        let internal = self.0.lock().unwrap();
        let element = internal.state.program().0.view();
        let node = element
            .as_widget()
            .layout(
                // TODO Avoid creating a new tree here?
                &mut Tree::new(element.as_widget()),
                &internal.renderer,
                &Limits::new(IcedSize::ZERO, IcedSize::INFINITY)
                    .width(Length::Shrink)
                    .height(Length::Shrink),
            )
            .size();
        Size::from((node.width.ceil() as i32, node.height.ceil() as i32))
    }

    pub fn loop_handle(&self) -> LoopHandle<'static, GlobalState> {
        self.0.lock().unwrap().handle.clone()
    }

    pub fn resize(&self, size: Size<i32, Logical>) {
        let mut internal = self.0.lock().unwrap();
        let internal_ref = &mut *internal;
        if internal_ref.size == size {
            return;
        }

        internal_ref.size = size;
        for (scale, (buffer, ..)) in internal_ref.buffers.iter_mut() {
            let buffer_size =
                internal_ref.size.to_f64().to_buffer(**scale, Transform::Normal).to_i32_round();
            *buffer =
                MemoryRenderBuffer::new(Fourcc::Argb8888, buffer_size, 1, Transform::Normal, None);
        }

        if internal_ref.pending_update.is_none() {
            internal_ref.pending_update = Some(Instant::now());
        }
    }

    pub fn force_update(&self) {
        self.0.lock().unwrap().update(true);
    }

    pub fn set_theme(&self, theme: cosmic::Theme) {
        let mut guard = self.0.lock().unwrap();
        guard.theme = theme.clone();
    }

    pub fn force_redraw(&self) {
        let mut internal = self.0.lock().unwrap();

        internal.update(true);
    }
}

impl<P: Program + Send + 'static + Clone> IcedElement<P> {
    pub fn deep_clone(&self) -> Self {
        let internal = self.0.lock().unwrap();
        if !internal.state.is_queue_empty() {
            self.force_update();
        }
        IcedElement(Arc::new(Mutex::new(internal.clone())))
    }
}

impl<P: Program + Send + 'static> IcedElementInternal<P> {
    fn update(&mut self, mut force: bool) -> Vec<Task<<P as Program>::Message>> {
        while let Ok(Some(message)) = self.rx.try_recv() {
            self.state.queue_message(message);
            force = true;
        }

        if !force {
            return Vec::new();
        }

        let cursor = self
            .cursor_pos
            .map(|p| IcedPoint::new(p.x as f32, p.y as f32))
            .map(Cursor::Available)
            .unwrap_or(Cursor::Unavailable);
        let actions = self
            .state
            .update(
                ID.clone(),
                IcedSize::new(self.size.w as f32, self.size.h as f32),
                cursor,
                &mut self.renderer,
                &self.theme,
                &Style {
                    scale_factor: 1.0, // TODO: why is this
                    icon_color: self.theme.cosmic().on_bg_color().into(),
                    text_color: self.theme.cosmic().on_bg_color().into(),
                },
                &mut NullClipboard,
                &mut self.debug,
            )
            .1;

        actions
            .into_iter()
            .filter_map(|action| {
                if let Some(t) = into_stream(action) {
                    let _ = self.scheduler.schedule(t.into_future().map(|f| match f.0 {
                        Some(Action::Output(msg)) => Some(msg),
                        _ => None,
                    }));
                }
                None
            })
            .collect::<Vec<_>>()
    }
}

impl<P: Program + Send + 'static> PointerTarget<GlobalState> for IcedElement<P> {
    fn enter(&self, _seat: &Seat<GlobalState>, _data: &mut GlobalState, event: &MotionEvent) {
        let mut internal = self.0.lock().unwrap();
        if internal.request_redraws {
            internal.pending_update = Some(Instant::now());
        }
        let panel_id = internal.panel_id;
        internal.handle.insert_idle(move |state| {
            state.space.iced_request_redraw(panel_id);
        });
        internal.state.queue_event(Event::Mouse(MouseEvent::CursorEntered));
        let position = IcedPoint::new(event.location.x as f32, event.location.y as f32);
        internal.state.queue_event(Event::Mouse(MouseEvent::CursorMoved { position }));
        internal.cursor_pos = Some(event.location);
        let _ = internal.update(true);
    }

    fn motion(&self, _seat: &Seat<GlobalState>, _data: &mut GlobalState, event: &MotionEvent) {
        let mut internal = self.0.lock().unwrap();
        let bbox = Rectangle::from_loc_and_size((0, 0), internal.size);
        if internal.request_redraws {
            internal.pending_update = Some(Instant::now());
        }

        if bbox.contains(event.location.to_i32_round()) {
            internal.cursor_pos = Some(event.location);
            let position = IcedPoint::new(event.location.x as f32, event.location.y as f32);
            internal.state.queue_event(Event::Mouse(MouseEvent::CursorMoved { position }));
        } else {
            internal.cursor_pos = None;
            internal.state.queue_event(Event::Mouse(MouseEvent::CursorLeft));
        }
        let _ = internal.update(true);
    }

    fn relative_motion(
        &self,
        _seat: &Seat<GlobalState>,
        _data: &mut GlobalState,
        _event: &RelativeMotionEvent,
    ) {
    }

    fn button(&self, _seat: &Seat<GlobalState>, _data: &mut GlobalState, event: &ButtonEvent) {
        let mut internal = self.0.lock().unwrap();
        if internal.request_redraws {
            internal.pending_update = Some(Instant::now());
        }
        let button = match event.button {
            0x110 => MouseButton::Left,
            0x111 => MouseButton::Right,
            0x112 => MouseButton::Middle,
            x => MouseButton::Other(x as u16),
        };
        internal.state.queue_event(Event::Mouse(match event.state {
            ButtonState::Pressed => MouseEvent::ButtonPressed(button),
            ButtonState::Released => MouseEvent::ButtonReleased(button),
        }));
        let _ = internal.update(true);
    }

    fn axis(&self, _seat: &Seat<GlobalState>, _data: &mut GlobalState, frame: AxisFrame) {
        let mut internal = self.0.lock().unwrap();
        if internal.request_redraws {
            internal.pending_update = Some(Instant::now());
        }
        internal.state.queue_event(Event::Mouse(MouseEvent::WheelScrolled {
            delta: if let Some(discrete) = frame.v120 {
                ScrollDelta::Lines { x: discrete.0 as f32 / 120., y: discrete.1 as f32 / 120. }
            } else {
                ScrollDelta::Pixels { x: frame.axis.0 as f32, y: frame.axis.1 as f32 }
            },
        }));
        let _ = internal.update(true);
    }

    fn frame(&self, _seat: &Seat<GlobalState>, _data: &mut GlobalState) {}

    fn leave(
        &self,
        _seat: &Seat<GlobalState>,
        _data: &mut GlobalState,
        _serial: Serial,
        _time: u32,
    ) {
        let mut internal = self.0.lock().unwrap();
        if internal.request_redraws {
            internal.pending_update = Some(Instant::now());
            let panel_id = internal.panel_id;
            internal.handle.insert_idle(move |state| {
                state.space.iced_request_redraw(panel_id);
            });
        }
        internal.cursor_pos = None;
        internal.state.queue_event(Event::Mouse(MouseEvent::CursorLeft));
        let _ = internal.update(true);
    }

    fn gesture_swipe_begin(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GestureSwipeBeginEvent,
    ) {
    }

    fn gesture_swipe_update(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GestureSwipeUpdateEvent,
    ) {
    }

    fn gesture_swipe_end(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GestureSwipeEndEvent,
    ) {
    }

    fn gesture_pinch_begin(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GesturePinchBeginEvent,
    ) {
    }

    fn gesture_pinch_update(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GesturePinchUpdateEvent,
    ) {
    }

    fn gesture_pinch_end(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GesturePinchEndEvent,
    ) {
    }

    fn gesture_hold_begin(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GestureHoldBeginEvent,
    ) {
    }

    fn gesture_hold_end(
        &self,
        _: &Seat<GlobalState>,
        _: &mut GlobalState,
        _: &GestureHoldEndEvent,
    ) {
    }
}

impl<P: Program + Send + 'static> KeyboardTarget<GlobalState> for IcedElement<P> {
    fn enter(
        &self,
        _seat: &Seat<GlobalState>,
        _data: &mut GlobalState,
        _keys: Vec<KeysymHandle<'_>>,
        _serial: Serial,
    ) {
        // TODO convert keys
    }

    fn leave(&self, _seat: &Seat<GlobalState>, _data: &mut GlobalState, _serial: Serial) {
        // TODO remove all held keys
    }

    fn key(
        &self,
        _seat: &Seat<GlobalState>,
        _data: &mut GlobalState,
        _key: KeysymHandle<'_>,
        _state: KeyState,
        _serial: Serial,
        _time: u32,
    ) {
        // TODO convert keys
    }

    fn modifiers(
        &self,
        _seat: &Seat<GlobalState>,
        _data: &mut GlobalState,
        modifiers: ModifiersState,
        _serial: Serial,
    ) {
        let mut internal = self.0.lock().unwrap();
        let mut mods = IcedModifiers::empty();
        if modifiers.shift {
            mods.insert(IcedModifiers::SHIFT);
        }
        if modifiers.alt {
            mods.insert(IcedModifiers::ALT);
        }
        if modifiers.ctrl {
            mods.insert(IcedModifiers::CTRL);
        }
        if modifiers.logo {
            mods.insert(IcedModifiers::LOGO);
        }
        internal.state.queue_event(Event::Keyboard(KeyboardEvent::ModifiersChanged(mods)));
        let _ = internal.update(true);
    }
}

impl<P: Program + Send + 'static> IsAlive for IcedElement<P> {
    fn alive(&self) -> bool {
        true
    }
}

impl<P: Program + Send + 'static> SpaceElement for IcedElement<P> {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), self.0.lock().unwrap().size)
    }

    fn is_in_input_region(&self, _point: &Point<f64, Logical>) -> bool {
        true
    }

    fn set_activate(&self, activated: bool) {
        let mut internal = self.0.lock().unwrap();
        internal.state.queue_event(Event::Window(if activated {
            WindowEvent::Focused
        } else {
            WindowEvent::Unfocused
        }));
        let _ = internal.update(true); // TODO
    }

    #[allow(clippy::map_entry)]
    fn output_enter(&self, output: &Output, _overlap: Rectangle<i32, Logical>) {
        let mut internal = self.0.lock().unwrap();
        let scale = output.current_scale().fractional_scale();
        if !internal.buffers.contains_key(&OrderedFloat(scale)) {
            let buffer_size =
                internal.size.to_f64().to_buffer(scale, Transform::Normal).to_i32_round();
            internal.buffers.insert(
                OrderedFloat(scale),
                (
                    MemoryRenderBuffer::new(
                        Fourcc::Argb8888,
                        buffer_size,
                        1,
                        Transform::Normal,
                        None,
                    ),
                    cosmic::iced::Color::TRANSPARENT,
                ),
            );
        }
        internal.outputs.insert(output.clone());
    }

    fn output_leave(&self, output: &Output) {
        self.0.lock().unwrap().outputs.remove(output);
        self.refresh();
    }

    fn z_index(&self) -> u8 {
        // meh, user-provided?
        RenderZindex::Shell as u8
    }

    fn refresh(&self) {
        let mut internal = self.0.lock().unwrap();
        // makes partial borrows easier
        let internal_ref = &mut *internal;
        internal_ref.buffers.retain(|scale, _| {
            internal_ref.outputs.iter().any(|o| o.current_scale().fractional_scale() == **scale)
        });
        let mut changed = false;
        for scale in internal_ref
            .outputs
            .iter()
            .map(|o| OrderedFloat(o.current_scale().fractional_scale()))
            .filter(|scale| !internal_ref.buffers.contains_key(scale))
            .collect::<Vec<_>>()
            .into_iter()
        {
            changed = true;
            let buffer_size =
                internal_ref.size.to_f64().to_buffer(*scale, Transform::Normal).to_i32_round();
            internal_ref.buffers.insert(
                scale,
                (
                    MemoryRenderBuffer::new(
                        Fourcc::Argb8888,
                        buffer_size,
                        1,
                        Transform::Normal,
                        None,
                    ),
                    cosmic::iced::Color::TRANSPARENT,
                ),
            );
        }
        internal.update(changed);
    }
}

impl<P, R> AsRenderElements<R> for IcedElement<P>
where
    P: Program + Send + 'static,
    R: Renderer + ImportMem,
    R::TextureId: 'static + Clone + Send,
{
    type RenderElement = MemoryRenderBufferRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        let mut internal = self.0.lock().unwrap();
        // makes partial borrows easier
        let internal_ref = &mut *internal;
        let force = if matches!(
            internal_ref.pending_update,
            Some(instant) if Instant::now().duration_since(instant) > Duration::from_millis(25)
        ) {
            true
        } else {
            false
        };
        if force {
            internal_ref.pending_update = None;
        }
        let _ = internal_ref.update(force);
        if let Some((buffer, _)) = internal_ref.buffers.get_mut(&OrderedFloat(scale.x)) {
            let size: Size<i32, BufferCoords> =
                internal_ref.size.to_f64().to_buffer(scale.x, Transform::Normal).to_i32_round();
            if size.w > 0 && size.h > 0 {
                let state_ref = &internal_ref.state;
                let mut clip_mask = tiny_skia::Mask::new(size.w as u32, size.h as u32).unwrap();
                let overlay = internal_ref.debug.overlay();

                _ = buffer.render().draw(|buf| {
                    let mut pixels =
                        tiny_skia::PixmapMut::from_bytes(buf, size.w as u32, size.h as u32)
                            .expect("Failed to create pixel map");

                    let background_color = state_ref.program().0.background_color();
                    let bounds = IcedSize::new(size.w as u32, size.h as u32);
                    let viewport = Viewport::with_physical_size(bounds, scale.x);

                    let damage = vec![cosmic::iced::Rectangle::new(
                        cosmic::iced::Point::default(),
                        viewport.logical_size(),
                    )];

                    internal_ref.renderer.draw(
                        &mut pixels,
                        &mut clip_mask,
                        &viewport,
                        &damage,
                        background_color,
                        &overlay,
                    );

                    let damage = damage
                        .into_iter()
                        .filter_map(|x| x.snap())
                        .map(|damage_rect| {
                            Rectangle::new(
                                (damage_rect.x as i32, damage_rect.y as i32).into(),
                                (bounds.width as i32, bounds.height as i32).into(),
                            )
                        })
                        .collect::<Vec<_>>();
                    state_ref.program().0.foreground(&mut pixels, &damage, scale.x as f32);

                    Result::<_, ()>::Ok(damage)
                });
            }

            if let Ok(buffer) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                &buffer,
                Some(alpha),
                Some(Rectangle::new(
                    (0., 0.).into(),
                    size.to_f64().to_logical(1., Transform::Normal).to_i32_round(),
                )),
                Some(size.to_logical(1, Transform::Normal)),
                Kind::Unspecified,
            ) {
                return vec![C::from(buffer)];
            }
        }
        Vec::new()
    }
}

impl<P: Program + Send> WaylandFocus for IcedElement<P> {
    fn wl_surface(
        &self,
    ) -> Option<Cow<'_, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>> {
        None
    }

    fn same_client_as(&self, _: &smithay::reexports::wayland_server::backend::ObjectId) -> bool {
        false
    }
}

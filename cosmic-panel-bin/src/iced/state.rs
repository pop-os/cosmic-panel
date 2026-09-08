use super::IcedProgram as Program;
use cosmic::iced::core::event::{self, Event};

use cosmic::iced::core::{Clipboard, Size, id, mouse, renderer};
use cosmic::iced::runtime::Task;
use cosmic::iced::runtime::user_interface::{self, UserInterface};

/// The execution state of a [`Program`]. It leverages caching, event
/// processing, and rendering primitive storage.
#[allow(missing_debug_implementations)]
pub struct State<P>
where
    P: Program + 'static,
{
    program: P,
    cache: Option<user_interface::Cache>,
    queued_events: Vec<Event>,
    queued_messages: Vec<P::Message>,
    mouse_interaction: mouse::Interaction,
}

impl<P> State<P>
where
    P: Program + 'static,
{
    /// Creates a new [`State`] with the provided [`Program`], initializing its
    /// primitive with the given logical bounds and renderer.
    pub fn new(id: id::Id, mut program: P, bounds: Size, renderer: &mut cosmic::Renderer) -> Self {
        let user_interface = build_user_interface(
            id,
            &mut program,
            user_interface::Cache::default(),
            renderer,
            bounds,
        );

        let cache = Some(user_interface.into_cache());

        State {
            program,
            cache,
            queued_events: Vec::new(),
            queued_messages: Vec::new(),
            mouse_interaction: mouse::Interaction::None,
        }
    }

    /// Returns a reference to the [`Program`] of the [`State`].
    pub fn program(&self) -> &P {
        &self.program
    }

    /// Queues an event in the [`State`] for processing during an [`update`].
    ///
    /// [`update`]: Self::update
    pub fn queue_event(&mut self, event: Event) {
        self.queued_events.push(event);
    }

    /// Queues a message in the [`State`] for processing during an [`update`].
    ///
    /// [`update`]: Self::update
    pub fn queue_message(&mut self, message: P::Message) {
        self.queued_messages.push(message);
    }

    /// Returns whether the event queue of the [`State`] is empty or not.
    pub fn is_queue_empty(&self) -> bool {
        self.queued_events.is_empty() && self.queued_messages.is_empty()
    }

    /// Processes all the queued events and messages, rebuilding and redrawing
    /// the widgets of the linked [`Program`] if necessary.
    ///
    /// Returns a list containing the instances of [`Event`] that were not
    /// captured by any widget, and the [`Task`] obtained from [`Program`]
    /// after updating it, only if an update was necessary.
    #[allow(clippy::too_many_arguments)] // Mirrors iced's UI update inputs.
    pub fn update(
        &mut self,
        id: id::Id,
        bounds: Size,
        cursor: mouse::Cursor,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &renderer::Style,
        clipboard: &mut dyn Clipboard,
    ) -> (Vec<Event>, Option<Task<P::Message>>) {
        let mut user_interface = build_user_interface(
            id.clone(),
            &mut self.program,
            self.cache.take().unwrap(),
            renderer,
            bounds,
        );

        let mut messages = Vec::new();

        let (state, event_statuses) =
            user_interface.update(&self.queued_events, cursor, renderer, clipboard, &mut messages);

        let uncaptured_events = self
            .queued_events
            .iter()
            .zip(event_statuses)
            .filter_map(|(event, status)| matches!(status, event::Status::Ignored).then_some(event))
            .cloned()
            .collect();

        self.queued_events.clear();
        messages.append(&mut self.queued_messages);

        let task = if messages.is_empty() {
            if let cosmic::iced::runtime::user_interface::State::Updated {
                mouse_interaction, ..
            } = state
            {
                self.mouse_interaction = mouse_interaction;
            }

            user_interface.draw(renderer, theme, style, cursor);

            self.cache = Some(user_interface.into_cache());

            None
        } else {
            // When there are messages, we are forced to rebuild twice
            // for now :^)
            let temp_cache = user_interface.into_cache();

            let tasks =
                Task::batch(messages.into_iter().map(|message| self.program.update(message)));

            let mut user_interface =
                build_user_interface(id, &mut self.program, temp_cache, renderer, bounds);

            if let cosmic::iced::runtime::user_interface::State::Updated {
                mouse_interaction, ..
            } = state
            {
                self.mouse_interaction = mouse_interaction;
            }

            user_interface.draw(renderer, theme, style, cursor);

            self.cache = Some(user_interface.into_cache());

            Some(tasks)
        };

        (uncaptured_events, task)
    }
}

fn build_user_interface<'a, P: Program>(
    _id: id::Id,
    program: &'a mut P,
    cache: user_interface::Cache,
    renderer: &mut cosmic::Renderer,
    size: Size,
) -> UserInterface<'a, P::Message, cosmic::Theme, cosmic::Renderer> {
    let view = program.view();

    UserInterface::build(view, size, cache, renderer)
}

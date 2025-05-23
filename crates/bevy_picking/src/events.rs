//! This module defines a stateful set of interaction events driven by the `PointerInput` stream
//! and the hover state of each Pointer.
//!
//! # Usage
//!
//! To receive events from this module, you must use an [`Observer`]
//! The simplest example, registering a callback when an entity is hovered over by a pointer, looks like this:
//!
//! ```rust
//! # use bevy_ecs::prelude::*;
//! # use bevy_picking::prelude::*;
//! # let mut world = World::default();
//! world.spawn_empty()
//!     .observe(|trigger: Trigger<Pointer<Over>>| {
//!         println!("I am being hovered over");
//!     });
//! ```
//!
//! Observers give us three important properties:
//! 1. They allow for attaching event handlers to specific entities,
//! 2. they allow events to bubble up the entity hierarchy,
//! 3. and they allow events of different types to be called in a specific order.
//!
//! The order in which interaction events are received is extremely important, and you can read more
//! about it on the docs for the dispatcher system: [`pointer_events`]. This system runs in
//! [`PreUpdate`](bevy_app::PreUpdate) in [`PickSet::Focus`](crate::PickSet::Focus). All pointer-event
//! observers resolve during the sync point between [`pointer_events`] and
//! [`update_interactions`](crate::focus::update_interactions).
//!
//! # Events Types
//!
//! The events this module defines fall into a few broad categories:
//! + Hovering and movement: [`Over`], [`Move`], and [`Out`].
//! + Clicking and pressing: [`Down`], [`Up`], and [`Click`].
//! + Dragging and dropping: [`DragStart`], [`Drag`], [`DragEnd`], [`DragEnter`], [`DragOver`], [`DragDrop`], [`DragLeave`].
//!
//! When received by an observer, these events will always be wrapped by the [`Pointer`] type, which contains
//! general metadata about the pointer and it's location.

use core::fmt::Debug;

use bevy_ecs::{prelude::*, system::SystemParam};
use bevy_hierarchy::Parent;
use bevy_math::Vec2;
use bevy_reflect::prelude::*;
use bevy_utils::{tracing::debug, Duration, HashMap, Instant};

use crate::{
    backend::{prelude::PointerLocation, HitData},
    focus::{HoverMap, PreviousHoverMap},
    pointer::{
        Location, PointerAction, PointerButton, PointerId, PointerInput, PointerMap, PressDirection,
    },
};

/// Stores the common data needed for all pointer events.
///
/// The documentation for the [`pointer_events`] explains the events this module exposes and
/// the order in which they fire.
#[derive(Clone, PartialEq, Debug, Reflect, Component)]
#[reflect(Component, Debug)]
pub struct Pointer<E: Debug + Clone + Reflect> {
    /// The original target of this picking event, before bubbling
    pub target: Entity,
    /// The pointer that triggered this event
    pub pointer_id: PointerId,
    /// The location of the pointer during this event
    pub pointer_location: Location,
    /// Additional event-specific data. [`DragDrop`] for example, has an additional field to describe
    /// the `Entity` that is being dropped on the target.
    pub event: E,
}

impl<E> Event for Pointer<E>
where
    E: Debug + Clone + Reflect,
{
    type Traversal = &'static Parent;

    const AUTO_PROPAGATE: bool = true;
}

impl<E: Debug + Clone + Reflect> core::fmt::Display for Pointer<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "{:?}, {:.1?}, {:.1?}",
            self.pointer_id, self.pointer_location.position, self.event
        ))
    }
}

impl<E: Debug + Clone + Reflect> core::ops::Deref for Pointer<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}

impl<E: Debug + Clone + Reflect> Pointer<E> {
    /// Construct a new `Pointer<E>` event.
    pub fn new(target: Entity, id: PointerId, location: Location, event: E) -> Self {
        Self {
            target,
            pointer_id: id,
            pointer_location: location,
            event,
        }
    }
}

/// Fires when a pointer is canceled, and it's current interaction state is dropped.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Cancel {
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires when a the pointer crosses into the bounds of the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Over {
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires when a the pointer crosses out of the bounds of the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Out {
    /// Information about the latest prior picking intersection.
    pub hit: HitData,
}

/// Fires when a pointer button is pressed over the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Down {
    /// Pointer button pressed to trigger this event.
    pub button: PointerButton,
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires when a pointer button is released over the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Up {
    /// Pointer button lifted to trigger this event.
    pub button: PointerButton,
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires when a pointer sends a pointer down event followed by a pointer up event, with the same
/// `target` entity for both events.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Click {
    /// Pointer button pressed and lifted to trigger this event.
    pub button: PointerButton,
    /// Information about the picking intersection.
    pub hit: HitData,
    /// Duration between the pointer pressed and lifted for this click
    pub duration: Duration,
}

/// Fires while a pointer is moving over the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Move {
    /// Information about the picking intersection.
    pub hit: HitData,
    /// The change in position since the last move event.
    pub delta: Vec2,
}

/// Fires when the `target` entity receives a pointer down event followed by a pointer move event.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct DragStart {
    /// Pointer button pressed and moved to trigger this event.
    pub button: PointerButton,
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires while the `target` entity is being dragged.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct Drag {
    /// Pointer button pressed and moved to trigger this event.
    pub button: PointerButton,
    /// The total distance vector of a drag, measured from drag start to the current position.
    pub distance: Vec2,
    /// The change in position since the last drag event.
    pub delta: Vec2,
}

/// Fires when a pointer is dragging the `target` entity and a pointer up event is received.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct DragEnd {
    /// Pointer button pressed, moved, and lifted to trigger this event.
    pub button: PointerButton,
    /// The vector of drag movement measured from start to final pointer position.
    pub distance: Vec2,
}

/// Fires when a pointer dragging the `dragged` entity enters the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct DragEnter {
    /// Pointer button pressed to enter drag.
    pub button: PointerButton,
    /// The entity that was being dragged when the pointer entered the `target` entity.
    pub dragged: Entity,
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires while the `dragged` entity is being dragged over the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct DragOver {
    /// Pointer button pressed while dragging over.
    pub button: PointerButton,
    /// The entity that was being dragged when the pointer was over the `target` entity.
    pub dragged: Entity,
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Fires when a pointer dragging the `dragged` entity leaves the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct DragLeave {
    /// Pointer button pressed while leaving drag.
    pub button: PointerButton,
    /// The entity that was being dragged when the pointer left the `target` entity.
    pub dragged: Entity,
    /// Information about the latest prior picking intersection.
    pub hit: HitData,
}

/// Fires when a pointer drops the `dropped` entity onto the `target` entity.
#[derive(Clone, PartialEq, Debug, Reflect)]
pub struct DragDrop {
    /// Pointer button lifted to drop.
    pub button: PointerButton,
    /// The entity that was dropped onto the `target` entity.
    pub dropped: Entity,
    /// Information about the picking intersection.
    pub hit: HitData,
}

/// Dragging state.
#[derive(Debug, Clone)]
pub struct DragEntry {
    /// The position of the pointer at drag start.
    pub start_pos: Vec2,
    /// The latest position of the pointer during this drag, used to compute deltas.
    pub latest_pos: Vec2,
}

/// An entry in the cache that drives the `pointer_events` system, storing additional data
/// about pointer button presses.
#[derive(Debug, Clone, Default)]
pub struct PointerButtonState {
    /// Stores the press location and start time for each button currently being pressed by the pointer.
    pub pressing: HashMap<Entity, (Location, Instant, HitData)>,
    /// Stores the starting and current locations for each entity currently being dragged by the pointer.
    pub dragging: HashMap<Entity, DragEntry>,
    /// Stores  the hit data for each entity currently being dragged over by the pointer.
    pub dragging_over: HashMap<Entity, HitData>,
}

/// State for all pointers.
#[derive(Debug, Clone, Default, Resource)]
pub struct PointerState {
    /// Pressing and dragging state, organized by pointer and button.
    pub pointer_buttons: HashMap<(PointerId, PointerButton), PointerButtonState>,
}

impl PointerState {
    /// Retrieves the current state for a specific pointer and button, if it has been created.
    pub fn get(&self, pointer_id: PointerId, button: PointerButton) -> Option<&PointerButtonState> {
        self.pointer_buttons.get(&(pointer_id, button))
    }

    /// Provides write access to the state of a pointer and button, creating it if it does not yet exist.
    pub fn get_mut(
        &mut self,
        pointer_id: PointerId,
        button: PointerButton,
    ) -> &mut PointerButtonState {
        self.pointer_buttons
            .entry((pointer_id, button))
            .or_default()
    }

    /// Clears all the data assoceated with all of the buttons on a pointer. Does not free the underlying memory.
    pub fn clear(&mut self, pointer_id: PointerId) {
        for button in PointerButton::iter() {
            if let Some(state) = self.pointer_buttons.get_mut(&(pointer_id, button)) {
                state.pressing.clear();
                state.dragging.clear();
                state.dragging_over.clear();
            }
        }
    }
}

/// A helper system param for accessing the picking event writers.
#[derive(SystemParam)]
pub struct PickingEventWriters<'w> {
    cancel_events: EventWriter<'w, Pointer<Cancel>>,
    click_events: EventWriter<'w, Pointer<Click>>,
    down_events: EventWriter<'w, Pointer<Down>>,
    drag_drop_events: EventWriter<'w, Pointer<DragDrop>>,
    drag_end_events: EventWriter<'w, Pointer<DragEnd>>,
    drag_enter_events: EventWriter<'w, Pointer<DragEnter>>,
    drag_events: EventWriter<'w, Pointer<Drag>>,
    drag_leave_events: EventWriter<'w, Pointer<DragLeave>>,
    drag_over_events: EventWriter<'w, Pointer<DragOver>>,
    drag_start_events: EventWriter<'w, Pointer<DragStart>>,
    move_events: EventWriter<'w, Pointer<Move>>,
    out_events: EventWriter<'w, Pointer<Out>>,
    over_events: EventWriter<'w, Pointer<Over>>,
    up_events: EventWriter<'w, Pointer<Up>>,
}

/// Dispatches interaction events to the target entities.
///
/// Within a single frame, events are dispatched in the following order:
/// + [`Out`] → [`DragLeave`].
/// + [`DragEnter`] → [`Over`].
/// + Any number of any of the following:
///   + For each movement: [`DragStart`] → [`Drag`] → [`DragOver`] → [`Move`].
///   + For each button press: [`Down`] or [`Click`] → [`Up`] → [`DragDrop`] → [`DragEnd`] → [`DragLeave`].
///   + For each pointer cancellation: [`Cancel`].
///
/// Additionally, across multiple frames, the following are also strictly
/// ordered by the interaction state machine:
/// + When a pointer moves over the target:
///   [`Over`], [`Move`], [`Out`].
/// + When a pointer presses buttons on the target:
///   [`Down`], [`Click`], [`Up`].
/// + When a pointer drags the target:
///   [`DragStart`], [`Drag`], [`DragEnd`].
/// + When a pointer drags something over the target:
///   [`DragEnter`], [`DragOver`], [`DragDrop`], [`DragLeave`].
/// + When a pointer is canceled:
///   No other events will follow the [`Cancel`] event for that pointer.
///
/// Two events -- [`Over`] and [`Out`] -- are driven only by the [`HoverMap`].
/// The rest rely on additional data from the [`PointerInput`] event stream. To
/// receive these events for a custom pointer, you must add [`PointerInput`]
/// events.
///
/// When the pointer goes from hovering entity A to entity B, entity A will
/// receive [`Out`] and then entity B will receive [`Over`]. No entity will ever
/// receive both an [`Over`] and and a [`Out`] event during the same frame.
///
/// When we account for event bubbling, this is no longer true. When focus shifts
/// between children, parent entities may receive redundant [`Out`] → [`Over`] pairs.
/// In the context of UI, this is especially problematic. Additional hierarchy-aware
/// events will be added in a future release.
///
/// Both [`Click`] and [`Up`] target the entity hovered in the *previous frame*,
/// rather than the current frame. This is because touch pointers hover nothing
/// on the frame they are released. The end effect is that these two events can
/// be received sequentally after an [`Out`] event (but always on the same frame
/// as the [`Out`] event).
///
/// Note: Though it is common for the [`PointerInput`] stream may contain
/// multiple pointer movements and presses each frame, the hover state is
/// determined only by the pointer's *final position*. Since the hover state
/// ultimately determines which entities receive events, this may mean that an
/// entity can receive events from before or after it was actually hovered.
#[allow(clippy::too_many_arguments)]
pub fn pointer_events(
    // Input
    mut input_events: EventReader<PointerInput>,
    // ECS State
    pointers: Query<&PointerLocation>,
    pointer_map: Res<PointerMap>,
    hover_map: Res<HoverMap>,
    previous_hover_map: Res<PreviousHoverMap>,
    mut pointer_state: ResMut<PointerState>,
    // Output
    mut commands: Commands,
    mut event_writers: PickingEventWriters,
) {
    // Setup utilities
    let now = Instant::now();
    let pointer_location = |pointer_id: PointerId| {
        pointer_map
            .get_entity(pointer_id)
            .and_then(|entity| pointers.get(entity).ok())
            .and_then(|pointer| pointer.location.clone())
    };

    // If the entity was hovered by a specific pointer last frame...
    for (pointer_id, hovered_entity, hit) in previous_hover_map
        .iter()
        .flat_map(|(id, hashmap)| hashmap.iter().map(|data| (*id, *data.0, data.1.clone())))
    {
        // ...but is now not being hovered by that same pointer...
        if !hover_map
            .get(&pointer_id)
            .iter()
            .any(|e| e.contains_key(&hovered_entity))
        {
            let Some(location) = pointer_location(pointer_id) else {
                debug!(
                    "Unable to get location for pointer {:?} during pointer out",
                    pointer_id
                );
                continue;
            };

            // Always send Out events
            let out_event = Pointer::new(
                hovered_entity,
                pointer_id,
                location.clone(),
                Out { hit: hit.clone() },
            );
            commands.trigger_targets(out_event.clone(), hovered_entity);
            event_writers.out_events.send(out_event);

            // Possibly send DragLeave events
            for button in PointerButton::iter() {
                let state = pointer_state.get_mut(pointer_id, button);
                state.dragging_over.remove(&hovered_entity);
                for drag_target in state.dragging.keys() {
                    let drag_leave_event = Pointer::new(
                        hovered_entity,
                        pointer_id,
                        location.clone(),
                        DragLeave {
                            button,
                            dragged: *drag_target,
                            hit: hit.clone(),
                        },
                    );
                    commands.trigger_targets(drag_leave_event.clone(), hovered_entity);
                    event_writers.drag_leave_events.send(drag_leave_event);
                }
            }
        }
    }

    // If the entity is hovered...
    for (pointer_id, hovered_entity, hit) in hover_map
        .iter()
        .flat_map(|(id, hashmap)| hashmap.iter().map(|data| (*id, *data.0, data.1.clone())))
    {
        // ...but was not hovered last frame...
        if !previous_hover_map
            .get(&pointer_id)
            .iter()
            .any(|e| e.contains_key(&hovered_entity))
        {
            let Some(location) = pointer_location(pointer_id) else {
                debug!(
                    "Unable to get location for pointer {:?} during pointer over",
                    pointer_id
                );
                continue;
            };

            // Possibly send DragEnter events
            for button in PointerButton::iter() {
                let state = pointer_state.get_mut(pointer_id, button);

                for drag_target in state
                    .dragging
                    .keys()
                    .filter(|&&drag_target| hovered_entity != drag_target)
                {
                    state.dragging_over.insert(hovered_entity, hit.clone());
                    let drag_enter_event = Pointer::new(
                        hovered_entity,
                        pointer_id,
                        location.clone(),
                        DragEnter {
                            button,
                            dragged: *drag_target,
                            hit: hit.clone(),
                        },
                    );
                    commands.trigger_targets(drag_enter_event.clone(), hovered_entity);
                    event_writers.drag_enter_events.send(drag_enter_event);
                }
            }

            // Always send Over events
            let over_event = Pointer::new(
                hovered_entity,
                pointer_id,
                location.clone(),
                Over { hit: hit.clone() },
            );
            commands.trigger_targets(over_event.clone(), hovered_entity);
            event_writers.over_events.send(over_event);
        }
    }

    // Dispatch input events...
    for PointerInput {
        pointer_id,
        location,
        action,
    } in input_events.read().cloned()
    {
        match action {
            // Pressed Button
            PointerAction::Pressed { direction, button } => {
                let state = pointer_state.get_mut(pointer_id, button);

                // The sequence of events emitted depends on if this is a press or a release
                match direction {
                    PressDirection::Down => {
                        // If it's a press, emit a Down event and mark the hovered entities as pressed
                        for (hovered_entity, hit) in hover_map
                            .get(&pointer_id)
                            .iter()
                            .flat_map(|h| h.iter().map(|(entity, data)| (*entity, data.clone())))
                        {
                            let down_event = Pointer::new(
                                hovered_entity,
                                pointer_id,
                                location.clone(),
                                Down {
                                    button,
                                    hit: hit.clone(),
                                },
                            );
                            commands.trigger_targets(down_event.clone(), hovered_entity);
                            event_writers.down_events.send(down_event);
                            // Also insert the press into the state
                            state
                                .pressing
                                .insert(hovered_entity, (location.clone(), now, hit));
                        }
                    }
                    PressDirection::Up => {
                        // Emit Click and Up events on all the previously hovered entities.
                        for (hovered_entity, hit) in previous_hover_map
                            .get(&pointer_id)
                            .iter()
                            .flat_map(|h| h.iter().map(|(entity, data)| (*entity, data.clone())))
                        {
                            // If this pointer previously pressed the hovered entity, emit a Click event
                            if let Some((_, press_instant, _)) = state.pressing.get(&hovered_entity)
                            {
                                let click_event = Pointer::new(
                                    hovered_entity,
                                    pointer_id,
                                    location.clone(),
                                    Click {
                                        button,
                                        hit: hit.clone(),
                                        duration: now - *press_instant,
                                    },
                                );
                                commands.trigger_targets(click_event.clone(), hovered_entity);
                                event_writers.click_events.send(click_event);
                            }
                            // Always send the Up event
                            let up_event = Pointer::new(
                                hovered_entity,
                                pointer_id,
                                location.clone(),
                                Up {
                                    button,
                                    hit: hit.clone(),
                                },
                            );
                            commands.trigger_targets(up_event.clone(), hovered_entity);
                            event_writers.up_events.send(up_event);
                        }

                        // Then emit the drop events.
                        for (drag_target, drag) in state.dragging.drain() {
                            // Emit DragDrop
                            for (dragged_over, hit) in state.dragging_over.iter() {
                                let drag_drop_event = Pointer::new(
                                    *dragged_over,
                                    pointer_id,
                                    location.clone(),
                                    DragDrop {
                                        button,
                                        dropped: drag_target,
                                        hit: hit.clone(),
                                    },
                                );
                                commands.trigger_targets(drag_drop_event.clone(), *dragged_over);
                                event_writers.drag_drop_events.send(drag_drop_event);
                            }
                            // Emit DragEnd
                            let drag_end_event = Pointer::new(
                                drag_target,
                                pointer_id,
                                location.clone(),
                                DragEnd {
                                    button,
                                    distance: drag.latest_pos - drag.start_pos,
                                },
                            );
                            commands.trigger_targets(drag_end_event.clone(), drag_target);
                            event_writers.drag_end_events.send(drag_end_event);
                            // Emit DragLeave
                            for (dragged_over, hit) in state.dragging_over.iter() {
                                let drag_leave_event = Pointer::new(
                                    *dragged_over,
                                    pointer_id,
                                    location.clone(),
                                    DragLeave {
                                        button,
                                        dragged: drag_target,
                                        hit: hit.clone(),
                                    },
                                );
                                commands.trigger_targets(drag_leave_event.clone(), *dragged_over);
                                event_writers.drag_leave_events.send(drag_leave_event);
                            }
                        }

                        // Finally, we can clear the state of everything relating to presses or drags.
                        state.pressing.clear();
                        state.dragging.clear();
                        state.dragging_over.clear();
                    }
                }
            }
            // Moved
            PointerAction::Moved { delta } => {
                // Triggers during movement even if not over an entity
                for button in PointerButton::iter() {
                    let state = pointer_state.get_mut(pointer_id, button);

                    // Emit DragEntry and DragStart the first time we move while pressing an entity
                    for (press_target, (location, _, hit)) in state.pressing.iter() {
                        if state.dragging.contains_key(press_target) {
                            continue; // This entity is already logged as being dragged
                        }
                        state.dragging.insert(
                            *press_target,
                            DragEntry {
                                start_pos: location.position,
                                latest_pos: location.position,
                            },
                        );
                        let drag_start_event = Pointer::new(
                            *press_target,
                            pointer_id,
                            location.clone(),
                            DragStart {
                                button,
                                hit: hit.clone(),
                            },
                        );
                        commands.trigger_targets(drag_start_event.clone(), *press_target);
                        event_writers.drag_start_events.send(drag_start_event);
                    }

                    // Emit Drag events to the entities we are dragging
                    for (drag_target, drag) in state.dragging.iter_mut() {
                        let delta = location.position - drag.latest_pos;
                        if delta == Vec2::ZERO {
                            continue; // No need to emit a Drag event if there is no movement
                        }
                        let drag_event = Pointer::new(
                            *drag_target,
                            pointer_id,
                            location.clone(),
                            Drag {
                                button,
                                distance: location.position - drag.start_pos,
                                delta,
                            },
                        );
                        commands.trigger_targets(drag_event.clone(), *drag_target);
                        event_writers.drag_events.send(drag_event);

                        // Update drag position
                        drag.latest_pos = location.position;

                        // Emit corresponding DragOver to the hovered entities
                        for (hovered_entity, hit) in hover_map
                            .get(&pointer_id)
                            .iter()
                            .flat_map(|h| h.iter().map(|(entity, data)| (*entity, data.to_owned())))
                            .filter(|(hovered_entity, _)| *hovered_entity != *drag_target)
                        {
                            let drag_over_event = Pointer::new(
                                hovered_entity,
                                pointer_id,
                                location.clone(),
                                DragOver {
                                    button,
                                    dragged: *drag_target,
                                    hit: hit.clone(),
                                },
                            );
                            commands.trigger_targets(drag_over_event.clone(), hovered_entity);
                            event_writers.drag_over_events.send(drag_over_event);
                        }
                    }
                }

                for (hovered_entity, hit) in hover_map
                    .get(&pointer_id)
                    .iter()
                    .flat_map(|h| h.iter().map(|(entity, data)| (*entity, data.to_owned())))
                {
                    // Emit Move events to the entities we are hovering
                    let move_event = Pointer::new(
                        hovered_entity,
                        pointer_id,
                        location.clone(),
                        Move {
                            hit: hit.clone(),
                            delta,
                        },
                    );
                    commands.trigger_targets(move_event.clone(), hovered_entity);
                    event_writers.move_events.send(move_event);
                }
            }
            // Canceled
            PointerAction::Canceled => {
                // Emit a Cancel to the hovered entity.
                for (hovered_entity, hit) in hover_map
                    .get(&pointer_id)
                    .iter()
                    .flat_map(|h| h.iter().map(|(entity, data)| (*entity, data.to_owned())))
                {
                    let cancel_event =
                        Pointer::new(hovered_entity, pointer_id, location.clone(), Cancel { hit });
                    commands.trigger_targets(cancel_event.clone(), hovered_entity);
                    event_writers.cancel_events.send(cancel_event);
                }
                // Clear the state for the canceled pointer
                pointer_state.clear(pointer_id);
            }
        }
    }
}

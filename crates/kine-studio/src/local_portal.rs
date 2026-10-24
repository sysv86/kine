//! A local fork of Xilem's `portal` view.
//!
//! Upstream `xilem::view::portal` stores `PhantomData<(State, Action)>`, which makes the
//! view require `State: Send + Sync` (because `WidgetView` itself is `Send + Sync`).
//! `StudioState` is deliberately `!Send` — it owns taffy nodes with raw pointers — so the
//! stock portal cannot be used. This fork stores `PhantomData<fn() -> (State, Action)>`
//! instead, which keeps the view `Send + Sync` without constraining the state type.

use std::marker::PhantomData;

use xilem::core::{MessageContext, MessageResult, Mut, View, ViewMarker};
use xilem::masonry::widgets;
use xilem::{Pod, ViewCtx, WidgetView};

pub fn portal<Child, State, Action>(child: Child) -> Portal<Child, State, Action>
where
    Child: WidgetView<State, Action>,
{
    Portal {
        child,
        phantom: PhantomData,
    }
}

pub struct Portal<V, State, Action> {
    child: V,
    phantom: PhantomData<fn() -> (State, Action)>,
}

impl<V, State, Action> ViewMarker for Portal<V, State, Action> {}

impl<Child, State, Action> View<State, Action, ViewCtx> for Portal<Child, State, Action>
where
    Child: WidgetView<State, Action>,
    State: 'static,
    Action: 'static,
{
    type Element = Pod<widgets::Portal<Child::Widget>>;
    type ViewState = Child::ViewState;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (child, child_state) = self.child.build(ctx, app_state);
        (
            ctx.create_pod(
                widgets::Portal::new(child.new_widget)
                    .constrain_horizontal(true)
                    .constrain_vertical(true),
            ),
            child_state,
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        let child_element = widgets::Portal::child_mut(&mut element);
        self.child
            .rebuild(&prev.child, view_state, ctx, child_element, app_state);
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
    ) {
        let child_element = widgets::Portal::child_mut(&mut element);
        self.child.teardown(view_state, ctx, child_element);
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageContext,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let child_element = widgets::Portal::child_mut(&mut element);
        self.child
            .message(view_state, message, child_element, app_state)
    }
}

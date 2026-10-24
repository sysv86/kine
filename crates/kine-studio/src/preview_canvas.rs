//! A preview canvas that always fills its allocated box.
//!
//! The stock `masonry::widgets::Image` reports its intrinsic pixel size as its
//! minimum, so a 640×360 frame forces the centre split to be at least 640px wide
//! and pushes a dock off-screen on narrow windows. This widget reports the
//! parent's max constraint instead (minimum 0) and paints the frame with
//! `ObjectFit::Contain`, so the preview shrinks with the window.

use masonry::accesskit::{Node, Role};
use masonry::core::{
    AccessCtx, AccessEvent, BoxConstraints, ChildrenIds, EventCtx, LayoutCtx, PaintCtx,
    PointerEvent, PropertiesMut, PropertiesRef, RegisterCtx, TextEvent, Widget,
    WidgetMut,
};
use masonry::kurbo::{Affine, Size, Stroke};
use masonry::peniko::{BlendMode, Color, Fill, ImageBrush};
use masonry::properties::ObjectFit;
use masonry::vello::Scene;
use xilem::masonry;

use xilem::core::{MessageContext, MessageResult, Mut, View, ViewMarker};
use xilem::{Pod, ViewCtx};

#[derive(Debug)]
pub struct PreviewCanvas {
    image: ImageBrush,
    background: Color,
    border: Color,
}

impl PreviewCanvas {
    pub fn new(image: ImageBrush, background: Color, border: Color) -> Self {
        Self {
            image,
            background,
            border,
        }
    }

    fn set_image(this: &mut WidgetMut<'_, Self>, image: ImageBrush) {
        this.widget.image = image;
        this.ctx.request_paint_only();
    }
}

impl Widget for PreviewCanvas {
    type Action = ();

    fn on_pointer_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &PointerEvent,
    ) {
    }

    fn on_text_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &TextEvent,
    ) {
    }

    fn on_access_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &AccessEvent,
    ) {
    }

    fn layout(
        &mut self,
        _ctx: &mut LayoutCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        bc: &BoxConstraints,
    ) -> Size {
        let max = bc.max();
        if max.width.is_finite() && max.height.is_finite() {
            max
        } else {
            Size::new(640.0, 360.0)
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        let size = ctx.size();
        let rect = size.to_rect();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.background,
            None,
            &rect,
        );

        let image_size = Size::new(
            self.image.image.width as f64,
            self.image.image.height as f64,
        );
        if !image_size.is_zero_area() {
            let transform = ObjectFit::Contain.affine_to_fill(size, image_size);
            scene.push_layer(BlendMode::default(), 1.0, Affine::IDENTITY, &rect);
            scene.draw_image(&self.image, transform);
            scene.pop_layer();
        }

        scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            self.border,
            None,
            &rect,
        );
    }

    fn accessibility_role(&self) -> Role {
        Role::Image
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}
}

#[must_use = "View values do nothing unless provided to Xilem."]
pub struct PreviewCanvasView {
    image: ImageBrush,
    background: Color,
    border: Color,
}

pub fn preview(
    image: impl Into<ImageBrush>,
    background: Color,
    border: Color,
) -> PreviewCanvasView {
    PreviewCanvasView {
        image: image.into(),
        background,
        border,
    }
}

impl ViewMarker for PreviewCanvasView {}

impl<State, Action> View<State, Action, ViewCtx> for PreviewCanvasView {
    type Element = Pod<PreviewCanvas>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (
            ctx.create_pod(PreviewCanvas::new(
                self.image.clone(),
                self.background,
                self.border,
            )),
            (),
        )
    }

    fn rebuild(
        &self,
        prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if prev.image != self.image {
            PreviewCanvas::set_image(&mut element, self.image.clone());
        }
    }

    fn teardown(&self, (): &mut Self::ViewState, _ctx: &mut ViewCtx, _: Mut<'_, Self::Element>) {}

    fn message(
        &self,
        (): &mut Self::ViewState,
        _message: &mut MessageContext,
        _element: Mut<'_, Self::Element>,
        _: &mut State,
    ) -> MessageResult<Action> {
        MessageResult::Stale
    }
}

impl PreviewCanvasView {
    pub fn fit(self, _fit: ObjectFit) -> Self {
        self
    }
}

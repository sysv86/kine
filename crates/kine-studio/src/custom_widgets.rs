//! Custom Vello-rendered widgets for Kine Studio on Xilem `0.4.0` / Masonry `0.4.0`.
//!
//! Why custom Vello widgets are essential:
//! - Standard Flexbox spacer hacks (`FlexSpacer`) distort timeline clip positions
//!   and cannot render vertical playhead needles, keyframe diamonds (`◆`), or
//!   adaptive ruler ticks at true pixel coordinates (`time_sec * zoom_pps`).
//! - These leaf `masonry::core::Widget` primitives paint crisp `kurbo` + `peniko`
//!   vector geometry directly into `masonry::vello::Scene` while remaining 100%
//!   compatible with Xilem's declarative view tree (`ZStack`, `Flex`, `Portal`).

use masonry::accesskit::{Node, Role};
use masonry::core::{
    AccessCtx, AccessEvent, BoxConstraints, ChildrenIds, EventCtx, LayoutCtx, PaintCtx,
    PointerEvent, PropertiesMut, PropertiesRef, RegisterCtx, TextEvent, Widget, WidgetMut,
};
use masonry::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Size, Stroke};
use masonry::peniko::{Color, Fill};
use masonry::vello::Scene;
use xilem::core::{MessageContext, MessageResult, Mut, View, ViewMarker};
use xilem::masonry;
use xilem::{Pod, ViewCtx};

// ── 1. TimelineRulerCanvas ──────────────────────────────────────────────────

/// Data snapshot for painting the NLE time ruler and playhead needle head.
#[derive(Debug, Clone, PartialEq)]
pub struct RulerPaintSpec {
    pub duration: f64,
    pub playhead: f64,
    pub fps: f64,
    pub bg: Color,
    pub border: Color,
    pub tick_major: Color,
    pub tick_minor: Color,
    pub playhead_color: Color,
    pub accent_glow: Color,
    pub keyframe_times: Vec<f64>,
}

#[derive(Debug)]
pub struct TimelineRulerWidget {
    spec: RulerPaintSpec,
}

impl TimelineRulerWidget {
    pub fn new(spec: RulerPaintSpec) -> Self {
        Self { spec }
    }

    fn set_spec(this: &mut WidgetMut<'_, Self>, spec: RulerPaintSpec) {
        this.widget.spec = spec;
        this.ctx.request_paint_only();
    }
}

impl Widget for TimelineRulerWidget {
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
        Size::new(
            if max.width.is_finite() { max.width } else { 600.0 },
            if max.height.is_finite() { max.height.min(24.0) } else { 24.0 },
        )
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        let size = ctx.size();
        let w = size.width.max(1.0);
        let h = size.height.max(1.0);
        let rect = size.to_rect();

        scene.fill(Fill::NonZero, Affine::IDENTITY, self.spec.bg, None, &rect);

        let dur = self.spec.duration.max(0.25);
        let fps = self.spec.fps.clamp(1.0, 120.0);
        let total_frames = (dur * fps).round() as usize;

        // Draw frame sub-ticks if not too dense
        let frame_step = if total_frames > 180 {
            10
        } else if total_frames > 90 {
            5
        } else {
            1
        };
        let mut f = 0;
        while f <= total_frames {
            let t = (f as f64) / fps;
            let u = (t / dur).clamp(0.0, 1.0);
            let x = u * w;
            let is_sec = f % (fps.round() as usize).max(1) == 0;
            let is_half = f % ((fps * 0.5).round() as usize).max(1) == 0;
            let tick_h = if is_sec {
                h * 0.58
            } else if is_half {
                h * 0.38
            } else {
                h * 0.20
            };
            let col = if is_sec {
                self.spec.tick_major
            } else {
                self.spec.tick_minor
            };
            let line = Line::new((x, h - tick_h), (x, h));
            scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, col, None, &line);
            f += frame_step;
        }

        // Draw global keyframe tick dots along top rim of ruler
        for &kf_t in &self.spec.keyframe_times {
            let u = (kf_t / dur).clamp(0.0, 1.0);
            let x = u * w;
            let diamond = diamond_path(Point::new(x, 5.0), 3.0);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                self.spec.accent_glow,
                None,
                &diamond,
            );
        }

        // Draw playhead capstone & vertical needle
        let ph_u = (self.spec.playhead / dur).clamp(0.0, 1.0);
        let ph_x = ph_u * w;

        // Soft glow halo around playhead
        let glow_rect = Rect::new(ph_x - 3.0, 0.0, ph_x + 3.0, h);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.accent_glow,
            None,
            &glow_rect,
        );

        // Playhead pentagon cap
        let mut cap = BezPath::new();
        cap.move_to((ph_x - 5.5, 1.0));
        cap.line_to((ph_x + 5.5, 1.0));
        cap.line_to((ph_x + 5.5, 8.0));
        cap.line_to((ph_x, 13.5));
        cap.line_to((ph_x - 5.5, 8.0));
        cap.close_path();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.playhead_color,
            None,
            &cap,
        );

        let needle = Line::new((ph_x, 12.0), (ph_x, h));
        scene.stroke(
            &Stroke::new(1.5),
            Affine::IDENTITY,
            self.spec.playhead_color,
            None,
            &needle,
        );

        let bottom = Line::new((0.0, h - 0.5), (w, h - 0.5));
        scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            self.spec.border,
            None,
            &bottom,
        );
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
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
pub struct TimelineRulerView {
    spec: RulerPaintSpec,
}

pub fn timeline_ruler_canvas(spec: RulerPaintSpec) -> TimelineRulerView {
    TimelineRulerView { spec }
}

impl ViewMarker for TimelineRulerView {}

impl<State, Action> View<State, Action, ViewCtx> for TimelineRulerView {
    type Element = Pod<TimelineRulerWidget>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (
            ctx.create_pod(TimelineRulerWidget::new(self.spec.clone())),
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
        if prev.spec != self.spec {
            TimelineRulerWidget::set_spec(&mut element, self.spec.clone());
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

// ── 2. TrackLaneCanvas ──────────────────────────────────────────────────────

/// Paint specification for a single NLE track lane (clip bar, top label band,
/// keyframe diamonds `◆`, optional audio waveform, and live playhead needle).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackLaneSpec {
    pub duration: f64,
    pub playhead: f64,
    pub clip_start: f64,
    pub clip_end: f64,
    pub keyframe_times: Vec<f64>,
    pub selected: bool,
    pub is_subtrack: bool,
    pub is_audio: bool,
    pub show_waveform: bool,
    pub bg_lane: Color,
    pub grid_line: Color,
    pub clip_fill: Color,
    pub clip_band: Color,
    pub clip_border: Color,
    pub kf_color: Color,
    pub kf_active_color: Color,
    pub playhead_color: Color,
    pub playhead_glow: Color,
}

#[derive(Debug)]
pub struct TrackLaneWidget {
    spec: TrackLaneSpec,
}

impl TrackLaneWidget {
    pub fn new(spec: TrackLaneSpec) -> Self {
        Self { spec }
    }

    fn set_spec(this: &mut WidgetMut<'_, Self>, spec: TrackLaneSpec) {
        this.widget.spec = spec;
        this.ctx.request_paint_only();
    }
}

impl Widget for TrackLaneWidget {
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
        let default_h = if self.spec.is_subtrack { 24.0 } else { 32.0 };
        Size::new(
            if max.width.is_finite() { max.width } else { 600.0 },
            if max.height.is_finite() {
                max.height
            } else {
                default_h
            },
        )
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        let size = ctx.size();
        let w = size.width.max(1.0);
        let h = size.height.max(1.0);
        let rect = size.to_rect();

        // 1. Lane background
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.bg_lane,
            None,
            &rect,
        );

        let dur = self.spec.duration.max(0.25);

        // 2. Subtle vertical quarter-second grid lines
        let step = 0.5;
        let count = (dur / step).ceil() as usize;
        for i in 1..=count {
            let t = (i as f64) * step;
            if t >= dur {
                break;
            }
            let gx = (t / dur) * w;
            let gl = Line::new((gx, 0.0), (gx, h));
            scene.stroke(
                &Stroke::new(1.0),
                Affine::IDENTITY,
                self.spec.grid_line,
                None,
                &gl,
            );
        }

        // 3. Clip block at exact start..end time coordinates
        let u0 = (self.spec.clip_start.max(0.0) / dur).clamp(0.0, 0.99);
        let u1 = (self.spec.clip_end.max(self.spec.clip_start + 0.08) / dur).clamp(u0 + 0.015, 1.0);
        let x0 = (u0 * w).max(1.0);
        let x1 = (u1 * w).min(w - 1.0).max(x0 + 6.0);
        let pad_y = if self.spec.is_subtrack { 4.0 } else { 3.0 };
        let clip_rect = Rect::new(x0, pad_y, x1, (h - pad_y).max(pad_y + 4.0));
        let radius = if self.spec.is_subtrack { 4.0 } else { 6.0 };
        let rrect = RoundedRect::from_rect(clip_rect, radius);

        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.clip_fill,
            None,
            &rrect,
        );

        // Top 3px accent header band on primary node clips (HyperFrames Studio style)
        if !self.spec.is_subtrack {
            let band_rect = Rect::new(x0 + 1.0, pad_y, x1 - 1.0, (pad_y + 3.5).min(h - pad_y));
            let band_rr = RoundedRect::from_rect(band_rect, 3.0);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                self.spec.clip_band,
                None,
                &band_rr,
            );

            // Trim handles on left and right edges of clip
            let lh = Rect::new(x0 + 2.0, pad_y + 6.0, x0 + 3.5, h - pad_y - 4.0);
            let rh = Rect::new(x1 - 3.5, pad_y + 6.0, x1 - 2.0, h - pad_y - 4.0);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                self.spec.clip_band,
                None,
                &lh,
            );
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                self.spec.clip_band,
                None,
                &rh,
            );
        }

        // Optional synthesized audio waveform bars inside audio track clips
        if self.spec.is_audio && self.spec.show_waveform {
            let span_w = (x1 - x0 - 8.0).max(0.0);
            let num_bars = (span_w / 4.0).floor() as usize;
            let mid_y = h * 0.54;
            for b in 0..num_bars {
                let bx = x0 + 4.0 + (b as f64) * 4.0;
                let phase = (b as f64) * 0.65;
                let amp = (0.25 + 0.65 * (phase.sin().abs() * (phase * 0.37).cos().abs()))
                    * (h * 0.34);
                let bar = Line::new((bx, mid_y - amp), (bx, mid_y + amp));
                scene.stroke(
                    &Stroke::new(1.8),
                    Affine::IDENTITY,
                    self.spec.clip_band,
                    None,
                    &bar,
                );
            }
        }

        // Clip outline
        let border_w = if self.spec.selected { 1.5 } else { 1.0 };
        scene.stroke(
            &Stroke::new(border_w),
            Affine::IDENTITY,
            self.spec.clip_border,
            None,
            &rrect,
        );

        // 4. Keyframe diamonds (`◆`) at exact timestamp coordinates
        let cy = h * 0.54;
        let diamond_r = if self.spec.is_subtrack { 4.2 } else { 4.8 };
        for &kf_t in &self.spec.keyframe_times {
            let ku = (kf_t / dur).clamp(0.0, 1.0);
            let kx = (ku * w).clamp(diamond_r + 1.0, w - diamond_r - 1.0);
            let on_playhead = (kf_t - self.spec.playhead).abs() < 0.025;
            let d_path = diamond_path(Point::new(kx, cy), diamond_r);
            let fill_col = if on_playhead {
                self.spec.kf_active_color
            } else {
                self.spec.kf_color
            };
            scene.fill(Fill::NonZero, Affine::IDENTITY, fill_col, None, &d_path);
            scene.stroke(
                &Stroke::new(1.0),
                Affine::IDENTITY,
                self.spec.bg_lane,
                None,
                &d_path,
            );
        }

        // 5. Playhead vertical needle with soft glow
        let ph_u = (self.spec.playhead / dur).clamp(0.0, 1.0);
        let ph_x = ph_u * w;
        let glow = Rect::new(ph_x - 2.5, 0.0, ph_x + 2.5, h);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.playhead_glow,
            None,
            &glow,
        );
        let ph_line = Line::new((ph_x, 0.0), (ph_x, h));
        scene.stroke(
            &Stroke::new(1.5),
            Affine::IDENTITY,
            self.spec.playhead_color,
            None,
            &ph_line,
        );

        // Bottom row separator
        let bottom = Line::new((0.0, h - 0.5), (w, h - 0.5));
        scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            self.spec.grid_line,
            None,
            &bottom,
        );
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
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
pub struct TrackLaneView {
    spec: TrackLaneSpec,
}

pub fn track_lane_canvas(spec: TrackLaneSpec) -> TrackLaneView {
    TrackLaneView { spec }
}

impl ViewMarker for TrackLaneView {}

impl<State, Action> View<State, Action, ViewCtx> for TrackLaneView {
    type Element = Pod<TrackLaneWidget>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (ctx.create_pod(TrackLaneWidget::new(self.spec.clone())), ())
    }

    fn rebuild(
        &self,
        prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if prev.spec != self.spec {
            TrackLaneWidget::set_spec(&mut element, self.spec.clone());
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

// ── 3. EasingCurvePreviewWidget ─────────────────────────────────────────────

/// Mini Vello-painted bezier/spring curve visualizer for the Inspector Motion card.
#[derive(Debug, Clone, PartialEq)]
pub struct EasingCurveSpec {
    pub easing: String,
    pub bg: Color,
    pub grid: Color,
    pub stroke: Color,
    pub endpoint: Color,
}

#[derive(Debug)]
pub struct EasingCurveWidget {
    spec: EasingCurveSpec,
}

impl EasingCurveWidget {
    pub fn new(spec: EasingCurveSpec) -> Self {
        Self { spec }
    }

    fn set_spec(this: &mut WidgetMut<'_, Self>, spec: EasingCurveSpec) {
        this.widget.spec = spec;
        this.ctx.request_paint_only();
    }
}

impl Widget for EasingCurveWidget {
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
        Size::new(
            if max.width.is_finite() { max.width } else { 120.0 },
            if max.height.is_finite() {
                max.height.min(36.0)
            } else {
                36.0
            },
        )
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        let size = ctx.size();
        let w = size.width.max(12.0);
        let h = size.height.max(12.0);
        let rrect = RoundedRect::from_rect(size.to_rect(), 5.0);
        scene.fill(Fill::NonZero, Affine::IDENTITY, self.spec.bg, None, &rrect);
        scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            self.spec.grid,
            None,
            &rrect,
        );

        let pad_x = 8.0;
        let pad_y = 6.0;
        let iw = (w - pad_x * 2.0).max(4.0);
        let ih = (h - pad_y * 2.0).max(4.0);

        let mut path = BezPath::new();
        let samples = 24usize;
        for i in 0..=samples {
            let u = (i as f64) / (samples as f64);
            let v = sample_easing(&self.spec.easing, u);
            let px = pad_x + u * iw;
            let py = pad_y + (1.0 - v.clamp(-0.2, 1.25)) * ih;
            if i == 0 {
                path.move_to((px, py));
            } else {
                path.line_to((px, py));
            }
        }
        scene.stroke(
            &Stroke::new(1.75),
            Affine::IDENTITY,
            self.spec.stroke,
            None,
            &path,
        );

        let start_c = Circle::new((pad_x, pad_y + ih), 2.5);
        let end_c = Circle::new((pad_x + iw, pad_y), 2.5);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.endpoint,
            None,
            &start_c,
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            self.spec.endpoint,
            None,
            &end_c,
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
pub struct EasingCurveView {
    spec: EasingCurveSpec,
}

pub fn easing_curve_canvas(spec: EasingCurveSpec) -> EasingCurveView {
    EasingCurveView { spec }
}

impl ViewMarker for EasingCurveView {}

impl<State, Action> View<State, Action, ViewCtx> for EasingCurveView {
    type Element = Pod<EasingCurveWidget>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (ctx.create_pod(EasingCurveWidget::new(self.spec.clone())), ())
    }

    fn rebuild(
        &self,
        prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if prev.spec != self.spec {
            EasingCurveWidget::set_spec(&mut element, self.spec.clone());
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

fn diamond_path(center: Point, r: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((center.x, center.y - r));
    p.line_to((center.x + r, center.y));
    p.line_to((center.x, center.y + r));
    p.line_to((center.x - r, center.y));
    p.close_path();
    p
}

fn sample_easing(name: &str, t: f64) -> f64 {
    let u = t.clamp(0.0, 1.0);
    let lower = name.to_ascii_lowercase();
    if lower.contains("spring") {
        1.0 - (-6.0 * u).exp() * (10.0 * u).cos()
    } else if lower.contains("inout") {
        if u < 0.5 {
            4.0 * u * u * u
        } else {
            1.0 - (-2.0 * u + 2.0).powi(3) / 2.0
        }
    } else if lower.contains("out") {
        1.0 - (1.0 - u).powi(3)
    } else if lower.contains("in") {
        u * u * u
    } else {
        u
    }
}

use std::collections::HashMap;
use std::fmt;

// ─────────────────────────────────────────────────────────────────────────────
// Scene Graph Core
// ─────────────────────────────────────────────────────────────────────────────

/// Unique identifier for a node in the scene graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Color representation (RGBA 0-255).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    /// Return a copy of this color with its alpha scaled by `opacity ∈ [0, 1]`.
    pub fn with_opacity(self, opacity: f32) -> Self {
        let factor = opacity.clamp(0.0, 1.0);
        let a = ((self.a as f32) * factor).round().clamp(0.0, 255.0) as u8;
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a,
        }
    }

    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const RED: Self = Self::rgb(255, 0, 0);
    pub const GREEN: Self = Self::rgb(0, 255, 0);
    pub const BLUE: Self = Self::rgb(0, 0, 255);
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
}

/// A rectangle in 2D space.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width * 0.5, self.y + self.height * 0.5)
    }
}

/// 2D affine transform parameters applied relative to the node's center.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transform2D {
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    /// Clockwise rotation in degrees.
    pub rotation_deg: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform2D {
    pub const IDENTITY: Self = Self {
        translate_x: 0.0,
        translate_y: 0.0,
        scale_x: 1.0,
        scale_y: 1.0,
        rotation_deg: 0.0,
    };

    pub fn is_identity(&self) -> bool {
        self.translate_x.abs() < 1e-5
            && self.translate_y.abs() < 1e-5
            && (self.scale_x - 1.0).abs() < 1e-5
            && (self.scale_y - 1.0).abs() < 1e-5
            && self.rotation_deg.abs() < 1e-5
    }
}

/// Stroke border around a shape.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StrokeStyle {
    pub color: Color,
    pub width: f32,
}

/// Drop shadow rendered behind `Rect` and `Circle` nodes.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BoxShadow {
    pub color: Color,
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
}

/// How an [`NodeKind::Image`] or [`NodeKind::Video`] fits inside its layout box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ObjectFit {
    Fill,
    Contain,
    Cover,
}

impl Default for ObjectFit {
    fn default() -> Self {
        Self::Cover
    }
}

/// A stop in a linear or radial gradient (`offset ∈ [0.0, 1.0]`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// Linear or radial color gradient fill.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Gradient {
    Linear {
        /// Angle in degrees (0 = top-to-bottom, 90 = left-to-right).
        angle_deg: f32,
        stops: Vec<GradientStop>,
    },
    Radial {
        stops: Vec<GradientStop>,
    },
}

/// Text alignment within its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// How a shader layer combines with what is already in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ShaderBlend {
    /// Standard premultiplied-alpha source-over.
    Normal,
    /// Additive (glow, bloom).
    Add,
    /// Multiplicative (shading, vignette).
    Multiply,
    /// Screen (lighten).
    Screen,
}

fn default_playback_rate() -> f64 {
    1.0
}

/// Node types in the scene graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum NodeKind {
    /// Container node (flex/grid parent or leaf).
    Container,

    /// A filled rectangle (with optional corner radius, gradient, stroke, and shadow).
    Rect {
        fill: Color,
        corner_radius: f32,
    },

    /// A circle or ellipse fitting its layout box.
    Circle {
        fill: Color,
        /// Optional explicit radius; when `None`, uses `min(width, height) / 2`.
        radius: Option<f32>,
    },

    /// An SVG vector path (`M`, `L`, `H`, `V`, `C`, `Q`, `Z`, etc.).
    Path {
        d: String,
        fill: Option<Color>,
    },

    /// A raster or procedural image node (`PNG`, `QOI`, `BMP`, `PAM`, or `pattern:checkerboard`).
    Image {
        src: String,
        fit: ObjectFit,
        corner_radius: f32,
    },

    /// A video clip node sampled at the current scene timeline position.
    Video {
        /// Path or URI to the video source (`.y4m`, `.gif`, `.kvid`, image sequence directory, `video:...`, or container file).
        src: String,
        fit: ObjectFit,
        corner_radius: f32,
        #[serde(default)]
        start: f64,
        #[serde(default)]
        trim_start: f64,
        #[serde(default)]
        trim_end: Option<f64>,
        #[serde(default = "default_playback_rate")]
        playback_rate: f64,
        #[serde(default)]
        loop_video: bool,
    },

    /// A WGSL effect layer.
    Shader {
        source: String,
        uniforms: serde_json::Map<String, serde_json::Value>,
        blend: ShaderBlend,
    },

    /// Text content.
    Text {
        content: String,
        font_family: String,
        font_size: f32,
        font_weight: u16,
        color: Color,
        align: TextAlign,
    },
}

/// A node in the scene graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub layout: taffy::Style,
    pub children: Vec<NodeId>,
    pub parent: Option<NodeId>,

    /// Runtime-computed absolute position (filled by layout pass).
    pub abs_pos: Option<Rect>,

    /// Opacity multiplier in `[0.0, 1.0]`. Multiplies down the hierarchy.
    #[serde(default = "default_opacity")]
    pub opacity: f32,

    /// 2D visual transform (translation, scale, rotation around center).
    #[serde(default)]
    pub transform: Transform2D,

    /// Optional stroke border drawn on `Rect`, `Circle`, or `Path` nodes.
    #[serde(default)]
    pub stroke: Option<StrokeStyle>,

    /// Optional drop shadow drawn behind `Rect` or `Circle` nodes.
    #[serde(default)]
    pub shadow: Option<BoxShadow>,

    /// Optional gradient fill overriding solid fill on `Rect` or `Circle` nodes.
    #[serde(default)]
    pub gradient: Option<Gradient>,

    /// Whether children are clipped to this node's layout bounds.
    #[serde(default)]
    pub clip: bool,
}

fn default_opacity() -> f32 {
    1.0
}

impl Node {
    fn new(id: NodeId, kind: NodeKind) -> Self {
        Self {
            id,
            kind,
            layout: taffy::Style::default(),
            children: Vec::new(),
            parent: None,
            abs_pos: None,
            opacity: 1.0,
            transform: Transform2D::IDENTITY,
            stroke: None,
            shadow: None,
            gradient: None,
            clip: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Scene
// ─────────────────────────────────────────────────────────────────────────────

/// A layout or visual property a script can animate over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AnimProperty {
    /// Horizontal visual offset (`inset.left`).
    OffsetX,
    /// Vertical visual offset (`inset.top`).
    OffsetY,
    Width,
    Height,
    /// Node opacity (`0.0..=1.0`).
    Opacity,
    /// 2D transform X translation (does not trigger layout reflow).
    TranslateX,
    /// 2D transform Y translation (does not trigger layout reflow).
    TranslateY,
    /// Uniform 2D scale (`scale_x` and `scale_y`).
    Scale,
    ScaleX,
    ScaleY,
    /// Clockwise rotation in degrees.
    Rotation,
    /// Rectangle corner radius.
    CornerRadius,
    /// Text font size.
    FontSize,
}

impl AnimProperty {
    /// Parse the authoring spelling.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "offsetX" | "offset_x" => Some(Self::OffsetX),
            "offsetY" | "offset_y" => Some(Self::OffsetY),
            "width" => Some(Self::Width),
            "height" => Some(Self::Height),
            "opacity" | "alpha" => Some(Self::Opacity),
            "translateX" | "translate_x" | "x" => Some(Self::TranslateX),
            "translateY" | "translate_y" | "y" => Some(Self::TranslateY),
            "scale" => Some(Self::Scale),
            "scaleX" | "scale_x" => Some(Self::ScaleX),
            "scaleY" | "scale_y" => Some(Self::ScaleY),
            "rotation" | "rotate" => Some(Self::Rotation),
            "cornerRadius" | "corner_radius" | "radius" => Some(Self::CornerRadius),
            "fontSize" | "font_size" => Some(Self::FontSize),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::OffsetX => "offsetX",
            Self::OffsetY => "offsetY",
            Self::Width => "width",
            Self::Height => "height",
            Self::Opacity => "opacity",
            Self::TranslateX => "translateX",
            Self::TranslateY => "translateY",
            Self::Scale => "scale",
            Self::ScaleX => "scaleX",
            Self::ScaleY => "scaleY",
            Self::Rotation => "rotation",
            Self::CornerRadius => "cornerRadius",
            Self::FontSize => "fontSize",
        }
    }
}

/// One animated property of one node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeAnimation {
    pub node: NodeId,
    pub property: AnimProperty,
    pub track: crate::timeline::KeyframeTrack,
    pub base: f32,
}

/// A complete scene: a tree of nodes with layout and rendering.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Scene {
    pub root: NodeId,
    pub nodes: HashMap<NodeId, Node>,
    pub width: u32,
    pub height: u32,
    /// Time-driven property tracks, applied before every layout pass.
    #[serde(default)]
    pub animations: Vec<NodeAnimation>,
    next_id: u64,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new(1920, 1080)
    }
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Self {
        let root_id = NodeId(0);
        let mut root = Node::new(root_id, NodeKind::Container);
        root.layout = taffy::Style {
            display: taffy::Display::Flex,
            size: taffy::Size {
                width: taffy::Dimension::length(width as f32),
                height: taffy::Dimension::length(height as f32),
            },
            ..Default::default()
        };

        let mut nodes = HashMap::new();
        nodes.insert(root_id, root);

        Self {
            root: root_id,
            nodes,
            width,
            height,
            animations: Vec::new(),
            next_id: 1,
        }
    }

    /// Register an animated property.
    pub fn add_animation(&mut self, animation: NodeAnimation) {
        self.animations.push(animation);
    }

    /// Evaluate every animation at `time` (seconds) and write the results into
    /// the node layouts and visual properties. Idempotent: safe to call for every frame.
    pub fn apply_time(&mut self, time: f64) {
        for animation in &self.animations {
            let value = animation.base + animation.track.eval(time) as f32;
            let Some(node) = self.nodes.get_mut(&animation.node) else {
                continue;
            };
            match animation.property {
                AnimProperty::OffsetX => {
                    node.layout.inset.left = taffy::LengthPercentageAuto::length(value);
                }
                AnimProperty::OffsetY => {
                    node.layout.inset.top = taffy::LengthPercentageAuto::length(value);
                }
                AnimProperty::Width => {
                    node.layout.size.width = taffy::Dimension::length(value.max(0.0));
                }
                AnimProperty::Height => {
                    node.layout.size.height = taffy::Dimension::length(value.max(0.0));
                }
                AnimProperty::Opacity => {
                    node.opacity = value.clamp(0.0, 1.0);
                }
                AnimProperty::TranslateX => {
                    node.transform.translate_x = value;
                }
                AnimProperty::TranslateY => {
                    node.transform.translate_y = value;
                }
                AnimProperty::Scale => {
                    node.transform.scale_x = value;
                    node.transform.scale_y = value;
                }
                AnimProperty::ScaleX => {
                    node.transform.scale_x = value;
                }
                AnimProperty::ScaleY => {
                    node.transform.scale_y = value;
                }
                AnimProperty::Rotation => {
                    node.transform.rotation_deg = value;
                }
                AnimProperty::CornerRadius => match &mut node.kind {
                    NodeKind::Rect { corner_radius, .. }
                    | NodeKind::Image { corner_radius, .. }
                    | NodeKind::Video { corner_radius, .. } => {
                        *corner_radius = value.max(0.0);
                    }
                    _ => {}
                },
                AnimProperty::FontSize => {
                    if let NodeKind::Text { font_size, .. } = &mut node.kind {
                        *font_size = value.max(1.0);
                    }
                }
            }
        }
    }

    /// Whether this scene has any time-driven properties or video nodes.
    pub fn is_animated(&self) -> bool {
        !self.animations.is_empty() || self.has_videos()
    }

    /// Whether this scene contains any WGSL shader layers.
    pub fn has_shaders(&self) -> bool {
        self.nodes
            .values()
            .any(|n| matches!(n.kind, NodeKind::Shader { .. }))
    }

    /// Whether this scene contains any video input nodes (`NodeKind::Video`).
    pub fn has_videos(&self) -> bool {
        self.nodes
            .values()
            .any(|n| matches!(n.kind, NodeKind::Video { .. }))
    }

    /// Maximum end timestamp across all animations in seconds (`0.0` if static).
    pub fn duration_seconds(&self) -> f64 {
        self.animations
            .iter()
            .map(|a| a.track.time_range().1)
            .fold(0.0f64, f64::max)
    }

    /// Add a child node under `parent`. Returns the new node's ID.
    pub fn add_node(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;

        let node = Node::new(id, kind);
        self.nodes.insert(id, node);

        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children.push(id);
        }

        if let Some(n) = self.nodes.get_mut(&id) {
            n.parent = Some(parent);
        }

        id
    }

    /// Mutate a node's layout via closure.
    pub fn with_layout<F: FnOnce(&mut taffy::Style)>(&mut self, id: NodeId, f: F) -> &mut Self {
        if let Some(node) = self.nodes.get_mut(&id) {
            f(&mut node.layout);
        }
        self
    }

    /// Get computed absolute position after layout.
    pub fn abs_rect(&self, id: NodeId) -> Option<Rect> {
        self.nodes.get(&id).and_then(|n| n.abs_pos)
    }

    /// Run Taffy layout and cache absolute positions.
    pub fn compute_layout(&mut self) -> Result<(), LayoutError> {
        let mut cache = taffy::TaffyTree::new();
        let mut id_map: HashMap<NodeId, taffy::NodeId> = HashMap::new();

        fn build(
            scene: &Scene,
            cache: &mut taffy::TaffyTree,
            id_map: &mut HashMap<NodeId, taffy::NodeId>,
            node_id: NodeId,
        ) -> Result<taffy::NodeId, LayoutError> {
            let node = scene
                .nodes
                .get(&node_id)
                .ok_or(LayoutError::NodeNotFound(node_id))?;

            let children: Result<Vec<taffy::NodeId>, LayoutError> = node
                .children
                .iter()
                .map(|&cid| build(scene, cache, id_map, cid))
                .collect();

            let children = children?;
            let taffy_id = cache
                .new_leaf(node.layout.clone())
                .map_err(|e| LayoutError::Taffy(e.to_string()))?;

            if !children.is_empty() {
                cache
                    .set_children(taffy_id, &children)
                    .map_err(|e| LayoutError::Taffy(e.to_string()))?;
            }

            id_map.insert(node_id, taffy_id);
            Ok(taffy_id)
        }

        let root_taffy = build(self, &mut cache, &mut id_map, self.root)?;

        let avail = taffy::Size {
            width: taffy::AvailableSpace::Definite(self.width as f32),
            height: taffy::AvailableSpace::Definite(self.height as f32),
        };
        cache
            .compute_layout(root_taffy, avail)
            .map_err(|e| LayoutError::Taffy(e.to_string()))?;

        fn collect(
            scene: &mut Scene,
            cache: &taffy::TaffyTree,
            id_map: &HashMap<NodeId, taffy::NodeId>,
            node_id: NodeId,
            base_x: f32,
            base_y: f32,
        ) -> Result<(), LayoutError> {
            let taffy_id = *id_map
                .get(&node_id)
                .ok_or(LayoutError::NodeNotFound(node_id))?;

            let layout = cache
                .layout(taffy_id)
                .map_err(|e| LayoutError::Taffy(e.to_string()))?;

            let abs_x = base_x + layout.location.x;
            let abs_y = base_y + layout.location.y;

            if let Some(node) = scene.nodes.get_mut(&node_id) {
                node.abs_pos = Some(Rect::new(abs_x, abs_y, layout.size.width, layout.size.height));
            }

            let children: Vec<NodeId> = scene
                .nodes
                .get(&node_id)
                .map(|n| n.children.clone())
                .unwrap_or_default();

            for child in children {
                collect(scene, cache, id_map, child, abs_x, abs_y)?;
            }

            Ok(())
        }

        collect(self, &cache, &id_map, self.root, 0.0, 0.0)?;

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Layout Errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("Node {0} not found in scene")]
    NodeNotFound(NodeId),

    #[error("Taffy layout error: {0}")]
    Taffy(String),
}

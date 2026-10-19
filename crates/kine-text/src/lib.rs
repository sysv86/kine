//! Text shaping for Kine: cosmic-text (harfrust shaping, fontdb fallback) →
//! positioned glyphs consumed by the Vello renderer.
//!
//! Determinism contract: the shaper only uses fonts explicitly loaded via
//! [`TextShaper::load_font`]; system-font fallback is intentionally not used.

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight};
use std::collections::HashMap;
use std::sync::Arc;

pub use cosmic_text::Align;

/// A glyph positioned in text-box coordinates (origin = top-left of the box).
#[derive(Debug, Clone, Copy)]
pub struct PositionedGlyph {
    /// Font-specific glyph index (not a Unicode code point).
    pub id: u32,
    /// Pen X within the text box.
    pub x: f32,
    /// Pen Y (baseline) within the text box.
    pub y: f32,
}

/// Result of shaping one text block.
#[derive(Debug, Clone, Default)]
pub struct ShapedText {
    pub glyphs: Vec<PositionedGlyph>,
    /// Width of the widest line.
    pub width: f32,
    pub line_height: f32,
    /// Baseline of the first line relative to the text-box top.
    pub first_baseline: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum ShapeError {
    #[error("font family not loaded: {0}")]
    FontNotLoaded(String),
}

/// Locale used for shaping. Fixed rather than read from the environment so
/// that the same project shapes identically on every machine.
const DETERMINISTIC_LOCALE: &str = "en-US";

/// Wraps a cosmic-text [`FontSystem`] + [`Buffer`] with an explicit font registry.
pub struct TextShaper {
    font_system: FontSystem,
    /// Created lazily on the first [`TextShaper::load_font`]: cosmic-text's
    /// shaper needs at least one registered face and panics on an empty
    /// database, which is exactly the state we want to start from.
    buffer: Option<Buffer>,
    fonts: HashMap<String, Arc<Vec<u8>>>,
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

impl TextShaper {
    /// Create a shaper with an **empty** font database.
    ///
    /// `FontSystem::new()` would call `load_system_fonts()`, pulling the host's
    /// installed fonts into shaping and making output machine-dependent. We
    /// build the database ourselves instead, so [`TextShaper::load_font`] is
    /// the only way a font can enter the pipeline. The library's script
    /// fallback lists are resolved by family name against this database, so
    /// with an empty database they match nothing.
    pub fn new() -> Self {
        let db = fontdb::Database::new();
        let font_system = FontSystem::new_with_locale_and_db(DETERMINISTIC_LOCALE.to_string(), db);
        Self {
            font_system,
            buffer: None,
            fonts: HashMap::new(),
        }
    }

    /// Number of font faces in the database. Determinism check: with `n`
    /// registered fonts this stays proportional to `n`, never hundreds.
    pub fn face_count(&self) -> usize {
        self.font_system.db().len()
    }

    /// Register a font under a family name. This is the only way fonts enter
    /// the engine (determinism: no ambient system fonts).
    pub fn load_font(&mut self, family: &str, data: Vec<u8>) {
        self.load_font_arc(family, Arc::new(data));
    }

    /// Register a font from an existing `Arc<Vec<u8>>` without cloning the font bytes.
    pub fn load_font_arc(&mut self, family: &str, data: Arc<Vec<u8>>) {
        {
            let db = self.font_system.db_mut();
            db.load_font_data((*data).clone());
            // Point the generic families at the registered font so that no
            // lookup can reach a host font that does not exist in this
            // database.
            db.set_monospace_family(family);
            db.set_sans_serif_family(family);
            db.set_serif_family(family);
        }
        self.fonts.insert(family.to_string(), data);
        if self.buffer.is_none() {
            self.buffer = Some(Buffer::new(&mut self.font_system, Metrics::new(16.0, 22.0)));
        }
    }

    /// Synchronize any fonts present in `source` that are not yet loaded here.
    pub fn sync_from(&mut self, source: &TextShaper) {
        for (family, blob) in &source.fonts {
            if !self.fonts.contains_key(family) {
                self.load_font_arc(family, Arc::clone(blob));
            }
        }
    }

    /// Raw bytes for a loaded family (used by the renderer to build GPU font data).
    pub fn font_blob(&self, family: &str) -> Option<Arc<Vec<u8>>> {
        self.fonts.get(family).cloned()
    }

    /// Families loaded so far.
    pub fn families(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.fonts.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// Shape `text` into positioned glyphs with default weight (400).
    ///
    /// * `max_width` — wrap limit; `None` means no wrapping.
    /// * Glyph positions are relative to the text box top-left; the first
    ///   baseline is at [`ShapedText::first_baseline`].
    pub fn shape(
        &mut self,
        text: &str,
        family: &str,
        size: f32,
        max_width: Option<f32>,
        align: Align,
    ) -> Result<ShapedText, ShapeError> {
        self.shape_with_weight(text, family, size, 400, max_width, align)
    }

    /// Shape `text` into positioned glyphs with explicit font weight.
    pub fn shape_with_weight(
        &mut self,
        text: &str,
        family: &str,
        size: f32,
        weight: u16,
        max_width: Option<f32>,
        align: Align,
    ) -> Result<ShapedText, ShapeError> {
        if !self.fonts.contains_key(family) {
            return Err(ShapeError::FontNotLoaded(family.to_string()));
        }
        let Some(buffer) = self.buffer.as_mut() else {
            return Err(ShapeError::FontNotLoaded(family.to_string()));
        };

        let line_height = size * 1.35;
        buffer.set_metrics(Metrics::new(size, line_height));
        buffer.set_size(max_width, None);

        let attrs = Attrs::new()
            .family(Family::Name(family))
            .weight(Weight(weight));
        buffer.set_text(text, &attrs, Shaping::Advanced, Some(align));
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut glyphs = Vec::new();
        let mut width = 0.0f32;
        let mut first_baseline = 0.0f32;
        let mut first_run = true;

        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            if first_run {
                first_baseline = run.line_y;
                first_run = false;
            }
            for g in run.glyphs {
                // Pen position within the text box: the line's baseline
                // (`run.line_y`) plus the glyph's own pen offset from it
                // (`g.y` is relative to the baseline, not to the box top).
                let x = g.x + g.font_size * g.x_offset;
                let y = run.line_y + g.y - g.font_size * g.y_offset;
                glyphs.push(PositionedGlyph {
                    id: u32::from(g.glyph_id),
                    x,
                    y,
                });
            }
        }

        Ok(ShapedText {
            glyphs,
            width,
            line_height,
            first_baseline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

    fn shaper() -> TextShaper {
        let mut s = TextShaper::new();
        s.load_font("JetBrains Mono", FONT.to_vec());
        s
    }

    #[test]
    fn shapes_glyphs_for_ascii() {
        let mut s = shaper();
        let out = s
            .shape("Hello", "JetBrains Mono", 32.0, None, Align::Left)
            .unwrap();
        // 5 ASCII chars → 5 glyphs (no ligatures in "Hello").
        assert_eq!(out.glyphs.len(), 5);
        assert!(out.width > 0.0);
        // Glyphs advance left-to-right.
        assert!(out.glyphs[1].x > out.glyphs[0].x);
    }

    #[test]
    fn wraps_at_max_width() {
        let mut s = shaper();
        let text = "word word word word word word";
        let no_wrap = s.shape(text, "JetBrains Mono", 24.0, None, Align::Left).unwrap();
        let wrapped = s
            .shape(text, "JetBrains Mono", 24.0, Some(120.0), Align::Left)
            .unwrap();
        assert!(wrapped.width <= 120.0 + 24.0);
        // Wrapping reduced the longest line versus the unwrapped run.
        assert!(wrapped.width < no_wrap.width);
    }

    #[test]
    fn unknown_family_is_an_error() {
        let mut s = shaper();
        let err = s.shape("x", "Nope", 16.0, None, Align::Left).unwrap_err();
        assert!(matches!(err, ShapeError::FontNotLoaded(_)));
    }

    #[test]
    fn only_explicitly_registered_fonts_enter_the_database() {
        // Regression guard for the determinism contract: `FontSystem::new()`
        // would load every system font, so the face count would jump into the
        // hundreds and output would differ between machines.
        let mut s = TextShaper::new();
        assert_eq!(
            s.face_count(),
            0,
            "a fresh shaper must have an empty font database"
        );
        s.load_font("JetBrains Mono", FONT.to_vec());
        let after_one = s.face_count();
        assert!(
            (1..=4).contains(&after_one),
            "one TTF should add a small number of faces, got {after_one} \
             (system fonts leaking in?)"
        );
        // A family we never registered must not resolve.
        assert!(s.font_blob("DejaVu Sans").is_none());
    }

    #[test]
    fn baseline_sits_below_the_box_top() {
        // Regression: `LayoutGlyph::y` is relative to the baseline, so the
        // line's baseline (`run.line_y`) must be added — otherwise text is
        // drawn above the box.
        let mut s = shaper();
        let out = s.shape("Ag", "JetBrains Mono", 100.0, None, Align::Left).unwrap();
        let baseline = out.glyphs[0].y;
        assert!(
            baseline > 60.0 && baseline < 130.0,
            "baseline {baseline} should be ~ascender below the box top"
        );
        assert!(out.first_baseline > 0.0);
    }
}

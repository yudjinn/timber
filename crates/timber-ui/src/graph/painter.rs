//! Drawing primitives behind one interface, so the screen, PNG export (canvas), and SVG
//! export render from the same code and look the same.

use std::fmt::Write;

use wasm_bindgen::JsValue;
use web_sys::CanvasRenderingContext2d;

/// Named UI fonts rather than `system-ui`, which follows the desktop's font setting (on
/// Linux, that can be a programming font, which is much wider). Matches `--sans` in style.css.
const FONT_FAMILY: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Noto Sans', Ubuntu, Cantarell, 'Helvetica Neue', Arial, sans-serif";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Font {
    pub size: f64,
    pub bold: bool,
}

impl Font {
    pub const fn new(size: f64, bold: bool) -> Self {
        Font { size, bold }
    }

    fn css(self) -> String {
        format!(
            "{}{}px {FONT_FAMILY}",
            if self.bold { "600 " } else { "" },
            self.size
        )
    }
}

pub trait Painter {
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: &str);
    #[allow(clippy::too_many_arguments)]
    fn rounded_rect(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        r: f64,
        fill: &str,
        stroke: Option<(&str, f64)>,
    );
    /// Straight segments `(x1, y1, x2, y2)` sharing one style.
    fn lines(&mut self, segments: &[(f64, f64, f64, f64)], stroke: &str, width: f64);
    fn curve(
        &mut self,
        from: (f64, f64),
        ctrl: (f64, f64),
        to: (f64, f64),
        stroke: &str,
        width: f64,
        dash: &[f64],
    );
    #[allow(clippy::too_many_arguments)]
    fn circle(
        &mut self,
        x: f64,
        y: f64,
        r: f64,
        fill: Option<&str>,
        stroke: Option<(&str, f64)>,
        dash: &[f64],
        alpha: f64,
    );
    /// Left-aligned text on an alphabetic baseline, optionally with a halo for legibility.
    fn text(
        &mut self,
        x: f64,
        y: f64,
        text: &str,
        font: Font,
        fill: &str,
        halo: Option<(&str, f64)>,
    );
    fn measure(&mut self, text: &str, font: Font) -> f64;
}

// ---- canvas ----

pub struct CanvasPainter<'a> {
    ctx: &'a CanvasRenderingContext2d,
    font: Option<Font>,
}

impl<'a> CanvasPainter<'a> {
    pub fn new(ctx: &'a CanvasRenderingContext2d) -> Self {
        ctx.set_text_baseline("alphabetic");
        ctx.set_text_align("left");
        ctx.set_line_join("round");
        CanvasPainter { ctx, font: None }
    }

    fn dash(&self, pattern: &[f64]) {
        let arr: js_sys::Array = pattern.iter().map(|p| JsValue::from_f64(*p)).collect();
        let _ = self.ctx.set_line_dash(&arr);
    }

    fn set_font(&mut self, font: Font) {
        if self.font != Some(font) {
            self.ctx.set_font(&font.css());
            self.font = Some(font);
        }
    }
}

impl Painter for CanvasPainter<'_> {
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: &str) {
        self.ctx.set_fill_style_str(fill);
        self.ctx.fill_rect(x, y, w, h);
    }

    fn rounded_rect(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        r: f64,
        fill: &str,
        stroke: Option<(&str, f64)>,
    ) {
        self.ctx.begin_path();
        let _ = self.ctx.round_rect_with_f64(x, y, w, h, r);
        self.ctx.set_fill_style_str(fill);
        self.ctx.fill();
        if let Some((color, width)) = stroke {
            self.ctx.set_stroke_style_str(color);
            self.ctx.set_line_width(width);
            self.ctx.stroke();
        }
    }

    fn lines(&mut self, segments: &[(f64, f64, f64, f64)], stroke: &str, width: f64) {
        self.ctx.set_stroke_style_str(stroke);
        self.ctx.set_line_width(width);
        self.ctx.begin_path();
        for &(x1, y1, x2, y2) in segments {
            self.ctx.move_to(x1, y1);
            self.ctx.line_to(x2, y2);
        }
        self.ctx.stroke();
    }

    fn curve(
        &mut self,
        from: (f64, f64),
        ctrl: (f64, f64),
        to: (f64, f64),
        stroke: &str,
        width: f64,
        dash: &[f64],
    ) {
        self.ctx.set_stroke_style_str(stroke);
        self.ctx.set_line_width(width);
        self.dash(dash);
        self.ctx.begin_path();
        self.ctx.move_to(from.0, from.1);
        self.ctx.quadratic_curve_to(ctrl.0, ctrl.1, to.0, to.1);
        self.ctx.stroke();
        self.dash(&[]);
    }

    fn circle(
        &mut self,
        x: f64,
        y: f64,
        r: f64,
        fill: Option<&str>,
        stroke: Option<(&str, f64)>,
        dash: &[f64],
        alpha: f64,
    ) {
        self.ctx.set_global_alpha(alpha);
        self.ctx.begin_path();
        let _ = self.ctx.arc(x, y, r, 0.0, std::f64::consts::TAU);
        if let Some(fill) = fill {
            self.ctx.set_fill_style_str(fill);
            self.ctx.fill();
        }
        if let Some((color, width)) = stroke {
            self.ctx.set_stroke_style_str(color);
            self.ctx.set_line_width(width);
            self.dash(dash);
            self.ctx.stroke();
            self.dash(&[]);
        }
        self.ctx.set_global_alpha(1.0);
    }

    fn text(
        &mut self,
        x: f64,
        y: f64,
        text: &str,
        font: Font,
        fill: &str,
        halo: Option<(&str, f64)>,
    ) {
        self.set_font(font);
        if let Some((color, width)) = halo {
            self.ctx.set_stroke_style_str(color);
            self.ctx.set_line_width(width);
            let _ = self.ctx.stroke_text(text, x, y);
        }
        self.ctx.set_fill_style_str(fill);
        let _ = self.ctx.fill_text(text, x, y);
    }

    fn measure(&mut self, text: &str, font: Font) -> f64 {
        self.set_font(font);
        self.ctx
            .measure_text(text)
            .map(|m| m.width())
            .unwrap_or(text.len() as f64 * font.size * 0.55)
    }
}

// ---- SVG ----

/// Builds an SVG document. Text is measured with a canvas so layout (label culling) matches.
pub struct SvgPainter<'a> {
    body: String,
    measurer: CanvasPainter<'a>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn dash_attr(dash: &[f64]) -> String {
    if dash.is_empty() {
        String::new()
    } else {
        format!(
            " stroke-dasharray=\"{}\"",
            dash.iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

impl<'a> SvgPainter<'a> {
    pub fn new(measure_ctx: &'a CanvasRenderingContext2d) -> Self {
        SvgPainter {
            body: String::new(),
            measurer: CanvasPainter::new(measure_ctx),
        }
    }

    pub fn finish(self, w: f64, h: f64) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\" \
             font-family=\"{}\">\n{}</svg>\n",
            esc(FONT_FAMILY),
            self.body
        )
    }
}

impl Painter for SvgPainter<'_> {
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: &str) {
        let _ = writeln!(
            self.body,
            "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" fill=\"{fill}\"/>"
        );
    }

    fn rounded_rect(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        r: f64,
        fill: &str,
        stroke: Option<(&str, f64)>,
    ) {
        let stroke = stroke.map_or(String::new(), |(c, sw)| {
            format!(" stroke=\"{c}\" stroke-width=\"{sw}\"")
        });
        let _ = writeln!(
            self.body,
            "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" rx=\"{r}\" fill=\"{fill}\"{stroke}/>"
        );
    }

    fn lines(&mut self, segments: &[(f64, f64, f64, f64)], stroke: &str, width: f64) {
        if segments.is_empty() {
            return;
        }
        let mut d = String::new();
        for &(x1, y1, x2, y2) in segments {
            let _ = write!(d, "M{x1:.1} {y1:.1}L{x2:.1} {y2:.1}");
        }
        let _ = writeln!(
            self.body,
            "<path d=\"{d}\" stroke=\"{stroke}\" stroke-width=\"{width}\" fill=\"none\"/>"
        );
    }

    fn curve(
        &mut self,
        from: (f64, f64),
        ctrl: (f64, f64),
        to: (f64, f64),
        stroke: &str,
        width: f64,
        dash: &[f64],
    ) {
        let _ = writeln!(
            self.body,
            "<path d=\"M{:.1} {:.1}Q{:.1} {:.1} {:.1} {:.1}\" stroke=\"{stroke}\" stroke-width=\"{width}\" fill=\"none\"{}/>",
            from.0,
            from.1,
            ctrl.0,
            ctrl.1,
            to.0,
            to.1,
            dash_attr(dash)
        );
    }

    fn circle(
        &mut self,
        x: f64,
        y: f64,
        r: f64,
        fill: Option<&str>,
        stroke: Option<(&str, f64)>,
        dash: &[f64],
        alpha: f64,
    ) {
        let fill = fill.unwrap_or("none");
        let stroke = stroke.map_or(String::new(), |(c, w)| {
            format!(" stroke=\"{c}\" stroke-width=\"{w}\"{}", dash_attr(dash))
        });
        let opacity = if alpha < 1.0 {
            format!(" opacity=\"{alpha}\"")
        } else {
            String::new()
        };
        let _ = writeln!(
            self.body,
            "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{r:.1}\" fill=\"{fill}\"{stroke}{opacity}/>"
        );
    }

    fn text(
        &mut self,
        x: f64,
        y: f64,
        text: &str,
        font: Font,
        fill: &str,
        halo: Option<(&str, f64)>,
    ) {
        let weight = if font.bold {
            " font-weight=\"600\""
        } else {
            ""
        };
        let halo = halo.map_or(String::new(), |(c, w)| {
            format!(" stroke=\"{c}\" stroke-width=\"{w}\" stroke-linejoin=\"round\" paint-order=\"stroke\"")
        });
        let _ = writeln!(
            self.body,
            "<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"{}\"{weight} fill=\"{fill}\"{halo}>{}</text>",
            font.size,
            esc(text)
        );
    }

    fn measure(&mut self, text: &str, font: Font) -> f64 {
        self.measurer.measure(text, font)
    }
}

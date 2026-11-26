//! Broadcast-mast intro ported from the GPLv3 Android RadioIntro and IntroPacing.

use crate::{
    canvas::{Align, Area, Canvas, Color, Gradient, alpha, rgb},
    model::label,
    ui::{HEIGHT, Palette},
};
use std::time::{Duration, Instant};

pub const FRAME_MS: u32 = 15;
pub const END_MS: f32 = 1720.;
const DISMISS_MS: f32 = 340.;
const OPEN_START: f32 = 1380.;
const IRIS_CLEAR: f32 = 0.68;
const TIP: (f32, f32) = (60., 28.);
const APEX: f32 = 44.;
const WARM: Color = rgb(0xe1c992);
const WHITE: Color = rgb(0xffffff);

#[derive(Clone, Copy)]
pub struct Frame {
    millis: f32,
    dismiss: f32,
}

impl Frame {
    pub fn at(millis: f32) -> Self {
        Self {
            millis,
            dismiss: 0.,
        }
    }
}

pub struct Animation {
    frame: Frame,
    started: Instant,
    last: Instant,
    dismissal: Option<Instant>,
}

impl Animation {
    pub fn new(now: Instant) -> Self {
        Self {
            frame: Frame::at(0.),
            started: now,
            last: now,
            dismissal: None,
        }
    }

    pub fn frame(&self) -> Frame {
        self.frame
    }

    pub fn dismiss(&mut self, now: Instant) {
        self.dismissal.get_or_insert(now);
    }

    pub fn advance(&mut self, now: Instant) -> bool {
        let elapsed = |instant| now.saturating_duration_since(instant).as_secs_f32() * 1000.;
        // Skip at most two frames after a short stall. A slow machine still finishes.
        let gap = elapsed(self.last).min(2. * FRAME_MS as f32);
        self.frame.millis = (self.frame.millis + gap).max(elapsed(self.started) - END_MS * 0.5);
        self.last = now;
        if let Some(start) = self.dismissal {
            self.frame.dismiss = (elapsed(start) / DISMISS_MS).min(1.);
        }
        self.frame.millis >= END_MS || self.frame.dismiss >= 1.
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

const STANDARD: Curve = Curve(0.4, 0., 0.2, 1.);
const DECELERATE: Curve = Curve(0., 0., 0.2, 1.);
const ACCELERATE: Curve = Curve(0.4, 0., 1., 1.);
const SPRING: Curve = Curve(0.22, 1.18, 0.36, 1.);
const BACK: Curve = Curve(0.34, 1.56, 0.64, 1.);

#[derive(Clone, Copy)]
struct Curve(f32, f32, f32, f32);

impl Curve {
    fn at(self, x: f32) -> f32 {
        if x <= 0. || x >= 1. {
            return x.clamp(0., 1.);
        }
        let value = |t: f32, p: f32, q: f32| {
            let u = 1. - t;
            3. * u * u * t * p + 3. * u * t * t * q + t * t * t
        };
        let (mut lo, mut hi) = (0., 1.);
        for _ in 0..17 {
            let mid = (lo + hi) * 0.5;
            if value(mid, self.0, self.2) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        value((lo + hi) * 0.5, self.1, self.3)
    }
}

fn span(t: f32, from: f32, to: f32) -> f32 {
    ((t - from) / (to - from)).clamp(0., 1.)
}
fn ease(t: f32, from: f32, to: f32) -> f32 {
    STANDARD.at(span(t, from, to))
}
fn charge(t: f32) -> f32 {
    ease(t, 740., 860.) * (1. - ease(t, 940., 1120.))
}

fn mix(from: Color, to: Color, fraction: f32) -> Color {
    [0, 8, 16, 24]
        .into_iter()
        .map(|shift| {
            let a = ((from >> shift) & 255) as f32;
            let b = ((to >> shift) & 255) as f32;
            (((a + (b - a) * fraction).round() as u32) & 255) << shift
        })
        .fold(0, |a, b| a | b)
}

fn charged(color: Color, accent: Color) -> Color {
    (color & 0xff000000)
        | [0, 8, 16]
            .into_iter()
            .map(|shift| (((color >> shift) & 255) * ((accent >> shift) & 255) / 255) << shift)
            .fold(0, |a, b| a | b)
}

struct Metal {
    dark: Color,
    body: Color,
    light: Color,
}

impl Metal {
    fn tone(&self, level: f32) -> Color {
        if level < 0.5 {
            mix(self.dark, self.body, level * 2.)
        } else {
            mix(self.body, self.light, (level - 0.5) * 2.)
        }
    }
}

const TITLE_HEIGHT: f32 = 29.;
const CAPTION_HEIGHT: f32 = 17.;
const MAST_HEIGHT: f32 = 150.;
const MAST_SCALE: f32 = MAST_HEIGHT / 102.;
const TOP: f32 = (HEIGHT as f32
    - (MAST_HEIGHT + 18. + TITLE_HEIGHT + 10. + 9. + 2. + 10. + CAPTION_HEIGHT))
    * 0.5;
const TITLE_TOP: f32 = TOP + MAST_HEIGHT + 18.;
const DIAL_Y: f32 = TITLE_TOP + TITLE_HEIGHT + 10. + 9.;
const CAPTION_TOP: f32 = DIAL_Y + 12.;

pub struct Scene {
    frame: Frame,
    viewport: Area,
    open: f32,
    reveal: f32,
    leave: f32,
    strength: f32,
}

impl Scene {
    pub fn new(frame: Frame, width: f32) -> Self {
        let t = frame.millis;
        let open = span(t, OPEN_START, END_MS).max(frame.dismiss);
        let leave = span(t, 1320., 1640.).max(frame.dismiss);
        Self {
            frame,
            viewport: Area::new(0., 0., width, HEIGHT as f32),
            open,
            reveal: ease(t, 120., 420.),
            leave,
            strength: 1. - STANDARD.at(leave),
        }
    }

    pub fn content(&self) -> Option<((f32, f32), f32)> {
        (self.open > 0.).then(|| (self.tip(), 0.92 + 0.08 * DECELERATE.at(self.open)))
    }

    fn tip(&self) -> (f32, f32) {
        (self.viewport.w * 0.5, TOP + TIP.1 * MAST_SCALE)
    }

    pub fn draw(&self, canvas: &mut Canvas, palette: Palette, language: &str) {
        let open = STANDARD.at(self.open);
        self.backdrop(canvas, palette.window, open);
        if self.reveal == 0. || self.strength == 0. {
            return;
        }

        let accent = palette.accent;
        let display = self.reveal * self.strength * (1. - open);
        let viewport = self.viewport;
        let edge = viewport.h.min(viewport.w);
        glow(
            canvas,
            Area::around((viewport.w * 0.94, viewport.h * 0.06), edge * 1.10),
            [
                (0., alpha(accent, 0.09 * display)),
                (0.5, alpha(accent, 0.025 * display)),
                (1., 0),
            ],
        );
        glow(
            canvas,
            Area::around((viewport.w * 0.03, viewport.h * 0.98), edge * 0.85),
            [(0., alpha(WARM, 0.035 * display)), (1., 0)],
        );

        let flight = 1. + 0.32 * ACCELERATE.at(self.leave);
        canvas.transformed(self.tip(), (flight, flight), self.tip(), |canvas| {
            self.foreground(canvas, palette, language);
        });
    }

    fn backdrop(&self, canvas: &Canvas, color: Color, open: f32) {
        if open == 0. {
            canvas.clear(color);
        } else if open < 1. {
            let (x, y) = self.tip();
            let reach = x.hypot(y.max(self.viewport.h - y));
            let opening = Area::around((x, y), (reach * open / IRIS_CLEAR).max(1.));
            // Overlap the two antialiased edges so the feather has no visible seam.
            let mask = opening.inset(1.0, 1.0);
            canvas.outside_ellipse(
                self.viewport,
                if mask.w > 0. { mask } else { opening },
                color,
            );
            glow(
                canvas,
                opening,
                [
                    (0., alpha(color, 0.)),
                    (IRIS_CLEAR, alpha(color, 0.)),
                    (1., color),
                ],
            );
        }
    }

    fn foreground(&self, canvas: &mut Canvas, p: Palette, language: &str) {
        let t = self.frame.millis;
        let out = self.strength;
        let charge_now = charge(t);
        let center = self.tip().0;
        let glow_center = (center, TOP + 40. * MAST_SCALE);
        let radius = MAST_HEIGHT * 1.25;
        let opacity = self.reveal * out;
        glow(
            canvas,
            Area::around(glow_center, radius),
            [
                (0., alpha(mix(p.window, p.accent, 0.30), opacity)),
                (0.5, alpha(mix(p.window, p.accent, 0.13), 0.64 * opacity)),
                (1., 0),
            ],
        );
        if charge_now > 0. {
            glow(
                canvas,
                Area::around(glow_center, radius * 0.9),
                [(0., alpha(p.accent, 0.42 * charge_now * out)), (1., 0)],
            );
        }
        for (begin, brightness) in [(760., 1.), (880., 0.7)] {
            let pulse = span(t, begin, begin + 420.);
            if pulse > 0. && pulse < 1. {
                let radius = MAST_HEIGHT * (0.10 + 1.05 * STANDARD.at(pulse));
                canvas.arcs(
                    Area::around(self.tip(), radius),
                    &[0.],
                    360.,
                    1.5,
                    alpha(p.accent, 0.5 * (1. - pulse).powi(2) * brightness * out),
                );
            }
        }

        let shadow = Area::new(
            center - MAST_HEIGHT * 0.27,
            TOP + 100.4 * MAST_SCALE - MAST_HEIGHT * 0.03,
            MAST_HEIGHT * 0.54,
            MAST_HEIGHT * 0.06,
        );
        glow(
            canvas,
            shadow,
            [(0., alpha(rgb(0), 0.65 * opacity)), (1., 0)],
        );
        let sy = MAST_SCALE * (0.91 + 0.09 * self.reveal);
        let sx = sy * (0.90 + 0.10 * self.reveal);
        let origin = (
            center - 60. * sx,
            TOP + (MAST_HEIGHT - 102. * sy) * 0.5 + 6. * (1. - self.reveal),
        );
        let steel = mix(p.muted, p.outline, 0.30);
        let metal = Metal {
            dark: mix(mix(steel, p.window, 0.78), p.accent, 0.12),
            body: mix(steel, p.window, 0.32),
            light: mix(steel, WHITE, 0.86),
        };
        canvas.transformed(origin, (sx, sy), (0., 0.), |canvas| {
            draw_waves(canvas, t, &metal, p.accent, out);
            draw_mast(canvas, t, &metal, p.accent, opacity);
            draw_tip(canvas, t, &metal, p.accent, out);
        });

        let title = ease(t, 340., 600.);
        if title > 0. {
            canvas.text(
                Area::new(
                    16.,
                    TITLE_TOP + 6. * (1. - title),
                    self.viewport.w - 32.,
                    TITLE_HEIGHT,
                ),
                "Desktop Radio",
                24,
                true,
                alpha(mix(p.text, p.accent, charge_now * 0.85), title * out),
                Align::Center,
            );
        }
        draw_dial(canvas, center, t, p, charge_now, out);
        let caption = ease(t, 500., 760.);
        if caption > 0. {
            canvas.text(
                Area::new(
                    16.,
                    CAPTION_TOP + 6. * (1. - caption),
                    self.viewport.w - 32.,
                    CAPTION_HEIGHT,
                ),
                label("Internet radio", language),
                14,
                false,
                alpha(p.muted, caption * out),
                Align::Center,
            );
        }
    }
}

fn glow<const N: usize>(canvas: &Canvas, area: Area, stops: [(f32, Color); N]) {
    if let Some(gradient) = canvas.radial_gradient(area, area.center(), stops) {
        canvas.gradient_ellipse(area, &gradient);
    }
}

fn glint(t: f32, accent: Color, opacity: f32) -> Option<Gradient> {
    if !(380. ..=1080.).contains(&t) || opacity <= 0. {
        return None;
    }
    // Use one stripe on the original 30:-17 axis, with transparent padding to prevent brush tiling.
    let x = -50. + 220. * span(t, 380., 1080.);
    let first = (x + 150.) / 450.;
    let last = first + (30. * 30. + 17. * 17.) / (30. * 450.);
    let shine = alpha(mix(WHITE, accent, 0.10), opacity);
    let transparent = alpha(shine, 0.);
    Gradient::linear(
        (-150., 85.),
        (300., -170.),
        [
            (0., transparent),
            (first, transparent),
            ((first + last) * 0.5, shine),
            (last, transparent),
            (1., transparent),
        ],
    )
}

fn draw_waves(canvas: &Canvas, t: f32, metal: &Metal, accent: Color, out: f32) {
    for (index, (radius, width, strength)) in [
        (11.5, 3.0, 1.),
        (19.5, 2.7, 0.9),
        (27.5, 2.4, 0.78),
        (35.5, 2.1, 0.66),
    ]
    .into_iter()
    .enumerate()
    {
        let start = 260. + index as f32 * 55.;
        let opacity = ease(t, start, start + 160.) * out * strength;
        if opacity <= 0. {
            continue;
        }
        let open = SPRING.at(span(t, start, start + 360.));
        let area = Area::around(TIP, radius * (0.55 + 0.45 * open));
        let half = 40. * (0.12 + 0.88 * open);
        let starts = [180. - half, -half];
        let sweep = 2. * half;
        let reach = radius * 0.66;
        let wave_charge = charge(t - (index + 1) as f32 * 35.);
        let stops = [
            (0., 0.95, 0.80),
            (0.30, 0.70, 0.42),
            (0.50, 0.40, 0.28),
            (0.70, 0.70, 0.42),
            (1., 0.92, 0.75),
        ];
        for (tone, tint) in [(opacity, false), (opacity * wave_charge * 0.82, true)] {
            if tone <= 0. {
                continue;
            }
            let stops = stops.map(|(position, level, alpha_factor)| {
                let color = metal.tone(level);
                (
                    position,
                    alpha(
                        if tint { charged(color, accent) } else { color },
                        alpha_factor * tone,
                    ),
                )
            });
            if let Some(gradient) =
                Gradient::linear((0., TIP.1 - reach), (0., TIP.1 + reach), stops)
            {
                canvas.gradient_arcs(area, &starts, sweep, width, &gradient);
            }
        }
        canvas.arcs(
            area,
            &starts,
            sweep,
            0.45,
            alpha(metal.tone(0.98), 0.55 * opacity),
        );
        if let Some(shine) = glint(t, accent, (0.66 - 0.30 * wave_charge) * opacity) {
            canvas.gradient_arcs(area, &starts, sweep, width, &shine);
        }
    }
}

const LEGS: [[(f32, f32); 4]; 2] = [
    [(59.3, APEX), (60.5, APEX + 1.), (51.8, 99.), (47.6, 99.)],
    [(60.7, APEX), (59.5, APEX + 1.), (68.2, 99.), (72.4, 99.)],
];
const PLINTH: Area = Area::new(43.5, 98.4, 33., 3.);

fn draw_mast(canvas: &Canvas, t: f32, metal: &Metal, accent: Color, opacity: f32) {
    let charge_now = charge(t);
    for (strength, tint) in [(opacity, false), (opacity * charge_now * 0.82, true)] {
        if strength <= 0. {
            continue;
        }
        let tone = |level, a| {
            let color = metal.tone(level);
            alpha(
                if tint { charged(color, accent) } else { color },
                a * strength,
            )
        };
        if let Some(depth) = Gradient::linear(
            (48., APEX),
            (72., 100.),
            [
                (0., tone(0.95, 0.9)),
                (0.5, tone(0., 0.9)),
                (1., tone(0.5, 0.9)),
            ],
        ) {
            for leg in LEGS {
                canvas.gradient_polygon(leg.map(|(x, y)| (x + 1.2, y + 2.)), &depth);
            }
        }
        if let Some(bevel) = Gradient::linear(
            (47., APEX),
            (73., 100.),
            [
                (0., tone(1., 1.)),
                (0.38, tone(0.80, 1.)),
                (0.57, tone(0.30, 1.)),
                (0.72, tone(0.97, 1.)),
                (1., tone(0.56, 1.)),
            ],
        ) {
            for leg in LEGS {
                canvas.gradient_polygon(leg, &bevel);
            }
            canvas.gradient_panel(PLINTH, 1.4, &bevel);
        }
        for (from, to) in [
            ((57.4, 64.), (62.6, 64.)),
            ((54.6, 82.), (65.4, 82.)),
            ((57.4, 64.), (65.4, 82.)),
            ((62.6, 64.), (54.6, 82.)),
        ] {
            canvas.line(from, to, tone(0.82, 0.85), 1.);
        }
        if let Some(rod) = Gradient::linear(
            (59., 0.),
            (61., 0.),
            [
                (0., tone(0.55, 1.)),
                (0.5, tone(0.98, 1.)),
                (1., tone(0.62, 1.)),
            ],
        ) {
            canvas.gradient_line((TIP.0, TIP.1 + 2.), (TIP.0, APEX + 1.), 1.7, &rod);
        }
    }
    if let Some(shine) = glint(t, accent, (0.66 - 0.30 * charge_now) * opacity) {
        for leg in LEGS {
            canvas.gradient_polygon(leg, &shine);
        }
        canvas.gradient_panel(PLINTH, 1.4, &shine);
        canvas.gradient_line((TIP.0, TIP.1 + 2.), (TIP.0, APEX + 1.), 1.7, &shine);
    }
}

fn draw_tip(canvas: &mut Canvas, t: f32, metal: &Metal, accent: Color, out: f32) {
    let opacity = ease(t, 220., 400.) * out;
    if opacity <= 0. {
        return;
    }
    let pop = BACK.at(span(t, 220., 480.));
    let charge_now = charge(t);
    canvas.transformed(TIP, (pop, pop), TIP, |canvas| {
        let halo = (0.35 + 0.65 * charge_now) * opacity;
        glow(
            canvas,
            Area::around(TIP, 11.),
            [
                (0., alpha(accent, 0.55 * halo)),
                (0.45, alpha(accent, 0.16 * halo)),
                (1., 0),
            ],
        );
        let highlight = (TIP.0 - 1.3, TIP.1 - 1.5);
        for (strength, tint) in [(opacity, false), (opacity * charge_now * 0.82, true)] {
            if strength <= 0. {
                continue;
            }
            let stops = [(0., 1.), (0.55, 0.66), (1., 0.16)].map(|(position, level)| {
                let color = metal.tone(level);
                (
                    position,
                    alpha(if tint { charged(color, accent) } else { color }, strength),
                )
            });
            if let Some(gloss) =
                canvas.radial_gradient(Area::around(highlight, 5.6), highlight, stops)
            {
                canvas.gradient_ellipse(Area::around(TIP, 3.8), &gloss);
            }
        }
    });
}

fn draw_dial(canvas: &Canvas, center: f32, t: f32, p: Palette, charge_now: f32, out: f32) {
    let drawn = ease(t, 420., 700.);
    if drawn <= 0. {
        return;
    }
    const HALF: f32 = 90.;
    let tint = alpha(p.accent, (0.55 + 0.45 * charge_now) * out);
    let clear = alpha(p.accent, 0.);
    if let Some(rule) = Gradient::linear(
        (center - HALF, 0.),
        (center + HALF, 0.),
        [(0., clear), (0.5, tint), (1., clear)],
    ) {
        canvas.gradient_panel(
            Area::new(center - HALF * drawn, DIAL_Y, 2. * HALF * drawn, 2.),
            0.,
            &rule,
        );
    }
    for tick in -10_i32..=10 {
        let distance = tick.abs() as f32 / 10.;
        let appear = ((drawn - distance) * 6.).clamp(0., 1.);
        if appear <= 0. {
            continue;
        }
        let x = center + tick as f32 * 9.;
        let major = tick % 5 == 0;
        canvas.line(
            (x, DIAL_Y - if major { 8. } else { 4.5 }),
            (x, DIAL_Y - 1.5),
            alpha(
                p.accent,
                (if major { 0.55 } else { 0.34 }) * (1. - distance * distance) * appear * out,
            ),
            if major { 1.3 } else { 1. },
        );
    }
    let visible = ease(t, 450., 650.) * out;
    if visible <= 0. {
        return;
    }
    let x = center - HALF + 2. * HALF * (0.06 + 0.44 * BACK.at(span(t, 450., 780.)));
    glow(
        canvas,
        Area::around((x, DIAL_Y - 3.), 16.),
        [
            (
                0.,
                alpha(p.accent, 0.45 * (0.45 + 0.55 * charge_now) * visible),
            ),
            (1., 0),
        ],
    );
    canvas.panel(
        Area::new(x - 1., DIAL_Y - 13., 2., 18.),
        1.,
        alpha(mix(WHITE, p.accent, 0.55 + 0.45 * charge_now), visible),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{COMPACT_WIDTH, WIDTH};

    #[test]
    fn every_open_hides_player_until_iris() {
        for width in [WIDTH as f32, COMPACT_WIDTH as f32] {
            for millis in [0., 15., 120., 240., 800., 1320., 1380.] {
                assert!(Scene::new(Frame::at(millis), width).content().is_none());
            }
            let (_, scale) = Scene::new(Frame::at(1450.), width).content().unwrap();
            assert!((0.92..1.).contains(&scale));
            let (tip, scale) = Scene::new(Frame::at(END_MS), width).content().unwrap();
            assert_eq!((tip.0, scale), (width * 0.5, 1.));
        }
    }

    #[test]
    fn late_frame_stays_bounded_and_tap_uses_original_exit_duration() {
        let start = Instant::now();
        let mut animation = Animation::new(start);
        assert!(!animation.advance(start + Duration::from_millis(250)));
        assert_eq!(animation.frame().millis, 2. * FRAME_MS as f32);
        let tapped = start + Duration::from_millis(300);
        animation.dismiss(tapped);
        animation.dismiss(tapped + Duration::from_millis(100));
        assert!(!animation.advance(tapped + Duration::from_millis(339)));
        assert!(animation.advance(tapped + Duration::from_millis(340)));
    }
}

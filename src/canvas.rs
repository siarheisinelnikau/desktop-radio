#![allow(unsafe_op_in_unsafe_fn)]

use anyhow::{Result, bail, ensure};
use std::{
    collections::HashMap,
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io::BufWriter,
    path::Path,
    ptr::null_mut,
    result::Result::Ok,
};
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::{Gdi::*, GdiPlus::*},
};

pub type Color = u32;

#[derive(Clone, Copy, Debug)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Area {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn inset(self, x: f32, y: f32) -> Self {
        Self::new(self.x + x, self.y + y, self.w - 2. * x, self.h - 2. * y)
    }
    pub fn right(self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }
    pub fn center(self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    fn native(self) -> RectF {
        RectF {
            X: self.x,
            Y: self.y,
            Width: self.w,
            Height: self.h,
        }
    }
}

pub const fn rgb(rgb: u32) -> Color {
    rgb | 0xff000000
}

pub fn alpha(color: Color, opacity: f32) -> Color {
    let opacity = ((color >> 24) as f32 * opacity.clamp(0., 1.)).round() as u32;
    (color & 0xffffff) | (opacity << 24)
}

pub const fn blend(color: Color, over: Color, alpha: u32) -> Color {
    let red = ((((color >> 16) & 255) * alpha + ((over >> 16) & 255) * (255 - alpha)) / 255) << 16;
    let green = ((((color >> 8) & 255) * alpha + ((over >> 8) & 255) * (255 - alpha)) / 255) << 8;
    let blue = ((color & 255) * alpha + (over & 255) * (255 - alpha)) / 255;
    rgb(red | green | blue)
}

pub struct GdiPlus(usize);

impl GdiPlus {
    pub fn start() -> Result<Self> {
        let mut token = 0;
        let options = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        ensure!(
            unsafe { GdiplusStartup(&mut token, &options, null_mut()) } == 0,
            "Start GDI+"
        );
        Ok(Self(token))
    }
}

impl Drop for GdiPlus {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.0) };
    }
}

pub struct Surface {
    pub dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    pixels: *mut u8,
    pub width: i32,
    pub height: i32,
}

impl Surface {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        ensure!(
            (1..=4096).contains(&width) && (1..=4096).contains(&height),
            "Invalid surface resolution"
        );
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            ensure!(!dc.is_null(), "Create memory DC");
            let header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = null_mut();
            let bitmap = CreateDIBSection(dc, &header, DIB_RGB_COLORS, &mut pixels, null_mut(), 0);
            if bitmap.is_null() || pixels.is_null() {
                DeleteDC(dc);
                bail!("Create DIB surface");
            }
            let old = SelectObject(dc, bitmap);
            Ok(Self {
                dc,
                bitmap,
                old,
                pixels: pixels.cast(),
                width,
                height,
            })
        }
    }

    pub fn blit(&self, dc: HDC) -> bool {
        unsafe { BitBlt(dc, 0, 0, self.width, self.height, self.dc, 0, 0, SRCCOPY) != 0 }
    }

    pub fn set_layer_alpha(&mut self, width: i32, radius: i32, refresh_corners: bool) {
        let radius = radius.min(width / 2).min(self.height / 2) as usize;
        let width = width as usize;
        let height = self.height as usize;
        unsafe {
            GdiFlush();
            let pixels = std::slice::from_raw_parts_mut(
                self.pixels.cast::<u32>(),
                self.width as usize * height,
            );
            for (y, scanline) in pixels.chunks_exact_mut(self.width as usize).enumerate() {
                let dy = y.min(height - 1 - y);
                let inset = if dy < radius { radius } else { 0 };
                for pixel in &mut scanline[inset..width - inset] {
                    *pixel |= 0xff00_0000;
                }
                if refresh_corners && dy < radius {
                    let dy = radius as f32 - dy as f32 - 0.5;
                    for x in 0..radius {
                        let dx = radius as f32 - x as f32 - 0.5;
                        let coverage = (radius as f32 + 0.5 - dx.hypot(dy)).clamp(0., 1.);
                        let alpha = (coverage * 255.).round() as u32;
                        for index in [x, width - 1 - x] {
                            let rgb = scanline[index];
                            let premultiply = |shift: u32| {
                                ((((rgb >> shift) & 255) * alpha + 127) / 255) << shift
                            };
                            scanline[index] =
                                (alpha << 24) | premultiply(16) | premultiply(8) | premultiply(0);
                        }
                    }
                }
            }
        }
    }

    pub fn rgb_checksum(&self, area: RECT) -> Option<String> {
        if area.left < 0 || area.top < 0 || area.right > self.width || area.bottom > self.height {
            return None;
        }
        unsafe {
            GdiFlush();
            let pixels = std::slice::from_raw_parts(
                self.pixels.cast::<u32>(),
                (self.width * self.height) as usize,
            );
            let mut checksum = DefaultHasher::new();
            for row in area.top as usize..area.bottom as usize {
                let offset = row * self.width as usize;
                for &pixel in &pixels[offset + area.left as usize..offset + area.right as usize] {
                    (pixel & 0x00ff_ffff).hash(&mut checksum);
                }
            }
            Some(format!("{:016x}", checksum.finish()))
        }
    }

    pub fn png(&self, path: &Path) -> Result<()> {
        unsafe {
            GdiFlush();
        }
        let source = unsafe {
            std::slice::from_raw_parts(self.pixels, (self.width * self.height * 4) as usize)
        };
        let mut pixels = Vec::with_capacity(source.len() / 4 * 3);
        for bgra in source.chunks_exact(4) {
            pixels.extend([bgra[2], bgra[1], bgra[0]]);
        }
        let mut encoder = png::Encoder::new(
            BufWriter::new(File::create(path)?),
            self.width as u32,
            self.height as u32,
        );
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            GdiFlush();
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

pub struct Canvas {
    graphics: *mut GpGraphics,
    family: *mut GpFontFamily,
    fonts: HashMap<(u8, bool), *mut GpFont>,
    format: *mut GpStringFormat,
    brush: *mut GpSolidFill,
    pen: *mut GpPen,
    path: *mut GpPath,
}

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Gradient(*mut GpBrush);

impl Gradient {
    pub fn linear<const N: usize>(
        from: (f32, f32),
        to: (f32, f32),
        stops: [(f32, Color); N],
    ) -> Option<Self> {
        let mut brush = null_mut();
        unsafe {
            if GdipCreateLineBrush(
                &point(from),
                &point(to),
                stops[0].1,
                stops[N - 1].1,
                WrapModeTileFlipXY,
                &mut brush,
            ) != 0
            {
                return None;
            }
            let gradient = Self(brush.cast());
            if N > 2 {
                let colors = stops.map(|(_, color)| color);
                let positions = stops.map(|(position, _)| position);
                if GdipSetLinePresetBlend(brush, colors.as_ptr(), positions.as_ptr(), N as i32) != 0
                {
                    return None;
                }
            }
            Some(gradient)
        }
    }
}

impl Drop for Gradient {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteBrush(self.0);
        }
    }
}

fn point((x, y): (f32, f32)) -> PointF {
    PointF { X: x, Y: y }
}

impl Area {
    pub fn around((x, y): (f32, f32), radius: f32) -> Self {
        Self::new(x - radius, y - radius, radius * 2., radius * 2.)
    }
}

impl Canvas {
    pub fn open(dc: HDC, scale: f32) -> Result<Self> {
        let mut canvas = Self {
            graphics: null_mut(),
            family: null_mut(),
            fonts: HashMap::new(),
            format: null_mut(),
            brush: null_mut(),
            pen: null_mut(),
            path: null_mut(),
        };
        unsafe {
            ensure!(
                GdipCreateFromHDC(dc, &mut canvas.graphics) == 0,
                "Create GDI+ graphics"
            );
            let family = crate::http::wide("Segoe UI");
            ensure!(
                GdipCreateFontFamilyFromName(family.as_ptr(), null_mut(), &mut canvas.family) == 0,
                "Load Segoe UI"
            );
            GdipCreateSolidFill(rgb(0xffffff), &mut canvas.brush);
            GdipCreatePen1(rgb(0xffffff), 1., UnitPixel, &mut canvas.pen);
            GdipCreatePath(FillModeWinding, &mut canvas.path);
            GdipCreateStringFormat(
                StringFormatFlagsNoWrap | StringFormatFlagsLineLimit,
                0,
                &mut canvas.format,
            );
            GdipSetStringFormatTrimming(canvas.format, StringTrimmingEllipsisCharacter);
            GdipSetStringFormatLineAlign(canvas.format, StringAlignmentCenter);
            GdipSetTextRenderingHint(canvas.graphics, TextRenderingHintAntiAliasGridFit);
            GdipSetSmoothingMode(canvas.graphics, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(canvas.graphics, PixelOffsetModeHalf);
            GdipScaleWorldTransform(canvas.graphics, scale, scale, MatrixOrderAppend);
        }
        Ok(canvas)
    }

    pub fn clear(&self, color: Color) {
        unsafe {
            GdipGraphicsClear(self.graphics, color);
        }
    }

    pub fn clipped(&mut self, area: Area, draw: impl FnOnce(&mut Self)) {
        let mut saved = 0;
        unsafe {
            if GdipSaveGraphics(self.graphics, &mut saved) != 0 {
                return;
            }
            GdipSetClipRect(
                self.graphics,
                area.x,
                area.y,
                area.w,
                area.h,
                CombineModeIntersect,
            );
        }
        draw(self);
        unsafe {
            GdipRestoreGraphics(self.graphics, saved);
        }
    }

    pub fn transformed(
        &mut self,
        (x, y): (f32, f32),
        (sx, sy): (f32, f32),
        (px, py): (f32, f32),
        draw: impl FnOnce(&mut Self),
    ) {
        let mut saved = 0;
        unsafe {
            if GdipSaveGraphics(self.graphics, &mut saved) != 0 {
                return;
            }
            GdipTranslateWorldTransform(self.graphics, x, y, MatrixOrderPrepend);
            GdipScaleWorldTransform(self.graphics, sx, sy, MatrixOrderPrepend);
            GdipTranslateWorldTransform(self.graphics, -px, -py, MatrixOrderPrepend);
        }
        draw(self);
        unsafe {
            GdipRestoreGraphics(self.graphics, saved);
        }
    }

    pub fn radial_gradient<const N: usize>(
        &self,
        area: Area,
        center: (f32, f32),
        stops: [(f32, Color); N],
    ) -> Option<Gradient> {
        let mut brush = null_mut();
        unsafe {
            GdipResetPath(self.path);
            GdipAddPathEllipse(self.path, area.x, area.y, area.w, area.h);
            if GdipCreatePathGradientFromPath(self.path, &mut brush) != 0 {
                return None;
            }
            let gradient = Gradient(brush.cast());
            // GDI+ orders its radial stops from the boundary towards the center.
            let colors: [Color; N] = std::array::from_fn(|i| stops[N - 1 - i].1);
            let positions: [f32; N] = std::array::from_fn(|i| 1. - stops[N - 1 - i].0);
            if GdipSetPathGradientCenterPoint(brush, &point(center)) != 0
                || GdipSetPathGradientPresetBlend(
                    brush,
                    colors.as_ptr(),
                    positions.as_ptr(),
                    N as i32,
                ) != 0
            {
                return None;
            }
            Some(gradient)
        }
    }

    pub fn gradient_ellipse(&self, area: Area, gradient: &Gradient) {
        unsafe {
            GdipFillEllipse(self.graphics, gradient.0, area.x, area.y, area.w, area.h);
        }
    }

    pub fn gradient_polygon<const N: usize>(&self, points: [(f32, f32); N], gradient: &Gradient) {
        let points = points.map(point);
        unsafe {
            GdipFillPolygon(
                self.graphics,
                gradient.0,
                points.as_ptr(),
                N as i32,
                FillModeWinding,
            );
        }
    }

    pub fn gradient_panel(&self, area: Area, radius: f32, gradient: &Gradient) {
        self.path(area, radius);
        unsafe {
            GdipFillPath(self.graphics, gradient.0, self.path);
        }
    }

    pub fn outside_ellipse(&self, area: Area, opening: Area, color: Color) {
        unsafe {
            GdipResetPath(self.path);
            GdipSetPathFillMode(self.path, FillModeAlternate);
            GdipAddPathRectangle(self.path, area.x, area.y, area.w, area.h);
            GdipAddPathEllipse(self.path, opening.x, opening.y, opening.w, opening.h);
            GdipSetClipRect(
                self.graphics,
                area.x,
                area.y,
                area.w,
                area.h,
                CombineModeIntersect,
            );
            GdipSetSolidFillColor(self.brush, color);
            GdipFillPath(self.graphics, self.brush.cast(), self.path);
            GdipResetClip(self.graphics);
            GdipSetPathFillMode(self.path, FillModeWinding);
        }
    }

    pub fn arcs(&self, area: Area, starts: &[f32], sweep: f32, width: f32, color: Color) {
        self.pen(color, width);
        self.stroke_arcs(self.pen, area, starts, sweep);
    }

    pub fn gradient_arcs(
        &self,
        area: Area,
        starts: &[f32],
        sweep: f32,
        width: f32,
        gradient: &Gradient,
    ) {
        self.with_gradient_pen(gradient, width, |pen| {
            self.stroke_arcs(pen, area, starts, sweep)
        });
    }

    pub fn gradient_line(
        &self,
        (x1, y1): (f32, f32),
        (x2, y2): (f32, f32),
        width: f32,
        gradient: &Gradient,
    ) {
        self.with_gradient_pen(gradient, width, |pen| unsafe {
            GdipDrawLine(self.graphics, pen, x1, y1, x2, y2);
        });
    }

    fn with_gradient_pen(&self, gradient: &Gradient, width: f32, draw: impl FnOnce(*mut GpPen)) {
        let mut pen = null_mut();
        unsafe {
            if GdipCreatePen2(gradient.0, width, UnitWorld, &mut pen) != 0 {
                return;
            }
            GdipSetPenStartCap(pen, LineCapRound);
            GdipSetPenEndCap(pen, LineCapRound);
        }
        draw(pen);
        unsafe {
            GdipDeletePen(pen);
        }
    }

    fn stroke_arcs(&self, pen: *mut GpPen, area: Area, starts: &[f32], sweep: f32) {
        unsafe {
            for &start in starts {
                GdipDrawArc(
                    self.graphics,
                    pen,
                    area.x,
                    area.y,
                    area.w,
                    area.h,
                    start,
                    sweep,
                );
            }
        }
    }

    fn path(&self, area: Area, radius: f32) {
        let d = (radius * 2.).min(area.h).min(area.w);
        unsafe {
            GdipResetPath(self.path);
            if d <= 0. {
                GdipAddPathRectangle(self.path, area.x, area.y, area.w, area.h);
                return;
            }
            for (x, y, angle) in [
                (area.x, area.y, 180.),
                (area.right() - d, area.y, 270.),
                (area.right() - d, area.bottom() - d, 0.),
                (area.x, area.bottom() - d, 90.),
            ] {
                GdipAddPathArc(self.path, x, y, d, d, angle, 90.);
            }
            GdipClosePathFigure(self.path);
        }
    }

    pub fn panel(&self, area: Area, radius: f32, color: Color, line: Option<Color>) {
        self.path(area, radius);
        unsafe {
            GdipSetSolidFillColor(self.brush, color);
            GdipFillPath(self.graphics, self.brush.cast(), self.path);
            if let Some(line) = line {
                self.pen(line, 1.);
                GdipDrawPath(self.graphics, self.pen, self.path);
            }
        }
    }

    pub fn gradient(&self, area: Area, radius: f32, start: Color, end: Color, edge: Color) {
        self.path(area, radius);
        unsafe {
            let mut gradient = null_mut();
            GdipCreateLineBrushFromRect(
                &area.native(),
                start,
                end,
                LinearGradientModeVertical,
                WrapModeTileFlipXY,
                &mut gradient,
            );
            if !gradient.is_null() {
                GdipFillPath(self.graphics, gradient.cast(), self.path);
                GdipDeleteBrush(gradient.cast());
            }
            self.pen(edge, 1.);
            GdipDrawPath(self.graphics, self.pen, self.path);
        }
    }

    pub fn circle(&self, x: f32, y: f32, radius: f32, color: Color) {
        unsafe {
            GdipSetSolidFillColor(self.brush, color);
            GdipFillEllipse(
                self.graphics,
                self.brush.cast(),
                x - radius,
                y - radius,
                radius * 2.,
                radius * 2.,
            );
        }
    }

    fn pen(&self, color: Color, width: f32) {
        unsafe {
            GdipSetPenColor(self.pen, color);
            GdipSetPenWidth(self.pen, width);
            GdipSetPenStartCap(self.pen, LineCapRound);
            GdipSetPenEndCap(self.pen, LineCapRound);
        }
    }

    pub fn line(&self, a: (f32, f32), b: (f32, f32), color: Color, width: f32) {
        self.pen(color, width);
        unsafe {
            GdipDrawLine(self.graphics, self.pen, a.0, a.1, b.0, b.1);
        }
    }

    pub fn polyline(&self, points: &[(f32, f32)], color: Color, width: f32) {
        self.pen(color, width);
        let points = points
            .iter()
            .map(|&(x, y)| PointF { X: x, Y: y })
            .collect::<Vec<_>>();
        unsafe {
            GdipDrawLines(
                self.graphics,
                self.pen,
                points.as_ptr(),
                points.len() as i32,
            );
        }
    }

    pub fn polygon(&self, points: &[(f32, f32)], color: Color, filled: bool) {
        let points = points
            .iter()
            .map(|&(x, y)| PointF { X: x, Y: y })
            .collect::<Vec<_>>();
        unsafe {
            if filled {
                GdipSetSolidFillColor(self.brush, color);
                GdipFillPolygon(
                    self.graphics,
                    self.brush.cast(),
                    points.as_ptr(),
                    points.len() as i32,
                    FillModeWinding,
                );
            } else {
                self.pen(color, 1.35);
                GdipDrawPolygon(
                    self.graphics,
                    self.pen,
                    points.as_ptr(),
                    points.len() as i32,
                );
            }
        }
    }

    pub fn text(
        &mut self,
        area: Area,
        text: &str,
        size: u8,
        bold: bool,
        color: Color,
        align: Align,
    ) {
        if text.is_empty() || area.w <= 0. || area.h <= 0. {
            return;
        }
        let font = *self.fonts.entry((size, bold)).or_insert_with(|| {
            let mut font = null_mut();
            unsafe {
                GdipCreateFont(
                    self.family,
                    size as f32,
                    if bold {
                        FontStyleBold
                    } else {
                        FontStyleRegular
                    },
                    UnitPixel,
                    &mut font,
                );
            }
            font
        });
        let text = text.encode_utf16().collect::<Vec<_>>();
        unsafe {
            GdipSetSolidFillColor(self.brush, color);
            GdipSetStringFormatAlign(
                self.format,
                match align {
                    Align::Left => StringAlignmentNear,
                    Align::Center => StringAlignmentCenter,
                    Align::Right => StringAlignmentFar,
                },
            );
            GdipDrawString(
                self.graphics,
                text.as_ptr(),
                text.len() as i32,
                font,
                &area.native(),
                self.format,
                self.brush.cast(),
            );
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            for (_, font) in self.fonts.drain() {
                if !font.is_null() {
                    GdipDeleteFont(font);
                }
            }
            if !self.family.is_null() {
                GdipDeleteFontFamily(self.family);
            }
            if !self.format.is_null() {
                GdipDeleteStringFormat(self.format);
            }
            if !self.path.is_null() {
                GdipDeletePath(self.path);
            }
            if !self.pen.is_null() {
                GdipDeletePen(self.pen);
            }
            if !self.brush.is_null() {
                GdipDeleteBrush(self.brush.cast());
            }
            if !self.graphics.is_null() {
                GdipDeleteGraphics(self.graphics);
            }
        }
    }
}

#![allow(unsafe_op_in_unsafe_fn)]

use anyhow::{Result, bail, ensure};
use std::{
    collections::HashMap, fs::File, io::BufWriter, path::Path, ptr::null_mut, result::Result::Ok,
};
use windows_sys::Win32::Graphics::{Gdi::*, GdiPlus::*};

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

    pub fn blit(&self, dc: HDC) {
        unsafe {
            BitBlt(dc, 0, 0, self.width, self.height, self.dc, 0, 0, SRCCOPY);
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

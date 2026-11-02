//! 共享的 Direct2D / DirectWrite 渲染设施。
//!
//! 只在 UI 线程使用：工厂按线程缓存，渲染目标绑定到窗口。
//! 坐标单位一律是物理像素（渲染目标的 DPI 固定为 96），字号由调用方按 DPI 换算成像素。
//!
//! - [`Surface`]：窗口渲染目标 + 一个纯色画刷；渲染目标丢失（显卡重置等）时由调用方丢弃重建
//! - [`Font`]：文本格式与行高、字符宽度等度量（统一行距，保证中英文混排时各行基线一致）
//! - [`TextLayout`]：单行文本排版，提供命中测试（坐标 ↔ 字符位置）
//! - [`TextFormat`] + [`RichText`]：自动折行的多行富文本（粗体、斜体、字体、颜色、删除线、下划线），
//!   提供命中测试与范围矩形（Markdown 预览用）
//! - [`Bitmap`]：从 PBGRA 像素创建的位图（属于创建它的 [`Surface`]）；[`decode_image`] 在后台线程用 WIC 解码图片

#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::core::{Error, Result, PCWSTR};
use windows::Win32::Foundation::{BOOL, D2DERR_RECREATE_TARGET, GENERIC_ACCESS_RIGHTS, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS, DWRITE_LINE_METRICS,
    DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_EMERGENCY_BREAK,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

thread_local! {
    static FACTORIES: RefCell<Option<(ID2D1Factory, IDWriteFactory)>> = const { RefCell::new(None) };
}

fn factories() -> Result<(ID2D1Factory, IDWriteFactory)> {
    FACTORIES.with(|f| {
        if let Some(pair) = f.borrow().as_ref() {
            return Ok(pair.clone());
        }
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        *f.borrow_mut() = Some((d2d.clone(), dw.clone()));
        Ok((d2d, dw))
    })
}

/// 0xRRGGBB → D2D 颜色。
pub fn rgb(c: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c >> 16) & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: (c & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// 绘制失败是否因为渲染目标丢失（需要丢弃 [`Surface`] 重建）。
pub fn needs_recreate(e: &Error) -> bool {
    e.code() == D2DERR_RECREATE_TARGET
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

/// 每个渲染目标一个编号：设备相关资源（画刷、位图）只能用在创建它的渲染目标上。
static NEXT_SURFACE: AtomicU64 = AtomicU64::new(1);

/// 窗口的渲染目标。
pub struct Surface {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    id: u64,
    /// 富文本颜色段用的画刷（按颜色缓存）
    brushes: RefCell<HashMap<u32, ID2D1SolidColorBrush>>,
}

impl Surface {
    /// 为窗口（原始 HWND）创建渲染目标，`width`/`height` 为客户区像素尺寸。
    pub fn new(hwnd: *mut core::ffi::c_void, width: u32, height: u32) -> Result<Surface> {
        let (d2d, _) = factories()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: HWND(hwnd),
            pixelSize: D2D_SIZE_U {
                width: width.max(1),
                height: height.max(1),
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = unsafe { d2d.CreateHwndRenderTarget(&props, &hprops)? };
        unsafe { rt.SetDpi(96.0, 96.0) };
        let brush = unsafe { rt.CreateSolidColorBrush(&rgb(0), None)? };
        Ok(Surface {
            rt,
            brush,
            id: NEXT_SURFACE.fetch_add(1, Ordering::Relaxed),
            brushes: RefCell::new(HashMap::new()),
        })
    }

    /// 渲染目标编号（重建后变化）。
    pub fn id(&self) -> u64 {
        self.id
    }

    fn brush_for(&self, c: u32) -> Option<ID2D1SolidColorBrush> {
        if let Some(b) = self.brushes.borrow().get(&c) {
            return Some(b.clone());
        }
        let b = unsafe { self.rt.CreateSolidColorBrush(&rgb(c), None).ok()? };
        self.brushes.borrow_mut().insert(c, b.clone());
        Some(b)
    }

    /// 矩形描边。
    #[allow(clippy::too_many_arguments)]
    pub fn outline(&self, l: f32, t: f32, r: f32, b: f32, width: f32, c: u32) {
        if r <= l || b <= t {
            return;
        }
        self.set(c);
        unsafe {
            self.rt
                .DrawRectangle(&rect(l, t, r, b), &self.brush, width, None)
        };
    }

    /// 圆角矩形填充。
    #[allow(clippy::too_many_arguments)]
    pub fn fill_round(&self, l: f32, t: f32, r: f32, b: f32, radius: f32, c: u32) {
        if r <= l || b <= t {
            return;
        }
        self.set(c);
        let rr = D2D1_ROUNDED_RECT {
            rect: rect(l, t, r, b),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.rt.FillRoundedRectangle(&rr, &self.brush) };
    }

    /// 绘制多行富文本，`(x, y)` 为左上角，`c` 为没有颜色段的文字的颜色。
    pub fn rich(&self, t: &RichText, x: f32, y: f32, c: u32) {
        if t.applied.get() != self.id {
            // 颜色段的画刷属于渲染目标，换了渲染目标要重新设置
            for &(start, len, color) in &t.colors {
                if let Some(b) = self.brush_for(color) {
                    let range = DWRITE_TEXT_RANGE {
                        startPosition: start,
                        length: len,
                    };
                    unsafe {
                        let _ = t.inner.SetDrawingEffect(&b, range);
                    }
                }
            }
            t.applied.set(self.id);
        }
        self.set(c);
        unsafe {
            self.rt.DrawTextLayout(
                D2D_POINT_2F { x, y },
                &t.inner,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            )
        };
    }

    /// 从预乘 alpha 的 BGRA 像素创建位图。
    pub fn create_bitmap(&self, width: u32, height: u32, pixels: &[u8]) -> Result<Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let size = D2D_SIZE_U {
            width: width.max(1),
            height: height.max(1),
        };
        if pixels.len() < width as usize * height as usize * 4 {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        let bmp = unsafe {
            self.rt.CreateBitmap(
                size,
                Some(pixels.as_ptr() as *const core::ffi::c_void),
                width * 4,
                &props,
            )?
        };
        Ok(Bitmap {
            inner: bmp,
            surface: self.id,
        })
    }

    /// 把位图画到矩形中（缩放）。
    pub fn bitmap(&self, b: &Bitmap, l: f32, t: f32, r: f32, bt: f32) {
        if b.surface != self.id || r <= l || bt <= t {
            return;
        }
        let dst = rect(l, t, r, bt);
        unsafe {
            self.rt.DrawBitmap(
                &b.inner,
                Some(&dst as *const D2D_RECT_F),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            )
        };
    }

    pub fn resize(&self, width: u32, height: u32) {
        let size = D2D_SIZE_U {
            width: width.max(1),
            height: height.max(1),
        };
        unsafe {
            let _ = self.rt.Resize(&size);
        }
    }

    pub fn begin(&self) {
        unsafe { self.rt.BeginDraw() };
    }

    /// 结束绘制。返回错误且 [`needs_recreate`] 为真时，调用方应丢弃本对象。
    pub fn end(&self) -> Result<()> {
        unsafe { self.rt.EndDraw(None, None) }
    }

    fn set(&self, c: u32) {
        unsafe { self.brush.SetColor(&rgb(c)) };
    }

    pub fn clear(&self, c: u32) {
        unsafe { self.rt.Clear(Some(&rgb(c))) };
    }

    pub fn fill(&self, l: f32, t: f32, r: f32, b: f32, c: u32) {
        if r <= l || b <= t {
            return;
        }
        self.set(c);
        unsafe { self.rt.FillRectangle(&rect(l, t, r, b), &self.brush) };
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: u32) {
        self.set(c);
        unsafe {
            self.rt.DrawLine(
                D2D_POINT_2F { x: x0, y: y0 },
                D2D_POINT_2F { x: x1, y: y1 },
                &self.brush,
                width,
                None,
            )
        };
    }

    /// 在矩形内绘制一段文字（超出部分裁掉）。
    #[allow(clippy::too_many_arguments)]
    pub fn text(&self, s: &[u16], font: &Font, l: f32, t: f32, r: f32, b: f32, c: u32) {
        if s.is_empty() {
            return;
        }
        self.set(c);
        unsafe {
            self.rt.DrawText(
                s,
                &font.fmt,
                &rect(l, t, r, b),
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    /// 绘制排好版的一行文字，`(x, y)` 为左上角。
    pub fn layout(&self, l: &TextLayout, x: f32, y: f32, c: u32) {
        self.set(c);
        unsafe {
            self.rt.DrawTextLayout(
                D2D_POINT_2F { x, y },
                &l.inner,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            )
        };
    }

    /// 之后的绘制限制在矩形内，直到 [`unclip`](Self::unclip)。
    pub fn clip(&self, l: f32, t: f32, r: f32, b: f32) {
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rect(l, t, r, b), D2D1_ANTIALIAS_MODE_ALIASED)
        };
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }
}

/// 字体（文本格式）与度量。
pub struct Font {
    fmt: IDWriteTextFormat,
    /// 统一行高（像素）
    pub line_h: f32,
    /// 半角字符宽度（"0" 的宽度）
    pub char_w: f32,
    /// 字号（像素）
    pub size: f32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Font {
    /// `size_px` 为像素字号；`tab_chars` 为制表符宽度（字符数）。
    pub fn new(face: &str, size_px: f32, tab_chars: u32) -> Result<Font> {
        Font::styled(face, size_px, tab_chars, false, false)
    }

    /// 指定粗体/斜体的字体。度量按各自字形测量，调用方需要等宽网格时应以常规字体的度量为准。
    pub fn styled(
        face: &str,
        size_px: f32,
        tab_chars: u32,
        bold: bool,
        italic: bool,
    ) -> Result<Font> {
        let (_, dw) = factories()?;
        let face_w = wide(face);
        let locale = wide("zh-cn");
        let fmt = unsafe {
            dw.CreateTextFormat(
                PCWSTR(face_w.as_ptr()),
                None,
                if bold {
                    DWRITE_FONT_WEIGHT_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                },
                if italic {
                    DWRITE_FONT_STYLE_ITALIC
                } else {
                    DWRITE_FONT_STYLE_NORMAL
                },
                DWRITE_FONT_STRETCH_NORMAL,
                size_px.max(1.0),
                PCWSTR(locale.as_ptr()),
            )?
        };
        unsafe { fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };

        // 用含中文的样本测行高与基线（中文回退字体通常比西文字体高）
        let sample: Vec<u16> = "Ag汉字0".encode_utf16().collect();
        let probe = unsafe { dw.CreateTextLayout(&sample, &fmt, 10_000.0, 10_000.0)? };
        let mut lm = [DWRITE_LINE_METRICS::default(); 1];
        let mut count = 0u32;
        unsafe { probe.GetLineMetrics(Some(&mut lm), &mut count)? };
        let line_h = lm[0].height.ceil().max(1.0);
        let baseline = lm[0].baseline;
        unsafe { fmt.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_h, baseline)? };

        let zeros: Vec<u16> = "0000000000".encode_utf16().collect();
        let probe = unsafe { dw.CreateTextLayout(&zeros, &fmt, 10_000.0, 10_000.0)? };
        let mut tm = DWRITE_TEXT_METRICS::default();
        unsafe { probe.GetMetrics(&mut tm)? };
        let char_w = (tm.widthIncludingTrailingWhitespace / 10.0).max(1.0);
        unsafe { fmt.SetIncrementalTabStop(char_w * tab_chars.max(1) as f32)? };
        Ok(Font {
            fmt,
            line_h,
            char_w,
            size: size_px,
        })
    }
}

/// 单行文本的排版结果。
pub struct TextLayout {
    inner: IDWriteTextLayout,
    /// 文字宽度（含行尾空白）
    pub width: f32,
    len: usize,
}

impl TextLayout {
    pub fn new(text: &[u16], font: &Font) -> Result<TextLayout> {
        let (_, dw) = factories()?;
        let inner = unsafe { dw.CreateTextLayout(text, &font.fmt, 1.0e7, font.line_h)? };
        let mut tm = DWRITE_TEXT_METRICS::default();
        unsafe { inner.GetMetrics(&mut tm)? };
        Ok(TextLayout {
            inner,
            width: tm.widthIncludingTrailingWhitespace,
            len: text.len(),
        })
    }

    /// 横坐标 `x`（相对文字左端）处的插入位置（UTF-16 码元下标，落在字符右半边时取其后）。
    pub fn hit(&self, x: f32) -> usize {
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        let ok = unsafe {
            self.inner
                .HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m)
        };
        if ok.is_err() {
            return 0;
        }
        let pos = m.textPosition as usize
            + if trailing.0 != 0 {
                m.length as usize
            } else {
                0
            };
        pos.min(self.len)
    }

    /// 插入位置 `pos`（UTF-16 码元下标）的横坐标（相对文字左端）。
    pub fn x_at(&self, pos: usize) -> f32 {
        if pos >= self.len {
            return self.width;
        }
        let (mut x, mut y) = (0f32, 0f32);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        let ok = unsafe {
            self.inner
                .HitTestTextPosition(pos as u32, BOOL(0), &mut x, &mut y, &mut m)
        };
        if ok.is_err() {
            return 0.0;
        }
        x
    }
}

// ---------------------------------------------------------------------------
// 多行富文本

/// 折行文本的格式（字体、字号、行距）。
pub struct TextFormat {
    fmt: IDWriteTextFormat,
    /// 统一行高（像素）
    pub line_h: f32,
    /// 字号（像素）
    pub size: f32,
}

/// 文字对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl TextFormat {
    /// `spacing` 为行距倍数（1.0 = 字体自然行高）；`wrap` 为是否自动折行（超长单词在必要时断开）。
    pub fn new(
        face: &str,
        size_px: f32,
        bold: bool,
        italic: bool,
        spacing: f32,
        wrap: bool,
    ) -> Result<TextFormat> {
        let (_, dw) = factories()?;
        let face_w = wide(face);
        let locale = wide("zh-cn");
        let fmt = unsafe {
            dw.CreateTextFormat(
                PCWSTR(face_w.as_ptr()),
                None,
                if bold {
                    DWRITE_FONT_WEIGHT_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                },
                if italic {
                    DWRITE_FONT_STYLE_ITALIC
                } else {
                    DWRITE_FONT_STYLE_NORMAL
                },
                DWRITE_FONT_STRETCH_NORMAL,
                size_px.max(1.0),
                PCWSTR(locale.as_ptr()),
            )?
        };
        unsafe {
            fmt.SetWordWrapping(if wrap {
                DWRITE_WORD_WRAPPING_EMERGENCY_BREAK
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            })?
        };
        // 用含中文的样本测行高与基线，再按倍数设统一行距（中英文混排时各行高度一致）
        let sample: Vec<u16> = "Ag汉字0".encode_utf16().collect();
        let probe = unsafe { dw.CreateTextLayout(&sample, &fmt, 10_000.0, 10_000.0)? };
        let mut lm = [DWRITE_LINE_METRICS::default(); 1];
        let mut count = 0u32;
        unsafe { probe.GetLineMetrics(Some(&mut lm), &mut count)? };
        let h = lm[0].height.max(1.0);
        let spacing = spacing.max(1.0);
        let line_h = (h * spacing).ceil();
        let baseline = lm[0].baseline + (line_h - h) / 2.0;
        unsafe { fmt.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_h, baseline)? };
        let zeros: Vec<u16> = "0000".encode_utf16().collect();
        let probe = unsafe { dw.CreateTextLayout(&zeros, &fmt, 10_000.0, 10_000.0)? };
        let mut tm = DWRITE_TEXT_METRICS::default();
        unsafe { probe.GetMetrics(&mut tm)? };
        let char_w = (tm.widthIncludingTrailingWhitespace / 4.0).max(1.0);
        unsafe { fmt.SetIncrementalTabStop(char_w * 4.0)? };
        Ok(TextFormat {
            fmt,
            line_h,
            size: size_px,
        })
    }
}

/// 自动折行的多行富文本。
pub struct RichText {
    inner: IDWriteTextLayout,
    /// (起点, 长度, 颜色)：绘制时设为画刷效果
    colors: Vec<(u32, u32, u32)>,
    /// 已为哪个渲染目标设置过颜色效果
    applied: Cell<u64>,
    len: usize,
    /// 实际宽度（最宽一行，含行尾空白）
    pub width: f32,
    pub height: f32,
    /// 行数
    pub lines: u32,
}

fn range(start: usize, len: usize) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: start as u32,
        length: len as u32,
    }
}

impl RichText {
    /// 按最大宽度排版 `text`（UTF-16）。设置完样式后调用 [`finish`](Self::finish) 更新度量。
    pub fn new(text: &[u16], fmt: &TextFormat, max_width: f32) -> Result<RichText> {
        let (_, dw) = factories()?;
        let inner = unsafe { dw.CreateTextLayout(text, &fmt.fmt, max_width.max(1.0), 1.0e7)? };
        let mut t = RichText {
            inner,
            colors: Vec::new(),
            applied: Cell::new(0),
            len: text.len(),
            width: 0.0,
            height: 0.0,
            lines: 0,
        };
        t.finish();
        Ok(t)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bold(&self, start: usize, len: usize) {
        unsafe {
            let _ = self
                .inner
                .SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range(start, len));
        }
    }

    pub fn weight(&self, w: i32, start: usize, len: usize) {
        unsafe {
            let _ = self
                .inner
                .SetFontWeight(DWRITE_FONT_WEIGHT(w), range(start, len));
        }
    }

    pub fn italic(&self, start: usize, len: usize) {
        unsafe {
            let _ = self
                .inner
                .SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range(start, len));
        }
    }

    pub fn strike(&self, start: usize, len: usize) {
        unsafe {
            let _ = self.inner.SetStrikethrough(BOOL(1), range(start, len));
        }
    }

    pub fn underline(&self, start: usize, len: usize) {
        unsafe {
            let _ = self.inner.SetUnderline(BOOL(1), range(start, len));
        }
    }

    pub fn family(&self, face: &str, start: usize, len: usize) {
        let w = wide(face);
        unsafe {
            let _ = self
                .inner
                .SetFontFamilyName(PCWSTR(w.as_ptr()), range(start, len));
        }
    }

    pub fn font_size(&self, px: f32, start: usize, len: usize) {
        unsafe {
            let _ = self.inner.SetFontSize(px.max(1.0), range(start, len));
        }
    }

    pub fn color(&mut self, c: u32, start: usize, len: usize) {
        if len > 0 {
            self.colors.push((start as u32, len as u32, c));
            self.applied.set(0);
        }
    }

    pub fn align(&self, a: Align) {
        let v = match a {
            Align::Left => 0,
            Align::Right => 1,
            Align::Center => 2,
        };
        unsafe {
            let _ = self.inner.SetTextAlignment(DWRITE_TEXT_ALIGNMENT(v));
        }
    }

    /// 改变最大宽度（重新折行）。
    pub fn set_max_width(&mut self, w: f32) {
        unsafe {
            let _ = self.inner.SetMaxWidth(w.max(1.0));
        }
        self.finish();
    }

    /// 样式或宽度变化后重新取得度量。
    pub fn finish(&mut self) {
        let mut tm = DWRITE_TEXT_METRICS::default();
        if unsafe { self.inner.GetMetrics(&mut tm) }.is_ok() {
            self.width = tm.widthIncludingTrailingWhitespace;
            self.height = tm.height;
            self.lines = tm.lineCount;
        }
    }

    /// 不折行时最长的不可断开片段的宽度（表格列宽用）。
    pub fn min_width(&self) -> f32 {
        unsafe { self.inner.DetermineMinWidth() }.unwrap_or(0.0)
    }

    /// 点 `(x, y)`（相对左上角）处的插入位置，以及该点是否落在文字上。
    pub fn hit(&self, x: f32, y: f32) -> (usize, bool) {
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        if unsafe {
            self.inner
                .HitTestPoint(x, y, &mut trailing, &mut inside, &mut m)
        }
        .is_err()
        {
            return (0, false);
        }
        let pos = m.textPosition as usize
            + if trailing.0 != 0 {
                m.length as usize
            } else {
                0
            };
        (pos.min(self.len), inside.0 != 0)
    }

    /// 点 `(x, y)` 下的字符位置（不在文字上时为 None）。
    pub fn char_at(&self, x: f32, y: f32) -> Option<usize> {
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            self.inner
                .HitTestPoint(x, y, &mut trailing, &mut inside, &mut m)
                .ok()?
        };
        (inside.0 != 0).then_some(m.textPosition as usize)
    }

    /// 文字范围覆盖的矩形（每行一个，相对左上角）。
    pub fn range_rects(&self, start: usize, len: usize) -> Vec<(f32, f32, f32, f32)> {
        if len == 0 || start >= self.len {
            return Vec::new();
        }
        let len = len.min(self.len - start) as u32;
        let mut count = 0u32;
        // 先取数量（缓冲区不足时返回错误但会给出数量）
        unsafe {
            let _ = self
                .inner
                .HitTestTextRange(start as u32, len, 0.0, 0.0, None, &mut count);
        }
        if count == 0 {
            return Vec::new();
        }
        let mut m = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        if unsafe {
            self.inner
                .HitTestTextRange(start as u32, len, 0.0, 0.0, Some(&mut m), &mut count)
        }
        .is_err()
        {
            return Vec::new();
        }
        m.truncate(count as usize);
        m.iter()
            .map(|r| (r.left, r.top, r.left + r.width, r.top + r.height))
            .collect()
    }

    /// 插入位置的光标坐标 (x, y, 行高)。
    pub fn caret(&self, pos: usize) -> (f32, f32, f32) {
        let (mut x, mut y) = (0f32, 0f32);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        let pos = pos.min(self.len) as u32;
        if unsafe {
            self.inner
                .HitTestTextPosition(pos, BOOL(0), &mut x, &mut y, &mut m)
        }
        .is_err()
        {
            return (0.0, 0.0, 0.0);
        }
        (x, m.top, m.height)
    }
}

// ---------------------------------------------------------------------------
// 位图与图片解码

/// 设备相关的位图，只能画在创建它的 [`Surface`] 上（渲染目标重建后要重新创建）。
pub struct Bitmap {
    inner: ID2D1Bitmap,
    surface: u64,
}

impl Bitmap {
    /// 是否属于这个渲染目标。
    pub fn belongs_to(&self, s: &Surface) -> bool {
        self.surface == s.id
    }
}

/// 解码后的图片（预乘 alpha 的 BGRA）。
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// 原始尺寸
    pub orig_width: u32,
    pub orig_height: u32,
    pub pixels: Vec<u8>,
}

/// 用 WIC 解码图片文件的第一帧，宽度超过 `max_width` 时等比缩小。
/// 可在任何线程调用（按需把线程初始化为 MTA）；原图超过 `max_pixels` 像素时拒绝，防止耗尽内存。
pub fn decode_image(
    path: &std::path::Path,
    max_width: u32,
    max_pixels: u64,
) -> std::result::Result<Image, String> {
    use std::os::windows::ffi::OsStrExt;
    let wpath: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let err = |e: Error| e.message();
    unsafe {
        // 已初始化（S_FALSE）或模式不同（RPC_E_CHANGED_MODE）都可以继续使用
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let f: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        let dec = f
            .CreateDecoderFromFilename(
                PCWSTR(wpath.as_ptr()),
                None,
                GENERIC_ACCESS_RIGHTS(0x8000_0000),
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(err)?;
        let frame = dec.GetFrame(0).map_err(err)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(err)?;
        if w == 0 || h == 0 {
            return Err("图片尺寸为 0".into());
        }
        if w as u64 * h as u64 > max_pixels {
            return Err(format!("图片太大（{w}×{h}）"));
        }
        let (tw, th) = if w > max_width.max(1) {
            let tw = max_width.max(1);
            let th = ((h as u64 * tw as u64) / w as u64).max(1) as u32;
            (tw, th)
        } else {
            (w, h)
        };
        let scaler = f.CreateBitmapScaler().map_err(err)?;
        scaler
            .Initialize(&frame, tw, th, WICBitmapInterpolationModeFant)
            .map_err(err)?;
        let conv = f.CreateFormatConverter().map_err(err)?;
        conv.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(err)?;
        let stride = tw * 4;
        let mut pixels = vec![0u8; stride as usize * th as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut pixels)
            .map_err(err)?;
        Ok(Image {
            width: tw,
            height: th,
            orig_width: w,
            orig_height: h,
            pixels,
        })
    }
}

//! 共享的 Direct2D / DirectWrite 渲染设施。
//!
//! 只在 UI 线程使用：工厂按线程缓存，渲染目标绑定到窗口。
//! 坐标单位一律是物理像素（渲染目标的 DPI 固定为 96），字号由调用方按 DPI 换算成像素。
//!
//! - [`Surface`]：窗口渲染目标 + 一个纯色画刷；渲染目标丢失（显卡重置等）时由调用方丢弃重建
//! - [`Font`]：文本格式与行高、字符宽度等度量（统一行距，保证中英文混排时各行基线一致）
//! - [`TextLayout`]：单行文本排版，提供命中测试（坐标 ↔ 字符位置）

#![cfg(windows)]

use std::cell::RefCell;

use windows::core::{Error, Result, PCWSTR};
use windows::Win32::Foundation::{BOOL, D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS, DWRITE_LINE_METRICS,
    DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP,
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

/// 窗口的渲染目标。
pub struct Surface {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
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
        Ok(Surface { rt, brush })
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
        let (_, dw) = factories()?;
        let face_w = wide(face);
        let locale = wide("zh-cn");
        let fmt = unsafe {
            dw.CreateTextFormat(
                PCWSTR(face_w.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
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

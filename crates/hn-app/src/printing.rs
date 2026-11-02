//! 打印（`app` 的子模块）：PrintDlgEx 选择打印机与范围，Scintilla 的 `SCI_FORMATRANGEFULL` 分页绘制正文
//! （保留语法颜色、白色背景），页眉（文件名、时间）与页脚（页码）用 GDI 绘制。
//!
//! - 缩放：样式字号含当前缩放比例，打印前临时按 100% 应用样式，打印后恢复（期间关闭重绘）
//! - 先用 `draw = false` 走一遍得到总页数，再逐页打印；每页都检查"有没有前进"，防止版面过小时死循环
//! - 打印机的设备与纸张设置保留到下次打印

use windows_sys::Win32::Foundation::{GetLastError, RECT, SYSTEMTIME};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject, FillRect,
    GetDeviceCaps, GetTextExtentPoint32W, SelectObject, SetBkMode, SetTextColor, TextOutW, HDC,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Controls::Dialogs::{
    PrintDlgExW, PD_NOCURRENTPAGE, PD_NOSELECTION, PD_PAGENUMS, PD_RESULT_PRINT, PD_RETURNDC,
    PD_SELECTION, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGEXW, PRINTPAGERANGE, START_PAGE_GENERAL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor};

use super::*;

const HORZRES: i32 = 8;
const VERTRES: i32 = 10;
const LOGPIXELSX: i32 = 88;
const LOGPIXELSY: i32 = 90;
const PHYSICALWIDTH: i32 = 110;
const PHYSICALHEIGHT: i32 = 111;
const PHYSICALOFFSETX: i32 = 112;
const PHYSICALOFFSETY: i32 = 113;
const TRANSPARENT: i32 = 1;
const ERROR_CANCELLED: u32 = 1223;
const IDC_WAIT: u16 = 32514;
/// 页边距（毫米）
const MARGIN_MM: f32 = 15.0;

/// 一页的版面（设备坐标，原点为可打印区域左上角）。
#[derive(Debug, Clone, Copy)]
struct Geometry {
    /// 正文区域
    body: (i32, i32, i32, i32),
    /// 整个可打印区域
    page: (i32, i32, i32, i32),
    left: i32,
    right: i32,
    header_y: i32,
    footer_y: i32,
    /// 页眉页脚字体高度
    font_h: i32,
}

fn geometry(hdc: HDC) -> Geometry {
    let caps = |i| unsafe { GetDeviceCaps(hdc, i) };
    let (dpix, dpiy) = (caps(LOGPIXELSX).max(72), caps(LOGPIXELSY).max(72));
    let (hres, vres) = (caps(HORZRES).max(100), caps(VERTRES).max(100));
    let (mut pw, mut ph, mut ox, mut oy) = (
        caps(PHYSICALWIDTH),
        caps(PHYSICALHEIGHT),
        caps(PHYSICALOFFSETX),
        caps(PHYSICALOFFSETY),
    );
    if pw <= 0 || ph <= 0 {
        // 屏幕 / 内存 DC（自测）：没有物理纸张，按可打印区域算
        (pw, ph, ox, oy) = (hres, vres, 0, 0);
    }
    let mx = (MARGIN_MM / 25.4 * dpix as f32) as i32;
    let my = (MARGIN_MM / 25.4 * dpiy as f32) as i32;
    let left = (mx - ox).clamp(0, hres / 3);
    let right = (pw - mx - ox).clamp(hres * 2 / 3, hres);
    let top = (my - oy).clamp(0, vres / 3);
    let bottom = (ph - my - oy).clamp(vres * 2 / 3, vres);
    let font_h = (9 * dpiy / 72).max(8);
    let band = font_h * 2;
    Geometry {
        body: (
            left,
            top + band,
            right,
            (bottom - band).max(top + band + font_h),
        ),
        page: (0, 0, hres, vres),
        left,
        right,
        header_y: top,
        footer_y: bottom - font_h,
        font_h,
    }
}

/// 打印任务的范围与标题。
pub(super) struct PrintJob {
    pub title: String,
    pub start: usize,
    pub end: usize,
    /// 只打印这些页（从 1 起，含两端）
    pub pages: Option<(u32, u32)>,
}

impl App {
    /// 文件 → 打印。
    pub(super) fn print_active(&self) {
        let Some(sci) = self.sci() else { return };
        if self.active_overlay().is_some() {
            self.show_message("大文件视图与 Hex 视图不支持打印");
            return;
        }
        if sci.is_empty() {
            self.show_message("文档是空的");
            return;
        }
        let (ss, se) = (sci.selection_start(), sci.selection_end());
        let has_sel = se > ss;
        let mut range = PRINTPAGERANGE {
            nFromPage: 1,
            nToPage: 1,
        };
        let (devmode, devnames) = self.print_dev.get();
        let mut pd: PRINTDLGEXW = unsafe { std::mem::zeroed() };
        pd.lStructSize = std::mem::size_of::<PRINTDLGEXW>() as u32;
        pd.hwndOwner = self.hwnd();
        pd.hDevMode = devmode as _;
        pd.hDevNames = devnames as _;
        pd.Flags = PD_RETURNDC
            | PD_USEDEVMODECOPIESANDCOLLATE
            | PD_NOCURRENTPAGE
            | if has_sel { 0 } else { PD_NOSELECTION };
        pd.nPageRanges = 0;
        pd.nMaxPageRanges = 1;
        pd.lpPageRanges = &mut range;
        pd.nMinPage = 1;
        pd.nMaxPage = 0xFFFF;
        pd.nCopies = 1;
        pd.nStartPage = START_PAGE_GENERAL;
        let hr = unsafe { PrintDlgExW(&mut pd) };
        // 保留打印机与纸张设置供下次使用
        self.print_dev
            .set((pd.hDevMode as isize, pd.hDevNames as isize));
        if hr != 0 {
            dialogs::error_box(
                self.hwnd(),
                &format!("无法打开打印对话框（0x{:08X}）", hr as u32),
            );
            return;
        }
        let hdc = pd.hDC;
        if pd.dwResultAction != PD_RESULT_PRINT {
            if !hdc.is_null() {
                unsafe { DeleteDC(hdc) };
            }
            return;
        }
        if hdc.is_null() {
            dialogs::error_box(self.hwnd(), "打印机没有返回设备上下文");
            return;
        }
        let title = self
            .active_doc(|d| {
                d.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| d.title())
            })
            .unwrap_or_default();
        let (start, end) = if pd.Flags & PD_SELECTION != 0 && has_sel {
            (ss, se)
        } else {
            (0, sci.len())
        };
        let pages = (pd.Flags & PD_PAGENUMS != 0 && pd.nPageRanges > 0).then(|| {
            let (a, b) = (range.nFromPage.max(1), range.nToPage.max(1));
            (a.min(b), a.max(b))
        });
        let job = PrintJob {
            title,
            start,
            end,
            pages,
        };
        unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT as usize as _)) };
        let res = self.print_to(hdc, &job, true);
        unsafe { DeleteDC(hdc) };
        match res {
            Ok(n) => self.show_message(&format!("已发送到打印机：{n} 页")),
            Err(e) if e.is_empty() => {}
            Err(e) => dialogs::error_box(self.hwnd(), &format!("打印失败：{e}")),
        }
    }

    /// 打印到设备上下文。`device` 为真时用 StartDoc/StartPage 输出到打印机，否则只排版
    /// （自测用内存 DC）。返回打印的页数；用户取消返回空错误。
    pub(super) fn print_to(&self, hdc: HDC, job: &PrintJob, device: bool) -> Result<u32, String> {
        let Some(sci) = self.sci() else {
            return Err("没有编辑视图".into());
        };
        let g = geometry(hdc);
        // 按 100% 缩放应用样式（打印字号与屏幕缩放无关），打印后恢复
        sci.set_redraw(false);
        let (font, lang) = {
            let s = self.settings.borrow();
            let lang = self
                .active_doc(|d| {
                    if d.long_lines {
                        "text"
                    } else {
                        LANGS[d.lang].id
                    }
                })
                .unwrap_or("text");
            (
                theme::FontSpec {
                    face: s.font_face.clone(),
                    size_x100: s.font_size_x100,
                },
                lang,
            )
        };
        theme::apply_styles(&sci, &GRUVBOX_LIGHT, &font, 100, lang);
        sci.send(SCI_SETPRINTCOLOURMODE, SC_PRINT_COLOURONWHITE as usize, 0);
        sci.send(SCI_SETPRINTMAGNIFICATION, 0, 0);
        sci.send(SCI_SETPRINTWRAPMODE, SC_WRAP_WORD as usize, 0);
        let res = self.print_pages(&sci, hdc, &g, job, device);
        self.apply_view(self.focus.get());
        sci.set_redraw(true);
        res
    }

    fn print_pages(
        &self,
        sci: &Sci,
        hdc: HDC,
        g: &Geometry,
        job: &PrintJob,
        device: bool,
    ) -> Result<u32, String> {
        // 第一遍：分页，记下每页的起点
        let mut starts = Vec::new();
        let mut pos = job.start;
        while pos < job.end {
            starts.push(pos);
            let next = sci.format_range(false, hdc as _, g.body, g.page, pos, job.end);
            if next <= pos {
                break; // 版面放不下任何内容
            }
            pos = next;
        }
        let total = starts.len() as u32;
        if total == 0 {
            return Err("没有可打印的内容".into());
        }
        let (from, to) = job.pages.unwrap_or((1, total));
        let (from, to) = (from.clamp(1, total), to.clamp(1, total));
        if device {
            let name = wide(&job.title);
            let di = DOCINFOW {
                cbSize: std::mem::size_of::<DOCINFOW>() as i32,
                lpszDocName: name.as_ptr(),
                lpszOutput: null(),
                lpszDatatype: null(),
                fwType: 0,
            };
            if unsafe { StartDocW(hdc, &di) } <= 0 {
                let e = unsafe { GetLastError() };
                return Err(if e == ERROR_CANCELLED {
                    String::new()
                } else {
                    format!("无法开始打印（错误 {e}）")
                });
            }
        }
        let time = {
            let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
            unsafe { GetLocalTime(&mut t) };
            format!(
                "{}-{:02}-{:02} {:02}:{:02}",
                t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute
            )
        };
        let mut printed = 0;
        for page in from..=to {
            let idx = (page - 1) as usize;
            let start = starts[idx];
            if device && unsafe { StartPage(hdc) } <= 0 {
                unsafe { AbortDoc(hdc) };
                return Err("打印机拒绝了新的一页".into());
            }
            self.print_decorations(hdc, g, &job.title, &time, page, total);
            sci.format_range(true, hdc as _, g.body, g.page, start, job.end);
            if device && unsafe { EndPage(hdc) } <= 0 {
                unsafe { AbortDoc(hdc) };
                return Err("打印机拒绝了这一页".into());
            }
            printed += 1;
        }
        if device {
            unsafe { EndDoc(hdc) };
        }
        Ok(printed)
    }

    /// 页眉（左：文件名，右：时间）、页脚（居中：页码）与分隔线。
    #[allow(clippy::too_many_arguments)]
    fn print_decorations(
        &self,
        hdc: HDC,
        g: &Geometry,
        title: &str,
        time: &str,
        page: u32,
        total: u32,
    ) {
        let face = wide("Microsoft YaHei UI");
        unsafe {
            let font = CreateFontW(
                -g.font_h,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                0,
                0,
                face.as_ptr(),
            );
            let old = SelectObject(hdc, font as _);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, theme::bgr(GRUVBOX_LIGHT.gray) as u32);
            let width_of = |s: &[u16]| {
                let mut sz = windows_sys::Win32::Foundation::SIZE { cx: 0, cy: 0 };
                GetTextExtentPoint32W(hdc, s.as_ptr(), s.len() as i32, &mut sz);
                sz.cx
            };
            let out = |x: i32, y: i32, s: &[u16]| {
                TextOutW(hdc, x, y, s.as_ptr(), s.len() as i32);
            };
            let t: Vec<u16> = time.encode_utf16().collect();
            let tw = width_of(&t);
            out(g.right - tw, g.header_y, &t);
            // 文件名太长时从左边截掉
            let full: Vec<u16> = title.encode_utf16().collect();
            let room = (g.right - g.left - tw - g.font_h * 2).max(g.font_h);
            let mut name = full.clone();
            let mut cut = 0;
            while width_of(&name) > room && full.len() - cut > 4 {
                cut += ((full.len() - cut) / 10).max(1);
                name = "…"
                    .encode_utf16()
                    .chain(full[cut..].iter().copied())
                    .collect();
            }
            out(g.left, g.header_y, &name);
            let foot: Vec<u16> = format!("第 {page} 页，共 {total} 页")
                .encode_utf16()
                .collect();
            let fw = width_of(&foot);
            out((g.left + g.right - fw) / 2, g.footer_y, &foot);
            let brush = CreateSolidBrush(theme::bgr(GRUVBOX_LIGHT.bg3) as u32);
            let thick = (g.font_h / 12).max(1);
            let ly = g.header_y + g.font_h * 3 / 2;
            let r1 = RECT {
                left: g.left,
                top: ly,
                right: g.right,
                bottom: ly + thick,
            };
            FillRect(hdc, &r1, brush);
            let fy = g.footer_y - g.font_h / 2;
            let r2 = RECT {
                left: g.left,
                top: fy,
                right: g.right,
                bottom: fy + thick,
            };
            FillRect(hdc, &r2, brush);
            DeleteObject(brush as _);
            SelectObject(hdc, old);
            DeleteObject(font as _);
        }
    }

    /// 自测：在内存 DC 上排版并"打印"当前文档，返回页数。
    pub(crate) fn st_print_pages(&self) -> Result<u32, String> {
        let Some(sci) = self.sci() else {
            return Err("no sci".into());
        };
        let hdc = unsafe { CreateCompatibleDC(null_mut()) };
        if hdc.is_null() {
            return Err("CreateCompatibleDC 失败".into());
        }
        let job = PrintJob {
            title: "selftest".into(),
            start: 0,
            end: sci.len(),
            pages: None,
        };
        let r = self.print_to(hdc, &job, false);
        unsafe { DeleteDC(hdc) };
        r
    }
}

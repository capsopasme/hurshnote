//! 保存、加密文件、外部修改处理（`app` 的子模块，可访问 App 的私有字段）。

use hn_core::payload::Payload;
use hn_core::stamp::DiskStamp;
use hn_crypto::{KdfParams, Key};
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use super::*;
use crate::fsio::{self, SaveError};

/// 显示等待光标（下次移动鼠标时恢复）。
unsafe fn set_wait_cursor() {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursor;
    unsafe { SetCursor(LoadCursorW(null_mut(), 32514usize as _)) };
}
use crate::pwdlg;
use crate::watcher::Event as FsEvent;

impl App {
    pub(super) fn watch(&self, id: u32, path: &Path) {
        if let Some(w) = self.watcher.borrow().as_ref() {
            w.watch(id, path.to_path_buf());
        }
    }

    pub(super) fn unwatch(&self, id: u32) {
        if let Some(w) = self.watcher.borrow().as_ref() {
            w.unwatch(id);
        }
    }

    /// 反复询问密码，直到 `open` 成功或用户取消。
    pub(super) fn ask_password_loop<T>(
        &self,
        prompt: &str,
        open: impl Fn(&str) -> Result<T, String>,
    ) -> Option<T> {
        loop {
            let pw = pwdlg::ask(self.hwnd(), self.hinstance, "输入密码", prompt, false)?;
            match open(&pw) {
                Ok(v) => return Some(v),
                Err(e) => dialogs::error_box(self.hwnd(), &e),
            }
        }
    }

    /// 读取并解密加密文件。返回 (密钥, 内容, 磁盘戳)。取消返回 None。
    pub(super) fn open_encrypted_file(&self, path: &Path) -> Option<(Key, Payload, DiskStamp)> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &format!("无法读取 {}\n\n{e}", path.display()));
                return None;
            }
        };
        if let Err(e) = hn_crypto::parse_header(&bytes) {
            dialogs::error_box(self.hwnd(), &format!("{}\n\n{e}", path.display()));
            return None;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prompt = format!("“{name}”已加密，请输入密码：");
        let (key, payload) = self.ask_password_loop(&prompt, |pw| {
            let (k, pt) = hn_crypto::open_with_password(pw, &bytes).map_err(|e| e.to_string())?;
            let p = Payload::decode(&pt).map_err(|e| e.to_string())?;
            Ok((k, p))
        })?;
        let stamp = DiskStamp::new(fsio::mtime(path).unwrap_or(0), &bytes);
        Some((key, payload, stamp))
    }

    /// 打开加密文件为新文档。
    pub(super) fn open_encrypted(
        &self,
        path: &Path,
        goto: Option<(usize, Option<usize>)>,
        read_only: bool,
    ) {
        if let Some((key, payload, stamp)) = self.open_encrypted_file(path) {
            self.install_encrypted(path, key, payload, stamp, goto, read_only);
        }
    }

    /// 把已解密的内容装为新文档。
    pub(super) fn install_encrypted(
        &self,
        path: &Path,
        key: Key,
        payload: Payload,
        stamp: DiskStamp,
        goto: Option<(usize, Option<usize>)>,
        read_only: bool,
    ) -> Option<u32> {
        let sci = self.sci()?;
        let Some(ptr) = self.document_from_text(&payload.text) else {
            dialogs::error_box(self.hwnd(), "内存不足，无法打开文件。");
            return None;
        };
        let pristine = self.pristine_untitled();
        let id = self.alloc_id();
        let lang_idx = payload
            .get("lang")
            .and_then(lang::by_id)
            .map(lang::index_of)
            .unwrap_or_else(|| lang::index_of(lang::detect(Some(path), "")));
        let eol = payload
            .get("eol")
            .and_then(docs::eol_from_str)
            .unwrap_or_else(|| hn_text::eol::detect(&payload.text, Eol::Crlf));
        let mut d = Doc::new(id, ptr);
        d.path = Some(path.to_path_buf());
        d.lang = lang_idx;
        d.key = Some(key);
        d.disk = Some(stamp);
        d.init = Some(docs::DocInit {
            eol: Some(eol),
            read_only,
            ..Default::default()
        });
        self.docs.borrow_mut().push(d);
        self.activate(id);
        if let Some(p) = pristine {
            self.close_doc_unchecked(p);
        }
        if let Some((l, c)) = goto {
            editops::goto_line_col(&sci, l, c);
        }
        self.watch(id, path);
        self.add_recent(path);
        self.refresh();
        Some(id)
    }

    pub(super) fn add_recent(&self, path: &Path) {
        let mut r = self.recent.borrow_mut();
        config::push_recent(&mut r, path);
        config::save_recent(&r);
    }

    /// 安全写入，处理只读属性与权限不足（提权）。返回保存后的修改时间。
    fn save_bytes(&self, path: &Path, data: &[u8]) -> Option<u64> {
        let hwnd = self.hwnd();
        let mut cleared = false;
        loop {
            match fsio::save_file(path, data) {
                Ok(t) => return Some(t),
                Err(SaveError::ReadOnlyAttr) if !cleared => {
                    let q = format!(
                        "{}\n\n文件带有只读属性。去掉只读属性并保存吗？",
                        path.display()
                    );
                    if dialogs::ask_yes_no_cancel(hwnd, &q) != Answer::Yes {
                        return None;
                    }
                    if let Err(e) = fsio::clear_readonly(path) {
                        // 可能也没有权限修改属性，交给提权流程
                        if e.kind() != std::io::ErrorKind::PermissionDenied {
                            dialogs::error_box(hwnd, &format!("无法去掉只读属性：{e}"));
                            return None;
                        }
                    }
                    cleared = true;
                }
                Err(SaveError::AccessDenied) | Err(SaveError::ReadOnlyAttr) => {
                    let q = format!(
                        "{}\n\n没有写入这个位置的权限。以管理员身份保存吗？\n（将弹出 Windows 用户账户控制提示）",
                        path.display()
                    );
                    if dialogs::ask_yes_no_cancel(hwnd, &q) != Answer::Yes {
                        return None;
                    }
                    return match fsio::save_elevated(hwnd, path, data) {
                        Ok(()) => Some(fsio::mtime(path).unwrap_or(0)),
                        Err(e) => {
                            dialogs::error_box(hwnd, &format!("保存失败：{e}"));
                            None
                        }
                    };
                }
                Err(SaveError::Io(e)) => {
                    dialogs::error_box(hwnd, &format!("无法保存 {}\n\n{e}", path.display()));
                    return None;
                }
            }
        }
    }

    /// 把当前文档写入 `path`。
    pub(super) fn write_active(&self, path: &Path) -> bool {
        let Some(sci) = self.sci() else { return false };
        let hwnd = self.hwnd();
        let id = self.active.get();
        let Some((mut enc, lossy, key, lang_idx)) =
            self.active_doc(|d| (d.enc, d.lossy, d.key.clone(), d.lang))
        else {
            return false;
        };
        let bytes = sci.bytes();
        let data = match &key {
            Some(k) => {
                let payload = Payload::new(bytes)
                    .with("eol", docs::eol_to_str(editops::eol_of(&sci)))
                    .with("lang", LANGS[lang_idx].id);
                match hn_crypto::seal(k, &payload.encode()) {
                    Ok(d) => d,
                    Err(e) => {
                        dialogs::error_box(hwnd, &format!("加密失败：{e}"));
                        return false;
                    }
                }
            }
            None => {
                if lossy {
                    let msg = format!(
                        "这个文件按 {} 打开时有无法识别的字节，已显示为 �。\n保存后这些字节会被永久替换。\n\n仍然保存吗？",
                        enc.label()
                    );
                    if dialogs::ask_yes_no_cancel(hwnd, &msg) != Answer::Yes {
                        return false;
                    }
                }
                let text = String::from_utf8_lossy(&bytes);
                let mut out = encoding::encode(&text, enc);
                if out.unmappable > 0 {
                    let msg = format!(
                        "有 {} 个字符无法用 {} 表示，按此编码保存会变成“?”。\n\n是：改用 UTF-8 保存\n否：仍按 {} 保存\n取消：不保存",
                        out.unmappable,
                        enc.label(),
                        enc.label()
                    );
                    match dialogs::ask_yes_no_cancel(hwnd, &msg) {
                        Answer::Yes => {
                            enc = EncodingInfo::UTF8;
                            out = encoding::encode(&text, enc);
                        }
                        Answer::No => {}
                        Answer::Cancel => return false,
                    }
                }
                out.bytes
            }
        };

        let Some(mtime) = self.save_bytes(path, &data) else {
            return false;
        };
        sci.set_save_point();
        self.with_doc_mut(id, |d| {
            d.enc = enc;
            d.lossy = false;
            d.meta_dirty = false;
            d.modified = false;
            d.missing = false;
            d.disk = Some(DiskStamp::new(mtime, &data));
        });
        self.watch(id, path);
        self.add_recent(path);
        // 已保存：删除快照
        self.snapshot_active();
        self.refresh();
        true
    }

    /// 首次加密前说明局限（只提示一次）。返回是否继续。
    fn confirm_first_encryption(&self) -> bool {
        if self.settings.borrow().crypto_warned || self.selftest_out.is_some() {
            return true;
        }
        let text = "关于加密文件：\n\n\
            • 用 Argon2id 从密码派生密钥、AES-256-GCM 加密；换一台电脑用同一密码即可打开。\n\
            • 忘记密码就无法恢复内容，HurshNote 没有任何后门或找回办法。\n\
            • 编辑期间明文在内存和剪贴板中，无法防御本机上的恶意软件。\n\
            • 未保存修改的快照用同一密钥加密后保存。\n\n\
            继续吗？";
        if !dialogs::ask_ok_cancel(self.hwnd(), text) {
            return false;
        }
        self.toggle_setting(|s| s.crypto_warned = true);
        true
    }

    /// 密码过短时确认。返回是否继续使用它。
    fn accept_password(&self, pw: &str) -> bool {
        if pw.chars().count() >= 8 || self.selftest_out.is_some() {
            return true;
        }
        dialogs::ask_ok_cancel(
            self.hwnd(),
            "密码少于 8 个字符，容易被暴力破解。\n\n仍然使用这个密码吗？",
        )
    }

    /// 派生密钥（约 0.2–0.5 秒，期间显示等待光标）。
    fn derive_new_key(&self, pw: &str) -> Option<Key> {
        unsafe { set_wait_cursor() };
        match Key::generate(pw, KdfParams::default()) {
            Ok(k) => Some(k),
            Err(e) => {
                dialogs::error_box(self.hwnd(), &format!("{e}"));
                None
            }
        }
    }

    /// 另存为加密文件。
    pub(super) fn save_encrypted_as(&self) -> bool {
        if !self.confirm_first_encryption() {
            return false;
        }
        let pw = loop {
            let Some(pw) = pwdlg::ask(
                self.hwnd(),
                self.hinstance,
                "另存为加密文件",
                "设置密码。打开这个文件时需要输入它：",
                true,
            ) else {
                return false;
            };
            if self.accept_password(&pw) {
                break pw;
            }
        };
        let Some(key) = self.derive_new_key(&pw) else {
            return false;
        };
        let current = self.active_doc(|d| d.path.clone()).flatten();
        let suggested = current
            .map(|p| p.with_extension("hne"))
            .or_else(|| self.active_doc(|d| PathBuf::from(format!("{}.hne", d.title()))));
        let Some(path) = dialogs::save_file(self.hwnd(), suggested.as_deref()) else {
            return false;
        };
        let old_key = self.active_doc_mut(|d| d.key.replace(key)).flatten();
        if !self.write_active(&path) {
            self.active_doc_mut(|d| d.key = old_key);
            return false;
        }
        self.active_doc_mut(|d| {
            d.path = Some(path.clone());
            d.enc = EncodingInfo::UTF8;
        });
        self.show_message("已加密保存。请牢记密码，忘记后无法恢复。");
        self.refresh();
        true
    }

    /// 文件 → 修改密码：确认当前密码后换新密码（新的随机 salt）并立即保存。
    pub(super) fn change_password(&self) -> bool {
        let Some((old_key, path)) = self
            .active_doc(|d| d.key.clone().zip(d.path.clone()))
            .flatten()
        else {
            self.show_message("当前文档没有加密。可以用“另存为加密文件”加密它");
            return false;
        };
        loop {
            let Some(pw) = pwdlg::ask(
                self.hwnd(),
                self.hinstance,
                "修改密码",
                "输入当前密码：",
                false,
            ) else {
                return false;
            };
            unsafe { set_wait_cursor() };
            match old_key.verify(&pw) {
                Ok(true) => break,
                Ok(false) => dialogs::error_box(self.hwnd(), "当前密码不正确。"),
                Err(e) => {
                    dialogs::error_box(self.hwnd(), &format!("{e}"));
                    return false;
                }
            }
        }
        let pw = loop {
            let Some(pw) = pwdlg::ask(
                self.hwnd(),
                self.hinstance,
                "修改密码",
                "设置新密码：",
                true,
            ) else {
                return false;
            };
            if self.accept_password(&pw) {
                break pw;
            }
        };
        let Some(key) = self.derive_new_key(&pw) else {
            return false;
        };
        let old = self.active_doc_mut(|d| d.key.replace(key)).flatten();
        if !self.write_active(&path) {
            self.active_doc_mut(|d| d.key = old);
            dialogs::error_box(self.hwnd(), "保存失败，密码没有修改。");
            return false;
        }
        self.show_message("密码已修改并重新加密保存。请牢记新密码。");
        true
    }

    /// 文件 → 另存为不加密的文件：当前文档改为指向新的普通文件，原加密文件不变。
    pub(super) fn save_decrypted_as(&self) -> bool {
        let Some(current) = self
            .active_doc(|d| d.is_encrypted().then(|| d.path.clone()))
            .flatten()
        else {
            self.show_message("当前文档没有加密");
            return false;
        };
        if !dialogs::ask_ok_cancel(
            self.hwnd(),
            "另存为不加密的普通文件？\n\n新文件中的内容任何人都能读取；原来的加密文件保持不变。",
        ) {
            return false;
        }
        // notes.md.hne → notes.md；没有内层扩展名时用 .txt
        let suggested = current.as_ref().map(|p| {
            let is_hne = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hne"));
            let stripped = if is_hne {
                p.with_extension("")
            } else {
                p.clone()
            };
            if stripped.extension().is_some() && is_hne {
                stripped
            } else {
                stripped.with_extension("txt")
            }
        });
        let Some(path) = dialogs::save_file(self.hwnd(), suggested.as_deref()) else {
            return false;
        };
        if current
            .as_deref()
            .is_some_and(|c| config::same_path(c, &path))
            && !dialogs::ask_ok_cancel(
                self.hwnd(),
                "这会用明文覆盖原来的加密文件，加密版本将不复存在。\n\n继续吗？",
            )
        {
            return false;
        }
        let old_key = self.active_doc_mut(|d| d.key.take()).flatten();
        self.active_doc_mut(|d| d.enc = EncodingInfo::UTF8);
        if !self.write_active(&path) {
            self.active_doc_mut(|d| d.key = old_key);
            return false;
        }
        drop(old_key);
        let new_lang = lang::index_of(lang::detect(Some(&path), ""));
        let changed = self.active_doc_mut(|d| {
            d.path = Some(path.clone());
            let c = d.lang == 0 && new_lang != 0;
            if c {
                d.lang = new_lang;
            }
            c
        });
        if changed == Some(true) {
            self.set_active_language(new_lang);
        }
        self.show_message("已另存为不加密的普通文件");
        self.refresh();
        true
    }

    // ---- 自测 ----

    /// 自测：用给定的新密码修改当前加密文档的密码（跳过对话框）。
    pub(crate) fn st_change_password(&self, old: &str, new: &str) -> Result<(), String> {
        let (key, path) = self
            .active_doc(|d| d.key.clone().zip(d.path.clone()))
            .flatten()
            .ok_or("not encrypted")?;
        if !key.verify(old).map_err(|e| e.to_string())? {
            return Err("old password rejected".into());
        }
        if key.verify(new).map_err(|e| e.to_string())? && old != new {
            return Err("new password accepted by old key".into());
        }
        let k = Key::generate(new, KdfParams::default()).map_err(|e| e.to_string())?;
        let prev = self.active_doc_mut(|d| d.key.replace(k)).flatten();
        if !self.write_active(&path) {
            self.active_doc_mut(|d| d.key = prev);
            return Err("write failed".into());
        }
        Ok(())
    }

    // ---- 外部修改 ----

    pub(super) fn on_fs_events(&self) {
        while let Ok(ev) = self.fs_rx.try_recv() {
            let mut p = self.pending_fs.borrow_mut();
            if !p.contains(&ev) {
                p.push(ev);
            }
        }
        // 窗口不在前台时，等激活后再提示
        if unsafe { GetForegroundWindow() } == self.hwnd() {
            self.process_fs_events();
        }
    }

    pub(super) fn process_fs_events(&self) {
        let events: Vec<FsEvent> = self.pending_fs.borrow_mut().drain(..).collect();
        for ev in events {
            match ev {
                FsEvent::Deleted(id) => {
                    let Some(name) = self.with_doc(id, |d| d.title()) else {
                        continue;
                    };
                    self.with_doc_mut(id, |d| d.missing = true);
                    self.show_message(&format!("“{name}”已在磁盘上被删除或移走"));
                    self.refresh();
                }
                FsEvent::Changed(id) => self.on_file_changed(id),
            }
        }
    }

    fn on_file_changed(&self, id: u32) {
        let Some((path, stamp, name, loaded, large)) = self.with_doc(id, |d| {
            (d.path.clone(), d.disk, d.title(), d.ptr != 0, d.is_large)
        }) else {
            return;
        };
        if large {
            return; // 大文件视图自己轮询（不能为算哈希读整个文件）
        }
        let Some(path) = path else { return };
        let Some(cur) = docs::disk_stamp(&path) else {
            return;
        };
        self.with_doc_mut(id, |d| d.missing = false);
        if stamp.is_some_and(|s| s.size == cur.size && s.hash == cur.hash) {
            // 内容没变（例如自己保存、外部只改了时间）
            self.with_doc_mut(id, |d| d.disk = Some(cur));
            return;
        }
        if !loaded {
            // 尚未载入的会话文档：载入时会自然读取新内容
            self.with_doc_mut(id, |d| d.disk = Some(cur));
            return;
        }
        let dirty = self.is_dirty(id);
        let auto = self.settings.borrow().auto_reload;
        let reload = if !dirty && auto {
            true
        } else {
            let q = if dirty {
                format!("“{name}”已被其他程序修改。\n\n重新载入吗？你的未保存修改将丢失。")
            } else {
                format!("“{name}”已被其他程序修改。\n\n重新载入吗？")
            };
            dialogs::ask_yes_no_cancel(self.hwnd(), &q) == Answer::Yes
        };
        if reload {
            self.reload_doc(id);
            self.show_message(&format!("已重新载入“{name}”"));
        } else {
            // 不再为这个版本提示；保存时会覆盖
            self.with_doc_mut(id, |d| d.disk = Some(cur));
        }
    }

    /// 从磁盘重新载入文档（保持编码与光标所在行）。
    pub(super) fn reload_doc(&self, id: u32) {
        let Some((path, enc, encrypted)) =
            self.with_doc(id, |d| (d.path.clone(), d.enc, d.key.is_some()))
        else {
            return;
        };
        let Some(path) = path else { return };
        // 文档可能在另一视图中，激活后再取焦点视图
        self.activate(id);
        let Some(sci) = self.sci() else { return };
        if encrypted {
            let key = self.with_doc(id, |d| d.key.clone()).flatten();
            let res = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    let k = key.ok_or("缺少密钥")?;
                    let pt = hn_crypto::open(&k, &b).map_err(|e| e.to_string())?;
                    let p = Payload::decode(&pt).map_err(|e| e.to_string())?;
                    Ok((p, DiskStamp::new(fsio::mtime(&path).unwrap_or(0), &b)))
                });
            match res {
                Ok((p, st)) => {
                    let line = sci.line_from_pos(sci.current_pos());
                    let ro = sci.is_read_only();
                    sci.send(SCI_SETREADONLY, 0, 0);
                    sci.load_document(&p.text);
                    sci.send(SCI_SETREADONLY, ro as usize, 0);
                    sci.send(SCI_GOTOLINE, line, 0);
                    self.with_doc_mut(id, |d| {
                        d.disk = Some(st);
                        d.meta_dirty = false;
                    });
                    self.snapshot_active();
                }
                Err(e) => dialogs::error_box(self.hwnd(), &format!("无法重新载入：{e}")),
            }
            return;
        }
        let size = match docs::check_size(&path) {
            Ok(s) => s,
            Err(e) => {
                dialogs::error_box(self.hwnd(), &e);
                return;
            }
        };
        let Some(loader) = sci.create_loader(size as usize) else {
            return;
        };
        let job = LoadJob {
            path,
            forced: Some(enc.encoding),
            reuse: Some(id),
            goto: None,
            read_only: sci.is_read_only(),
        };
        self.start_load(job, loader, size);
    }
}

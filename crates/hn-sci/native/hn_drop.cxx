// 编辑区的 OLE 拖放目标：同时支持拖入文件与拖放文字。
//
// Scintilla 自带的拖放目标只接受文字，拖入文件会被拒绝。这里在 Scintilla 注册之后撤销它的注册，
// 换成自己的实现：
// - 文件（CF_HDROP）：把路径交给回调（由程序延后打开，避免在拖放过程中弹出对话框）
// - 文字：插入到鼠标位置。若拖动的正是本视图的选区且为"移动"，插入后把选区放回原文字上，
//   Scintilla 在 DoDragDrop 返回后会删除选区（它认为拖到了外部），从而完成移动。
// 拖动过程中用指示器在落点下方画一个小三角表示插入位置。

#include <windows.h>
#include <ole2.h>
#include <shellapi.h>

#include <cstring>
#include <cwchar>
#include <string>

#include "Scintilla.h"

typedef void (*HnDropFilesFn)(void *ctx, const wchar_t *paths);

namespace {

constexpr int INDIC_DROP = 9;

class DropTarget final : public IDropTarget {
	LONG refs = 1;
	HWND sci;
	HnDropFilesFn callback;
	void *ctx;
	bool files = false;
	bool text = false;

	LRESULT S(UINT m, WPARAM w = 0, LPARAM l = 0) const {
		return ::SendMessageW(sci, m, w, l);
	}

	static bool Has(IDataObject *d, CLIPFORMAT cf) {
		FORMATETC f = {cf, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
		return d->QueryGetData(&f) == S_OK;
	}

	Sci_Position PosAt(POINTL pt) const {
		POINT p = {pt.x, pt.y};
		::ScreenToClient(sci, &p);
		return static_cast<Sci_Position>(S(SCI_POSITIONFROMPOINT, p.x, p.y));
	}

	void Mark(Sci_Position pos) const {
		const Sci_Position len = static_cast<Sci_Position>(S(SCI_GETLENGTH));
		S(SCI_SETINDICATORCURRENT, INDIC_DROP);
		S(SCI_INDICATORCLEARRANGE, 0, len);
		if (pos >= 0 && pos < len) {
			S(SCI_INDICATORFILLRANGE, pos, 1);
		}
	}

	DWORD Effect(DWORD keys, DWORD allowed) const {
		if (files) {
			return allowed & DROPEFFECT_COPY ? DROPEFFECT_COPY : DROPEFFECT_NONE;
		}
		if (!text || S(SCI_GETREADONLY)) {
			return DROPEFFECT_NONE;
		}
		DWORD e = (keys & MK_CONTROL) ? DROPEFFECT_COPY : DROPEFFECT_MOVE;
		if (!(allowed & e)) {
			e = (allowed & DROPEFFECT_COPY) ? DROPEFFECT_COPY : (allowed & DROPEFFECT_MOVE);
		}
		return e;
	}

	static std::string GetText(IDataObject *d) {
		std::string out;
		FORMATETC f = {CF_UNICODETEXT, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
		STGMEDIUM m = {};
		if (d->GetData(&f, &m) == S_OK) {
			const wchar_t *w = static_cast<const wchar_t *>(::GlobalLock(m.hGlobal));
			if (w) {
				const int wlen = static_cast<int>(wcsnlen(w, ::GlobalSize(m.hGlobal) / sizeof(wchar_t)));
				const int n = ::WideCharToMultiByte(CP_UTF8, 0, w, wlen, nullptr, 0, nullptr, nullptr);
				out.resize(n);
				::WideCharToMultiByte(CP_UTF8, 0, w, wlen, out.data(), n, nullptr, nullptr);
				::GlobalUnlock(m.hGlobal);
			}
			::ReleaseStgMedium(&m);
			return out;
		}
		f.cfFormat = CF_TEXT;
		if (d->GetData(&f, &m) == S_OK) {
			const char *a = static_cast<const char *>(::GlobalLock(m.hGlobal));
			if (a) {
				const int alen = static_cast<int>(strnlen(a, ::GlobalSize(m.hGlobal)));
				const int wn = ::MultiByteToWideChar(CP_ACP, 0, a, alen, nullptr, 0);
				std::wstring w(wn, L'\0');
				::MultiByteToWideChar(CP_ACP, 0, a, alen, w.data(), wn);
				const int n = ::WideCharToMultiByte(CP_UTF8, 0, w.data(), wn, nullptr, 0, nullptr, nullptr);
				out.resize(n);
				::WideCharToMultiByte(CP_UTF8, 0, w.data(), wn, out.data(), n, nullptr, nullptr);
				::GlobalUnlock(m.hGlobal);
			}
			::ReleaseStgMedium(&m);
		}
		return out;
	}

public:
	DropTarget(HWND sci_, HnDropFilesFn cb, void *ctx_) : sci(sci_), callback(cb), ctx(ctx_) {}

	STDMETHODIMP QueryInterface(REFIID riid, void **ppv) override {
		if (riid == IID_IUnknown || riid == IID_IDropTarget) {
			*ppv = static_cast<IDropTarget *>(this);
			AddRef();
			return S_OK;
		}
		*ppv = nullptr;
		return E_NOINTERFACE;
	}
	STDMETHODIMP_(ULONG) AddRef() override {
		return ::InterlockedIncrement(&refs);
	}
	STDMETHODIMP_(ULONG) Release() override {
		const LONG r = ::InterlockedDecrement(&refs);
		if (r == 0) {
			delete this;
		}
		return r;
	}

	STDMETHODIMP DragEnter(IDataObject *d, DWORD keys, POINTL pt, DWORD *eff) override {
		files = Has(d, CF_HDROP);
		text = !files && (Has(d, CF_UNICODETEXT) || Has(d, CF_TEXT));
		*eff = Effect(keys, *eff);
		if (text) {
			Mark(PosAt(pt));
		}
		return S_OK;
	}

	STDMETHODIMP DragOver(DWORD keys, POINTL pt, DWORD *eff) override {
		*eff = Effect(keys, *eff);
		if (text) {
			Mark(PosAt(pt));
		}
		return S_OK;
	}

	STDMETHODIMP DragLeave() override {
		Mark(-1);
		return S_OK;
	}

	STDMETHODIMP Drop(IDataObject *d, DWORD keys, POINTL pt, DWORD *eff) override {
		Mark(-1);
		if (files) {
			FORMATETC f = {CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
			STGMEDIUM m = {};
			if (d->GetData(&f, &m) == S_OK) {
				HDROP h = static_cast<HDROP>(m.hGlobal);
				const UINT n = ::DragQueryFileW(h, 0xFFFFFFFF, nullptr, 0);
				std::wstring buf;
				for (UINT i = 0; i < n; i++) {
					const UINT len = ::DragQueryFileW(h, i, nullptr, 0);
					std::wstring s(len, L'\0');
					::DragQueryFileW(h, i, s.data(), len + 1);
					buf += s;
					buf.push_back(L'\0');
				}
				buf.push_back(L'\0');
				if (callback) {
					callback(ctx, buf.c_str());
				}
				::ReleaseStgMedium(&m);
			}
			*eff = DROPEFFECT_COPY;
			return S_OK;
		}
		if (!text) {
			*eff = DROPEFFECT_NONE;
			return S_OK;
		}
		const std::string utf8 = GetText(d);
		const DWORD e = Effect(keys, *eff);
		if (utf8.empty() || e == DROPEFFECT_NONE) {
			*eff = DROPEFFECT_NONE;
			return S_OK;
		}
		Sci_Position pos = PosAt(pt);
		if (pos < 0) {
			pos = static_cast<Sci_Position>(S(SCI_GETLENGTH));
		}
		const Sci_Position a = static_cast<Sci_Position>(S(SCI_GETSELECTIONSTART));
		const Sci_Position b = static_cast<Sci_Position>(S(SCI_GETSELECTIONEND));
		bool internal = false;
		if (b > a && static_cast<Sci_Position>(utf8.size()) == b - a) {
			std::string sel(static_cast<size_t>(b - a) + 1, '\0');
			S(SCI_GETSELTEXT, 0, reinterpret_cast<LPARAM>(sel.data()));
			sel.resize(static_cast<size_t>(b - a));
			internal = sel == utf8;
		}
		const bool move = internal && e == DROPEFFECT_MOVE;
		if (move && pos >= a && pos <= b) {
			// 拖回原处：什么也不做
			*eff = DROPEFFECT_NONE;
			return S_OK;
		}
		const Sci_Position len = static_cast<Sci_Position>(utf8.size());
		S(SCI_INSERTTEXT, pos, reinterpret_cast<LPARAM>(utf8.c_str()));
		if (move) {
			const Sci_Position shift = pos < a ? len : 0;
			S(SCI_SETSEL, a + shift, b + shift);
		} else {
			S(SCI_SETSEL, pos, pos + len);
		}
		*eff = e;
		return S_OK;
	}
};

}

extern "C" int hn_drop_register(HWND sci, HnDropFilesFn callback, void *ctx) {
	::RevokeDragDrop(sci);
	DropTarget *t = new DropTarget(sci, callback, ctx);
	const HRESULT hr = ::RegisterDragDrop(sci, t);
	t->Release();
	return SUCCEEDED(hr) ? 1 : 0;
}

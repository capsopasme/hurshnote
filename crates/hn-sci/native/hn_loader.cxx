// Scintilla ILoader 的 C 接口封装，供 Rust 调用 C++ 虚函数。
//
// ILoader 由 SCI_CREATELOADER 在 UI 线程创建，可以在后台线程填充数据（AddData），
// 最后 ConvertToDocument 得到可挂到视图上的文档指针（引用计数为 1）。

#include <cstddef>

#include "Sci_Position.h"
#include "ILoader.h"

extern "C" {

int hn_loader_add(void *loader, const char *data, ptrdiff_t length) {
	return static_cast<Scintilla::ILoader *>(loader)->AddData(data, length);
}

void *hn_loader_convert(void *loader) {
	return static_cast<Scintilla::ILoader *>(loader)->ConvertToDocument();
}

int hn_loader_release(void *loader) {
	return static_cast<Scintilla::ILoader *>(loader)->Release();
}

}

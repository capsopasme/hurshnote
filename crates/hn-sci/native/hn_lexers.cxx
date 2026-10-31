// HurshNote 的 Lexilla lexer 目录。
//
// 只登记 build.rs 中 LEXERS 列表编译进来的 lexer，代替 Lexilla.cxx（后者引用全部 lexer）。
// 新增 lexer 时：build.rs 的 LEXERS 加源文件，这里加 extern 声明和目录项。

#include <cstring>
#include <vector>
#include <initializer_list>

#include "ILexer.h"
#include "LexerModule.h"
#include "CatalogueModules.h"

using namespace Lexilla;

extern const LexerModule lmBash;
extern const LexerModule lmBatch;
extern const LexerModule lmCPP;
extern const LexerModule lmCss;
extern const LexerModule lmDiff;
extern const LexerModule lmHTML;
extern const LexerModule lmJSON;
extern const LexerModule lmMarkdown;
extern const LexerModule lmNull;
extern const LexerModule lmPowerShell;
extern const LexerModule lmProps;
extern const LexerModule lmPython;
extern const LexerModule lmRust;
extern const LexerModule lmSQL;
extern const LexerModule lmTOML;
extern const LexerModule lmXML;
extern const LexerModule lmYAML;

namespace {

CatalogueModules catalogue;

void Init() {
	if (catalogue.Count() > 0) {
		return;
	}
	catalogue.AddLexerModules({
		&lmBash,
		&lmBatch,
		&lmCPP,
		&lmCss,
		&lmDiff,
		&lmHTML,
		&lmJSON,
		&lmMarkdown,
		&lmNull,
		&lmPowerShell,
		&lmProps,
		&lmPython,
		&lmRust,
		&lmSQL,
		&lmTOML,
		&lmXML,
		&lmYAML,
	});
}

}

extern "C" {

// 按名称创建 lexer（名称见各 LexerModule 定义，如 "python"、"cpp"、"xml"）。
// 返回的对象交给 SCI_SETILEXER 后由 Scintilla 负责释放；找不到时返回 nullptr。
Scintilla::ILexer5 *hn_create_lexer(const char *name) {
	Init();
	for (size_t i = 0; i < catalogue.Count(); i++) {
		if (0 == std::strcmp(catalogue.Name(i), name)) {
			return catalogue.Create(i);
		}
	}
	return nullptr;
}

int hn_lexer_count() {
	Init();
	return static_cast<int>(catalogue.Count());
}

}

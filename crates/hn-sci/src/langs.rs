//! 各语言的 lexer 配置：样式号 → 语义样式、关键字列表、lexer 属性。
//!
//! 语言 id 与 `hn_text::lang::LANGS` 一致。

use crate::theme::Sem::{self, *};
use crate::*;

pub struct LexConf {
    /// (lexer 样式号, 语义样式)
    pub styles: &'static [(isize, Sem)],
    /// 关键字列表，下标对应 SCI_SETKEYWORDS 的集合号。
    pub keywords: &'static [&'static str],
    pub props: &'static [(&'static str, &'static str)],
}

/// 所有语言共用的属性。
pub const COMMON_PROPS: &[(&str, &str)] = &[
    ("fold", "1"),
    ("fold.compact", "0"),
    ("fold.comment", "1"),
    ("fold.preprocessor", "1"),
    ("fold.html", "1"),
    ("fold.quotes.python", "1"),
    ("lexer.cpp.track.preprocessor", "0"),
    ("lexer.cpp.escape.sequence", "1"),
    ("lexer.json.allow.comments", "1"),
    ("lexer.json.escape.sequence", "1"),
];

const NONE: LexConf = LexConf {
    styles: &[],
    keywords: &[],
    props: &[],
};

const CPP_STYLES: &[(isize, Sem)] = &[
    (SCE_C_COMMENT, Comment),
    (SCE_C_COMMENTLINE, Comment),
    (SCE_C_COMMENTDOC, CommentDoc),
    (SCE_C_COMMENTLINEDOC, CommentDoc),
    (SCE_C_COMMENTDOCKEYWORD, CommentDoc),
    (SCE_C_COMMENTDOCKEYWORDERROR, CommentDoc),
    (SCE_C_NUMBER, Number),
    (SCE_C_WORD, Keyword),
    (SCE_C_WORD2, Type),
    (SCE_C_GLOBALCLASS, Type),
    (SCE_C_STRING, String),
    (SCE_C_CHARACTER, Char),
    (SCE_C_VERBATIM, String),
    (SCE_C_STRINGRAW, String),
    (SCE_C_TRIPLEVERBATIM, String),
    (SCE_C_HASHQUOTEDSTRING, String),
    (SCE_C_UUID, Constant),
    (SCE_C_PREPROCESSOR, Preprocessor),
    (SCE_C_PREPROCESSORCOMMENT, Comment),
    (SCE_C_PREPROCESSORCOMMENTDOC, CommentDoc),
    (SCE_C_OPERATOR, Operator),
    (SCE_C_STRINGEOL, Error),
    (SCE_C_REGEX, Regex),
    (SCE_C_USERLITERAL, Number),
    (SCE_C_TASKMARKER, TaskMarker),
    (SCE_C_ESCAPESEQUENCE, Escape),
];

const C_KW: &str = "alignas alignof and asm auto bool break case catch char char8_t char16_t char32_t class \
const consteval constexpr constinit const_cast continue co_await co_return co_yield decltype default delete do \
double dynamic_cast else enum explicit export extern false float for friend goto if inline int long mutable \
namespace new noexcept not nullptr operator or private protected public register reinterpret_cast requires \
restrict return short signed sizeof static static_assert static_cast struct switch template this thread_local \
throw true try typedef typeid typename union unsigned using virtual void volatile wchar_t while xor _Bool";
const C_TYPES: &str = "size_t ssize_t ptrdiff_t intptr_t uintptr_t int8_t int16_t int32_t int64_t uint8_t \
uint16_t uint32_t uint64_t FILE std string wstring string_view vector array map unordered_map set unordered_set \
list deque pair tuple optional variant unique_ptr shared_ptr weak_ptr BOOL BYTE WORD DWORD QWORD UINT INT LONG \
ULONG HANDLE HWND HINSTANCE HMODULE LPCSTR LPSTR LPCWSTR LPWSTR WCHAR TCHAR LPARAM WPARAM LRESULT HRESULT";

const CS_KW: &str = "abstract as base bool break byte case catch char checked class const continue decimal \
default delegate do double else enum event explicit extern false finally fixed float for foreach goto if implicit \
in int interface internal is lock long namespace new null object operator out override params private protected \
public readonly ref return sbyte sealed short sizeof stackalloc static string struct switch this throw true try \
typeof uint ulong unchecked unsafe ushort using virtual void volatile while async await var dynamic get set value \
yield partial where record init required global nameof when with";

const JAVA_KW: &str = "abstract assert boolean break byte case catch char class const continue default do \
double else enum extends final finally float for goto if implements import instanceof int interface long native \
new package private protected public return short static strictfp super switch synchronized this throw throws \
transient try void volatile while true false null var record sealed permits yield fun val when object companion \
data override internal open lateinit suspend";

const JS_KW: &str = "async await break case catch class const continue debugger default delete do else export \
extends false finally for from function get if import in instanceof let new null of return set static super \
switch this throw true try typeof undefined var void while with yield";
const JS_TYPES: &str =
    "Array Boolean Date Error Function JSON Map Math Number Object Promise Proxy Reflect \
RegExp Set String Symbol WeakMap WeakSet console window document globalThis";

const TS_KW: &str = "async await break case catch class const continue debugger default delete do else export \
extends false finally for from function get if import in instanceof let new null of return set static super \
switch this throw true try typeof undefined var void while with yield abstract any as asserts bigint boolean \
declare enum implements infer interface is keyof module namespace never number override private protected public \
readonly require satisfies string symbol type unique unknown";

const PY_STYLES: &[(isize, Sem)] = &[
    (SCE_P_COMMENTLINE, Comment),
    (SCE_P_COMMENTBLOCK, Comment),
    (SCE_P_NUMBER, Number),
    (SCE_P_STRING, String),
    (SCE_P_CHARACTER, String),
    (SCE_P_TRIPLE, String),
    (SCE_P_TRIPLEDOUBLE, String),
    (SCE_P_FSTRING, String),
    (SCE_P_FCHARACTER, String),
    (SCE_P_FTRIPLE, String),
    (SCE_P_FTRIPLEDOUBLE, String),
    (SCE_P_WORD, Keyword),
    (SCE_P_WORD2, Keyword2),
    (SCE_P_CLASSNAME, Type),
    (SCE_P_DEFNAME, Function),
    (SCE_P_OPERATOR, Operator),
    (SCE_P_STRINGEOL, Error),
    (SCE_P_DECORATOR, Preprocessor),
    (SCE_P_ATTRIBUTE, Default),
];

const PY_KW: &str = "False None True and as assert async await break class continue def del elif else except \
finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case";
const PY_BUILTINS: &str = "abs all any ascii bin bool breakpoint bytearray bytes callable chr classmethod compile \
complex delattr dict dir divmod enumerate eval exec filter float format frozenset getattr globals hasattr hash help \
hex id input int isinstance issubclass iter len list locals map max memoryview min next object oct open ord pow \
print property range repr reversed round set setattr slice sorted staticmethod str sum super tuple type vars zip \
self cls __init__ __name__ __main__ Exception ValueError TypeError KeyError IndexError AttributeError RuntimeError \
StopIteration OSError IOError FileNotFoundError NotImplementedError ImportError";

const JSON_STYLES: &[(isize, Sem)] = &[
    (SCE_JSON_NUMBER, Number),
    (SCE_JSON_STRING, String),
    (SCE_JSON_STRINGEOL, Error),
    (SCE_JSON_PROPERTYNAME, Key),
    (SCE_JSON_ESCAPESEQUENCE, Escape),
    (SCE_JSON_LINECOMMENT, Comment),
    (SCE_JSON_BLOCKCOMMENT, Comment),
    (SCE_JSON_OPERATOR, Operator),
    (SCE_JSON_URI, Link),
    (SCE_JSON_COMPACTIRI, Link),
    (SCE_JSON_KEYWORD, Constant),
    (SCE_JSON_LDKEYWORD, Keyword),
    (SCE_JSON_ERROR, Error),
];

const HTML_STYLES: &[(isize, Sem)] = &[
    (SCE_H_TAG, Tag),
    (SCE_H_TAGUNKNOWN, Tag),
    (SCE_H_TAGEND, Tag),
    (SCE_H_ATTRIBUTE, Attribute),
    (SCE_H_ATTRIBUTEUNKNOWN, Attribute),
    (SCE_H_NUMBER, Number),
    (SCE_H_DOUBLESTRING, String),
    (SCE_H_SINGLESTRING, String),
    (SCE_H_VALUE, String),
    (SCE_H_COMMENT, Comment),
    (SCE_H_XCCOMMENT, Comment),
    (SCE_H_ENTITY, Entity),
    (SCE_H_XMLSTART, Preprocessor),
    (SCE_H_XMLEND, Preprocessor),
    (SCE_H_CDATA, Code),
    (SCE_H_QUESTION, Preprocessor),
    (SCE_H_SGML_DEFAULT, Preprocessor),
    (SCE_H_SGML_COMMAND, Preprocessor),
    (SCE_H_SGML_1ST_PARAM, Preprocessor),
    (SCE_H_SGML_DOUBLESTRING, String),
    (SCE_H_SGML_SIMPLESTRING, String),
    (SCE_H_SGML_COMMENT, Comment),
    (SCE_H_SGML_ENTITY, Entity),
    (SCE_H_SGML_ERROR, Error),
    (SCE_HJ_COMMENT, Comment),
    (SCE_HJ_COMMENTLINE, Comment),
    (SCE_HJ_COMMENTDOC, CommentDoc),
    (SCE_HJ_NUMBER, Number),
    (SCE_HJ_KEYWORD, Keyword),
    (SCE_HJ_DOUBLESTRING, String),
    (SCE_HJ_SINGLESTRING, String),
    (SCE_HJ_TEMPLATELITERAL, String),
    (SCE_HJ_SYMBOLS, Operator),
    (SCE_HJ_STRINGEOL, Error),
    (SCE_HJ_REGEX, Regex),
];

const SQL_STYLES: &[(isize, Sem)] = &[
    (SCE_SQL_COMMENT, Comment),
    (SCE_SQL_COMMENTLINE, Comment),
    (SCE_SQL_COMMENTDOC, CommentDoc),
    (SCE_SQL_COMMENTLINEDOC, CommentDoc),
    (SCE_SQL_COMMENTDOCKEYWORD, CommentDoc),
    (SCE_SQL_COMMENTDOCKEYWORDERROR, CommentDoc),
    (SCE_SQL_NUMBER, Number),
    (SCE_SQL_WORD, Keyword),
    (SCE_SQL_WORD2, Keyword2),
    (SCE_SQL_STRING, String),
    (SCE_SQL_CHARACTER, String),
    (SCE_SQL_OPERATOR, Operator),
    (SCE_SQL_QUOTEDIDENTIFIER, Variable),
    (SCE_SQL_SQLPLUS, Preprocessor),
    (SCE_SQL_SQLPLUS_PROMPT, Preprocessor),
    (SCE_SQL_SQLPLUS_COMMENT, Comment),
    (SCE_SQL_USER1, Function),
    (SCE_SQL_QOPERATOR, Operator),
];

const SQL_KW: &str = "add all alter and any as asc begin between by cascade case cast check column commit \
constraint create cross current_date current_timestamp database declare default delete desc distinct drop else \
end exec execute exists fetch for foreign from full function grant group having if in index inner insert intersect \
into is join key left like limit merge natural not null offset on or order outer over partition primary procedure \
references replace return returns revoke right rollback row rows schema select set table then top transaction \
trigger truncate union unique update use using values view when where while with recursive window except \
int integer bigint smallint tinyint varchar nvarchar char nchar text date time datetime timestamp boolean bool \
float real double decimal numeric blob serial";
const SQL_FUNCS: &str = "count sum avg min max coalesce nullif isnull ifnull concat substring substr length len \
upper lower trim ltrim rtrim round floor ceiling ceil abs now getdate dateadd datediff row_number rank dense_rank \
lag lead first_value last_value convert";

const YAML_STYLES: &[(isize, Sem)] = &[
    (SCE_YAML_COMMENT, Comment),
    (SCE_YAML_IDENTIFIER, Key),
    (SCE_YAML_KEYWORD, Constant),
    (SCE_YAML_NUMBER, Number),
    (SCE_YAML_REFERENCE, Variable),
    (SCE_YAML_DOCUMENT, Section),
    (SCE_YAML_TEXT, String),
    (SCE_YAML_ERROR, Error),
    (SCE_YAML_OPERATOR, Operator),
];

const MD_STYLES: &[(isize, Sem)] = &[
    (SCE_MARKDOWN_STRONG1, Strong),
    (SCE_MARKDOWN_STRONG2, Strong),
    (SCE_MARKDOWN_EM1, Emphasis),
    (SCE_MARKDOWN_EM2, Emphasis),
    (SCE_MARKDOWN_HEADER1, Heading),
    (SCE_MARKDOWN_HEADER2, Heading),
    (SCE_MARKDOWN_HEADER3, Heading),
    (SCE_MARKDOWN_HEADER4, Heading),
    (SCE_MARKDOWN_HEADER5, Heading),
    (SCE_MARKDOWN_HEADER6, Heading),
    (SCE_MARKDOWN_PRECHAR, Code),
    (SCE_MARKDOWN_ULIST_ITEM, ListMarker),
    (SCE_MARKDOWN_OLIST_ITEM, ListMarker),
    (SCE_MARKDOWN_BLOCKQUOTE, Quote),
    (SCE_MARKDOWN_STRIKEOUT, Strike),
    (SCE_MARKDOWN_HRULE, Comment),
    (SCE_MARKDOWN_LINK, Link),
    (SCE_MARKDOWN_CODE, Code),
    (SCE_MARKDOWN_CODE2, Code),
    (SCE_MARKDOWN_CODEBK, Code),
    (SCE_MARKDOWN_FRONT_MARK, Comment),
    (SCE_MARKDOWN_FRONT, Comment),
    (SCE_MARKDOWN_FRONT_KEY, Key),
];

const PROPS_STYLES: &[(isize, Sem)] = &[
    (SCE_PROPS_COMMENT, Comment),
    (SCE_PROPS_SECTION, Section),
    (SCE_PROPS_ASSIGNMENT, Operator),
    (SCE_PROPS_DEFVAL, Constant),
    (SCE_PROPS_KEY, Key),
];

const TOML_STYLES: &[(isize, Sem)] = &[
    (SCE_TOML_COMMENT, Comment),
    (SCE_TOML_KEYWORD, Constant),
    (SCE_TOML_NUMBER, Number),
    (SCE_TOML_TABLE, Section),
    (SCE_TOML_KEY, Key),
    (SCE_TOML_ERROR, Error),
    (SCE_TOML_OPERATOR, Operator),
    (SCE_TOML_STRING_SQ, String),
    (SCE_TOML_STRING_DQ, String),
    (SCE_TOML_TRIPLE_STRING_SQ, String),
    (SCE_TOML_TRIPLE_STRING_DQ, String),
    (SCE_TOML_ESCAPECHAR, Escape),
    (SCE_TOML_DATETIME, Number),
    (SCE_TOML_STRINGEOL, Error),
];

const BAT_STYLES: &[(isize, Sem)] = &[
    (SCE_BAT_COMMENT, Comment),
    (SCE_BAT_WORD, Keyword),
    (SCE_BAT_LABEL, Label),
    (SCE_BAT_HIDE, Operator),
    (SCE_BAT_COMMAND, Function),
    (SCE_BAT_IDENTIFIER, Variable),
    (SCE_BAT_OPERATOR, Operator),
    (SCE_BAT_AFTER_LABEL, Comment),
];

const BAT_KW: &str = "rem set if else exist errorlevel for in do break call copy chcp cd chdir choice cls \
date del erase dir echo exit goto md mkdir move path pause prompt ren rename rd rmdir shift time type ver verify \
vol nul con defined not equ neq lss leq gtr geq setlocal endlocal enabledelayedexpansion enableextensions start \
pushd popd title color mode assoc ftype";

const PS_STYLES: &[(isize, Sem)] = &[
    (SCE_POWERSHELL_COMMENT, Comment),
    (SCE_POWERSHELL_COMMENTSTREAM, Comment),
    (SCE_POWERSHELL_COMMENTDOCKEYWORD, CommentDoc),
    (SCE_POWERSHELL_STRING, String),
    (SCE_POWERSHELL_CHARACTER, String),
    (SCE_POWERSHELL_HERE_STRING, String),
    (SCE_POWERSHELL_HERE_CHARACTER, String),
    (SCE_POWERSHELL_NUMBER, Number),
    (SCE_POWERSHELL_VARIABLE, Variable),
    (SCE_POWERSHELL_OPERATOR, Operator),
    (SCE_POWERSHELL_KEYWORD, Keyword),
    (SCE_POWERSHELL_CMDLET, Function),
    (SCE_POWERSHELL_ALIAS, Keyword2),
    (SCE_POWERSHELL_FUNCTION, Function),
    (SCE_POWERSHELL_USER1, Type),
];

const PS_KW: &str = "begin break catch class continue data default do dynamicparam else elseif end enum exit \
filter finally for foreach from function hidden if in param process return static switch throw trap try until \
using var while workflow";
const PS_CMDLETS: &str = "write-host write-output write-error write-warning write-verbose get-childitem \
get-content set-content add-content get-item set-item new-item remove-item copy-item move-item rename-item \
test-path join-path split-path resolve-path get-location set-location push-location pop-location select-object \
where-object foreach-object sort-object group-object measure-object format-table format-list out-file out-null \
out-string convertto-json convertfrom-json import-csv export-csv invoke-expression invoke-webrequest \
invoke-restmethod start-process stop-process get-process get-service start-service stop-service import-module \
get-command get-help get-member read-host start-sleep get-date";
const PS_ALIASES: &str =
    "cd ls dir cat echo cp mv rm del gci gc sc ii iwr irm ps kill sleep select where foreach \
sort measure clear cls pwd type copy move ren md";

const SH_STYLES: &[(isize, Sem)] = &[
    (SCE_SH_ERROR, Error),
    (SCE_SH_COMMENTLINE, Comment),
    (SCE_SH_NUMBER, Number),
    (SCE_SH_WORD, Keyword),
    (SCE_SH_STRING, String),
    (SCE_SH_CHARACTER, String),
    (SCE_SH_OPERATOR, Operator),
    (SCE_SH_SCALAR, Variable),
    (SCE_SH_PARAM, Variable),
    (SCE_SH_BACKTICKS, Escape),
    (SCE_SH_HERE_DELIM, Label),
    (SCE_SH_HERE_Q, String),
];

const SH_KW: &str = "if then else elif fi case esac for while until do done in function select return break \
continue export local readonly declare typeset unset shift exit source alias echo printf read test eval exec trap \
set cd pwd true false let";

const CSS_STYLES: &[(isize, Sem)] = &[
    (SCE_CSS_TAG, Tag),
    (SCE_CSS_CLASS, Type),
    (SCE_CSS_PSEUDOCLASS, Keyword2),
    (SCE_CSS_UNKNOWN_PSEUDOCLASS, Keyword2),
    (SCE_CSS_PSEUDOELEMENT, Keyword2),
    (SCE_CSS_EXTENDED_PSEUDOCLASS, Keyword2),
    (SCE_CSS_EXTENDED_PSEUDOELEMENT, Keyword2),
    (SCE_CSS_OPERATOR, Operator),
    (SCE_CSS_IDENTIFIER, Key),
    (SCE_CSS_IDENTIFIER2, Key),
    (SCE_CSS_IDENTIFIER3, Key),
    (SCE_CSS_EXTENDED_IDENTIFIER, Key),
    (SCE_CSS_UNKNOWN_IDENTIFIER, Key),
    (SCE_CSS_VALUE, Constant),
    (SCE_CSS_COMMENT, Comment),
    (SCE_CSS_ID, Function),
    (SCE_CSS_IMPORTANT, Keyword),
    (SCE_CSS_DIRECTIVE, Preprocessor),
    (SCE_CSS_DOUBLESTRING, String),
    (SCE_CSS_SINGLESTRING, String),
    (SCE_CSS_ATTRIBUTE, Attribute),
    (SCE_CSS_GROUP_RULE, Preprocessor),
    (SCE_CSS_VARIABLE, Variable),
];

const RUST_STYLES: &[(isize, Sem)] = &[
    (SCE_RUST_COMMENTBLOCK, Comment),
    (SCE_RUST_COMMENTLINE, Comment),
    (SCE_RUST_COMMENTBLOCKDOC, CommentDoc),
    (SCE_RUST_COMMENTLINEDOC, CommentDoc),
    (SCE_RUST_NUMBER, Number),
    (SCE_RUST_WORD, Keyword),
    (SCE_RUST_WORD2, Type),
    (SCE_RUST_WORD3, Keyword2),
    (SCE_RUST_STRING, String),
    (SCE_RUST_STRINGR, String),
    (SCE_RUST_CHARACTER, Char),
    (SCE_RUST_BYTESTRING, String),
    (SCE_RUST_BYTESTRINGR, String),
    (SCE_RUST_BYTECHARACTER, Char),
    (SCE_RUST_CSTRING, String),
    (SCE_RUST_CSTRINGR, String),
    (SCE_RUST_OPERATOR, Operator),
    (SCE_RUST_LIFETIME, Label),
    (SCE_RUST_MACRO, Preprocessor),
    (SCE_RUST_LEXERROR, Error),
];

const RUST_KW: &str = "as async await break const continue crate dyn else enum extern false fn for if impl in \
let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while \
yield union macro_rules";
const RUST_TYPES: &str =
    "bool char str u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32 f64 String Vec \
Option Result Box Rc Arc Cell RefCell HashMap HashSet BTreeMap Path PathBuf";
const RUST_CONSTS: &str = "Some None Ok Err";

const DIFF_STYLES: &[(isize, Sem)] = &[
    (SCE_DIFF_COMMENT, Comment),
    (SCE_DIFF_COMMAND, DiffHeader),
    (SCE_DIFF_HEADER, DiffHeader),
    (SCE_DIFF_POSITION, Constant),
    (SCE_DIFF_DELETED, Deleted),
    (SCE_DIFF_ADDED, Added),
    (SCE_DIFF_CHANGED, Changed),
    (SCE_DIFF_PATCH_ADD, Added),
    (SCE_DIFF_PATCH_DELETE, Deleted),
    (SCE_DIFF_REMOVED_PATCH_ADD, Added),
    (SCE_DIFF_REMOVED_PATCH_DELETE, Deleted),
];

static C: LexConf = LexConf {
    styles: CPP_STYLES,
    keywords: &[C_KW, C_TYPES],
    props: &[],
};
static CSHARP: LexConf = LexConf {
    styles: CPP_STYLES,
    keywords: &[CS_KW, ""],
    props: &[("lexer.cpp.verbatim.strings.allow.escapes", "0")],
};
static JAVA: LexConf = LexConf {
    styles: CPP_STYLES,
    keywords: &[JAVA_KW, ""],
    props: &[],
};
static JS: LexConf = LexConf {
    styles: CPP_STYLES,
    keywords: &[JS_KW, JS_TYPES],
    props: &[("lexer.cpp.backquoted.strings", "1")],
};
static TS: LexConf = LexConf {
    styles: CPP_STYLES,
    keywords: &[TS_KW, JS_TYPES],
    props: &[("lexer.cpp.backquoted.strings", "1")],
};
static PYTHON: LexConf = LexConf {
    styles: PY_STYLES,
    keywords: &[PY_KW, PY_BUILTINS],
    props: &[("lexer.python.decorator.attributes", "1")],
};
static JSON: LexConf = LexConf {
    styles: JSON_STYLES,
    keywords: &["true false null", ""],
    props: &[],
};
static XML: LexConf = LexConf {
    styles: HTML_STYLES,
    keywords: &[],
    props: &[],
};
static HTML: LexConf = LexConf {
    styles: HTML_STYLES,
    keywords: &["", JS_KW],
    props: &[],
};
static SQL: LexConf = LexConf {
    styles: SQL_STYLES,
    keywords: &[SQL_KW, "", "", "", SQL_FUNCS],
    props: &[],
};
static YAML: LexConf = LexConf {
    styles: YAML_STYLES,
    keywords: &["true false yes no null on off"],
    props: &[],
};
static MARKDOWN: LexConf = LexConf {
    styles: MD_STYLES,
    keywords: &[],
    props: &[],
};
static INI: LexConf = LexConf {
    styles: PROPS_STYLES,
    keywords: &[],
    props: &[],
};
static TOML: LexConf = LexConf {
    styles: TOML_STYLES,
    keywords: &["true false inf nan"],
    props: &[],
};
static BATCH: LexConf = LexConf {
    styles: BAT_STYLES,
    keywords: &[BAT_KW],
    props: &[],
};
static POWERSHELL: LexConf = LexConf {
    styles: PS_STYLES,
    keywords: &[PS_KW, PS_CMDLETS, PS_ALIASES],
    props: &[],
};
static BASH: LexConf = LexConf {
    styles: SH_STYLES,
    keywords: &[SH_KW],
    props: &[],
};
static CSS: LexConf = LexConf {
    styles: CSS_STYLES,
    keywords: &[],
    props: &[],
};
static RUST: LexConf = LexConf {
    styles: RUST_STYLES,
    keywords: &[RUST_KW, RUST_TYPES, RUST_CONSTS],
    props: &[],
};
static DIFF: LexConf = LexConf {
    styles: DIFF_STYLES,
    keywords: &[],
    props: &[],
};
static TEXT: LexConf = NONE;

/// 语言 id → lexer 配置；未知 id 返回空配置。
pub fn conf(lang_id: &str) -> &'static LexConf {
    match lang_id {
        "c" => &C,
        "csharp" => &CSHARP,
        "java" => &JAVA,
        "javascript" => &JS,
        "typescript" => &TS,
        "python" => &PYTHON,
        "json" => &JSON,
        "xml" => &XML,
        "html" => &HTML,
        "sql" => &SQL,
        "yaml" => &YAML,
        "markdown" => &MARKDOWN,
        "ini" => &INI,
        "toml" => &TOML,
        "batch" => &BATCH,
        "powershell" => &POWERSHELL,
        "bash" => &BASH,
        "css" => &CSS,
        "rust" => &RUST,
        "diff" => &DIFF,
        _ => &TEXT,
    }
}

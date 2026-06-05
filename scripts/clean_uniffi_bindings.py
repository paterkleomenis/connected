#!/usr/bin/env python3
"""
Apply safe in-place transforms to the UniFFI-generated Kotlin bindings and
prepend a comprehensive @file:Suppress header.

The transforms clean up cosmetic issues that the IDE flags but which are safe
to apply (they don't change semantics or break the FFI ABI). The generator
will re-emit the un-fixed version on the next regen, so this script is also
re-applied by the Gradle post-regen hook (see cleanUniffiBindings in
build.gradle.kts).

Usage:
    clean_uniffi_bindings.py [PATH]

If PATH is omitted, falls back to the in-repo default.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path
from re import Match

DEFAULT_PATH = Path(
    "~/connected/android/app/src/main/kotlin/uniffi/connected_ffi/connected_ffi.kt"
)

# The big @file:Suppress header. Covers everything that can't be fixed
# in-place
SUPPRESS_HEADER = """\
@file:Suppress(
    "ALL",

    // Proofreading / natural language
    "SpellCheckingInspection",
    "GrammaticalInspection",

    // Compiler warnings not covered by ALL
    "NAME_SHADOWING",
    "UNUSED_EXPRESSION",
    "UNUSED_VARIABLE",
    "UNUSED_PARAMETER",
    "unused",
    "RedundantVisibilityModifier",
    "RedundantUnitReturnType",
    "RedundantModalityModifier",
    "RedundantUnitExpression",
    "RedundantSemicolon",
    "RedundantComma",
    "RedundantTrailingComma",
    "RedundantLambdaArrow",
    "RemoveRedundantBackticks",
    "RemoveEmptyPrimaryConstructor",
    "RemoveEmptyParenthesesFromLambdaCall",
    "LiftReturnOrAssignment",
    "ExplicitTypeArgumentsCanBeInferred",
    "UnnecessaryExplicitTypeArguments",
    "UnnecessaryParenthesesInCallWithLambda",
    "CollectionAddAllCanBeReplacedWithConstructor",
    "ReplaceWithOperatorAssignment",
    "LambdaArgumentCouldBeLastArgument",
    "LambdaArgumentShouldBeLastArgument",
    "LambdaArgumentShouldBeMovedOutParentheses",
    "UsePropertyAccessSyntax",
    "UnusedReturnValue",
    "CanBeVal",
    "SameParameterValue",
    "ConvertCallChainIntoSequence",
    "Java8MapApi",
    "JavaStylePropertiesInvocation",
    "EnumValuesSoftDeprecateInJava",
    "EnumValuesTopLevelFunctionSoftDeprecate",

    // Imports
    "KotlinUnusedImport",
    "UnusedImport",

    // Naming conventions (FFI symbols don't follow Kotlin style)
    "ClassName",
    "FunctionName",
    "PropertyName",
    "LocalVariableName",
    "PrivatePropertyName",
    "ProtectedInFinal",
    "ConstructorParameterNaming",
    "NewClassNamingConvention",
    "VariableNaming",
    "ParameterNaming",
    "PackageNaming",
)

"""

# Kotlin hard keywords / soft keywords that legitimately require backticks.
# Identifiers matching any of these must stay backticked.
KOTLIN_RESERVED = {
    "as", "break", "class", "continue", "do", "else", "false", "for",
    "fun", "if", "in", "interface", "is", "null", "object", "package",
    "return", "super", "this", "throw", "true", "try", "typealias",
    "typeof", "val", "var", "when", "while",
    # Soft keywords
    "by", "catch", "constructor", "delegate", "dynamic", "field",
    "file", "finally", "get", "import", "init", "param", "property",
    "receiver", "set", "setparam", "value", "where",
    # Special identifiers that need quoting in some contexts
    "abstract", "actual", "annotation", "companion", "const",
    "crossinline", "data", "enum", "expect", "external", "final",
    "infix", "inline", "inner", "internal", "lateinit", "noinline",
    "open", "operator", "out", "override", "private", "protected",
    "public", "reified", "sealed", "suspend", "tailrec", "vararg",
}


def strip_public_modifier(text: str) -> str:
    """`^public ` at start of a declaration line -> ``.

    Only matches the standalone `public` keyword followed by a single space
    and then a Kotlin declaration keyword (object, fun, class, interface,
    abstract, data, enum, sealed, etc.). This avoids touching any string
    contents that happen to contain `public `.
    """
    pattern = re.compile(
        r"^public\s+(?=(?:abstract\s+|open\s+|final\s+|sealed\s+|data\s+|" +
        r"enum\s+|inner\s+|override\s+|suspend\s+|inline\s+|operator\s+|" +
        r"infix\s+|private\s+|internal\s+|protected\s+|public\s+|" +
        r"fun|val|var|class|object|interface)\b)",
        re.MULTILINE,
    )
    return pattern.sub("", text)


def strip_unit_return_type(text: str) -> str:
    """`): Unit$` on function declaration lines -> `)`."""
    return re.sub(r"\): Unit\s*$", ")", text, flags=re.MULTILINE)


def strip_kotlin_qualifier(text: str) -> str:
    """Drop the `kotlin.` qualifier from unambiguous built-in type references.

    Only replaces `kotlin.` immediately before one of the standard library
    types that this file uses (`Exception`, `String`, `Boolean`, `ULong`,
    `ByteArray`). Leaves any other `kotlin.` references alone.
    """
    return re.sub(
        r"\bkotlin\.(Exception|String|Boolean|ULong|UInt|UShort|ByteArray|Int|Long)\b",
        r"\1",
        text,
    )


def strip_redundant_backticks(text: str) -> str:
    """Drop backticks around identifiers that are not Kotlin reserved words."""
    out_parts: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == "`":
            # Find the closing backtick.
            j = text.find("`", i + 1)
            if j == -1:
                out_parts.append(text[i:])
                break
            ident = text[i + 1:j]
            if ident in KOTLIN_RESERVED or not ident.isidentifier():
                # Keep the backticks.
                out_parts.append(text[i : j + 1])
            else:
                out_parts.append(ident)
            i = j + 1
        else:
            out_parts.append(ch)
            i += 1
    return "".join(out_parts)


def strip_redundant_unit_expressions(text: str) -> str:
    """Drop standalone `Unit` lines inside `when` branches.

    These are leftover expression-as-statement markers; the surrounding `when`
    is already converted to an expression via `.let { ... }` at the end.
    """
    return re.sub(r"^[ \t]+Unit\s*$", "", text, flags=re.MULTILINE)


def simplify_zero_arg_lambdas(text: str) -> str:
    """`{ -> ... }` lambdas -> `{ ... }`.

    Only matches the simple empty-parameter case (`{ ->` followed by code,
    with `->` being the very first non-space content inside the braces).
    """
    out: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == "{":
            # Look ahead for ` ->` (whitespace then arrow).
            k = i + 1
            while k < n and text[k] in " \t":
                k += 1
            if k + 1 < n and text[k] == "-" and text[k + 1] == ">":
                # It's a `{ ->` lambda. Find the matching `}`.
                depth = 1
                m = i + 1
                while m < n and depth > 0:
                    if text[m] == "{":
                        depth += 1
                    elif text[m] == "}":
                        depth -= 1
                    m += 1
                body = text[k + 2 : m - 1]
                out.append("{" + body + "}")
                i = m
                continue
            out.append(ch)
            i += 1
        else:
            out.append(ch)
            i += 1
    return "".join(out)


def replace_enum_values_with_entries(text: str) -> str:
    return re.sub(r"\.values\(\)", ".entries", text)


def replace_handlemap_put(text: str) -> str:
    """Replace map.put(handle, obj) → map[handle] = obj in UniffiHandleMap."""
    return re.sub(r"\bmap\.put\((\w+),\s*(\w+)\)", r"map[\1] = \2", text)


def replace_handlemap_get(text: str) -> str:
    """Replace map.get(handle) → map[handle] and handleMap.get(handle) → handleMap[handle].

    Only targets handleMap.get and map.get, NOT buf.get or other ByteBuffer methods.
    """
    text = re.sub(r"\bhandleMap\.get\((\w+)\)", r"handleMap[\1]", text)
    text = re.sub(r"\bmap\.get\((\w+)\)", r"map[\1]", text)
    return text


def replace_map_sum_with_sumof(text: str) -> str:
    """Replace .map { ... }.sum() → .sumOf { ... }."""
    return re.sub(
        r"\.map\s*\{\s*(\w+\.\w+\(it\))\s*\}\s*\.sum\(\)",
        r".sumOf { \1 }",
        text,
    )


def add_operator_to_get(text: str) -> str:
    """Add 'operator' modifier to UniffiHandleMap.get() for bracket syntax."""
    return re.sub(
        r"(\bfun get\(handle: Long\): \w+)",
        r"operator \1",
        text,
    )


def move_lambda_out_of_error_call(text: str) -> str:
    """Move trailing lambda out of uniffiTraitInterfaceCallWithError().

    Converts:
        uniffiTraitInterfaceCallWithError(
            a, b, c,
            { e: ... -> ... }
        )
    To:
        uniffiTraitInterfaceCallWithError(
            a, b, c
        ) { e: ... -> ... }
    """
    # Match the full multi-line call with trailing lambda inside parens
    pattern = re.compile(
        r"(uniffiTraitInterfaceCallWithError\()" +  # call start
        r"([\s\S]*?)" +                             # args (non-greedy)
        r",\s*\n(\s*)" +                            # trailing comma + newline + indent
        r"(\{ [^}]+ \})" +                          # the lambda
        r"\s*\n\s*\)"                               # closing paren
    )
    return pattern.sub(r"\1\2\n\3) \4", text)


def strip_redundant_jna_qualifier(text: str) -> str:
    """Strip 'com.sun.jna.' prefix from Callback since it's already imported."""
    return re.sub(r":\s*com\.sun\.jna\.Callback\b", ": Callback", text)


def _build_enclosing_class_cache(text: str) -> dict[int, str]:
    """Forward scan to build (position -> innermost enclosing class) map."""
    result: dict[int, str] = {}
    scope_stack: list[tuple[str, int]] = []  # (class_name, open_brace_depth)
    depth = 0
    in_string = False
    in_line_comment = False
    in_block_comment = False
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if in_line_comment:
            if ch == '\n':
                in_line_comment = False
            i += 1
            continue
        if in_block_comment:
            if ch == '*' and i + 1 < n and text[i + 1] == '/':
                in_block_comment = False
                i += 2
                continue
            i += 1
            continue
        if in_string:
            if ch == '\\':
                i += 2
                continue
            if ch == '"':
                in_string = False
            i += 1
            continue
        if ch == '/' and i + 1 < n:
            if text[i + 1] == '/':
                in_line_comment = True
                i += 2
                continue
            if text[i + 1] == '*':
                in_block_comment = True
                i += 2
                continue
        if ch == '"':
            in_string = True
            i += 1
            continue
        if ch == '{':
            depth += 1
            # Check if preceding text declares a class/object
            line_start = text.rfind('\n', 0, i)
            if line_start == -1:
                line_start = 0
            preceding = text[line_start:i]
            m = re.search(r"\b(class|object)\s+(\w+)", preceding)
            if m:
                scope_stack.append((m.group(2), depth))
        elif ch == '}':
            if scope_stack and scope_stack[-1][1] == depth:
                _ = scope_stack.pop()
            depth -= 1
        else:
            # Record scope at non-brace positions too
            if scope_stack:
                result[i] = scope_stack[-1][0]
        i += 1
    return result


_find_enclosing_cache: tuple[str, dict[int, str]] | None = None


def _find_enclosing_class(text: str, pos: int) -> str | None:
    """Find the innermost class/object enclosing pos."""
    global _find_enclosing_cache
    if _find_enclosing_cache is None or _find_enclosing_cache[0] is not text:
        _find_enclosing_cache = (text, _build_enclosing_class_cache(text))
    return _find_enclosing_cache[1].get(pos)


def strip_redundant_qualifiers(text: str) -> str:
    """Strip redundant type qualifiers that are already in scope."""
    text = re.sub(r"(?<!import )\bjava\.util\.concurrent\.atomic\.AtomicLong\b", "AtomicLong", text)

    # , Structure.ByValue → , ByValue
    # Excludes "class ByValue: ..." self-declarations.
    def _replace_structure_byvalue(m: Match[str]) -> str:
        line_start = text.rfind("\n", 0, m.start()) + 1
        line = text[line_start:m.end()]
        if re.match(r"\s*class ByValue\b", line):
            return m.group(0)
        return ", ByValue"

    text = re.sub(r",\s*Structure\.ByValue", _replace_structure_byvalue, text)

    # RustBuffer.ByValue → ByValue (only inside RustBuffer class/object scope)
    def _replace_rustbuffer(m: Match[str]) -> str:
        cls = _find_enclosing_class(text, m.start())
        if cls == "RustBuffer":
            return "ByValue"
        return m.group(0)

    text = re.sub(r"\bRustBuffer\.ByValue\b", _replace_rustbuffer, text)

    # UniffiRustCallStatus.ByValue → ByValue (only inside UniffiRustCallStatus scope)
    def _replace_callstatus(m: Match[str]) -> str:
        cls = _find_enclosing_class(text, m.start())
        if cls == "UniffiRustCallStatus":
            return "ByValue"
        return m.group(0)

    text = re.sub(r"\bUniffiRustCallStatus\.ByValue\b", _replace_callstatus, text)

    return text


def strip_explicit_type_args(text: str) -> str:
    """Replace List<T>(len) → List(len) for collection constructors."""
    return re.sub(r"\b(List|Set|Map)<[^>]+>\(", lambda m: m.group(1) + "(", text)


def strip_trailing_commas(text: str) -> str:
    """Remove trailing commas in both single-line and multi-line contexts.

    Skips the @file:Suppress header to avoid stripping its syntactically
    required trailing comma.
    """
    sentinel = '"RedundantLambdaArrow"'
    idx = text.find(sentinel)
    body_start = 0
    if idx != -1:
        close = text.find(")", idx)
        if close != -1:
            body_start = close + 1
            while body_start < len(text) and text[body_start] in " \t\r\n":
                body_start += 1
    head = text[:body_start]
    body = text[body_start:]
    # Multi-line: comma + newline(s) + close-paren
    body = re.sub(r",(\s*\n\s*)\)", r"\1)", body)
    body = re.sub(r",(\s*\n\s*)\}", r"\1}", body)
    # Single-line: comma + optional whitespace + close-paren (same line)
    body = re.sub(r",(\s*)\)", r"\1)", body)
    return head + body


def fix_comment_grammar(text: str) -> str:
    """Fix grammar issues in comments."""
    text = text.replace("unittested", "unit-tested")
    text = text.replace("re-useable", "reusable")
    text = text.replace("until we the UTF-8", "until we know the UTF-8")
    text = re.sub(r"\betc\b(?!\.)", "etc.", text)
    # "rust code" / "rustbuffer" → "Rust code" / "RustBuffer" (programming language capitalization)
    text = re.sub(r"\brust code\b", "Rust code", text)
    text = re.sub(r"\brustbuffer\b", "RustBuffer", text)
    # "an Kotlin" → "a Kotlin" (consonant sound)
    text = re.sub(r"\ban Kotlin\b", "a Kotlin", text)
    # "a FFI" → "an FFI" (vowel sound "eff")
    text = re.sub(r"\ba FFI\b", "an FFI", text)
    text = text.replace("actually used", "used")
    return text


def transform(text: str) -> str:
    text = strip_public_modifier(text)
    text = strip_unit_return_type(text)
    text = strip_kotlin_qualifier(text)
    text = strip_redundant_backticks(text)
    text = strip_redundant_unit_expressions(text)
    text = simplify_zero_arg_lambdas(text)
    text = replace_enum_values_with_entries(text)
    text = strip_trailing_commas(text)
    text = replace_handlemap_put(text)
    text = replace_handlemap_get(text)
    text = replace_map_sum_with_sumof(text)
    text = strip_explicit_type_args(text)
    text = add_operator_to_get(text)
    text = move_lambda_out_of_error_call(text)
    text = strip_redundant_jna_qualifier(text)
    text = strip_redundant_qualifiers(text)
    text = fix_comment_grammar(text)
    return text


def strip_existing_header(text: str) -> str:
    """Strip existing @file:Suppress header and autogenerated banner.

    Handles the case where ) appears inside string literals in the Suppress block.
    """
    # Strip autogenerated banner
    text = re.sub(
        r"// This file was autogenerated by.*?Trust me, you don't want to mess with it!\s*\n+\n*",
        "",
        text,
        count=1,
        flags=re.DOTALL,
    )

    # Strip @file:Suppress(...) by finding matching closing paren
    m = re.match(r"@file:Suppress\(", text)
    if m:
        depth = 1
        i = m.end()
        while i < len(text) and depth > 0:
            if text[i] == "(":
                depth += 1
            elif text[i] == ")":
                depth -= 1
            i += 1
        # Skip trailing whitespace/newlines after the closing paren
        while i < len(text) and text[i] in " \t\r\n":
            i += 1
        text = text[i:]

    return text


def main() -> int:
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_PATH
    if not path.exists():
        print(f"skipping: {path} does not exist (regen not yet run?)")
        return 0
    original = path.read_text()
    text = original

    # Strip any existing @file:Suppress header + autogenerated banner.
    text = strip_existing_header(text)

    # Apply safe code-level transforms.
    text = transform(text)

    # Prepend the suppress header.
    text = SUPPRESS_HEADER + text

    if text == original:
        print(f"no changes applied to {path}")
        return 0

    _ = path.write_text(text)
    print(f"wrote {len(text) - len(original):+d} bytes to {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

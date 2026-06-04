#!/usr/bin/env python3
"""
Apply safe in-place transforms to the UniFFI-generated Kotlin bindings and
prepend a comprehensive @file:Suppress header.

The transforms clean up purely cosmetic issues that the IDE flags but which
are safe to apply (they don't change semantics or break the FFI ABI). The
generator will re-emit the un-fixed version on the next regen, so the same
script is also re-applied by the Gradle post-regen hook (see
cleanUniffiBindings in build.gradle.kts).

Usage:
    clean_uniffi_bindings.py [PATH]

If PATH is omitted, falls back to the in-repo default so the script can be
run by hand after a manual regen.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

DEFAULT_PATH = Path(
    "/home/pater/connected/android/app/src/main/kotlin/uniffi/connected_ffi/connected_ffi.kt"
)

# The big @file:Suppress header. Covers everything that can't be fixed
# in-place (FFI symbol names, unused FFI stubs, false-positive spell-checker
# hits on "uniffi", class-naming conventions, etc.).
SUPPRESS_HEADER = """@file:Suppress(
    // Compiler warnings
    "NAME_SHADOWING",
    "UNUSED_EXPRESSION",
    "RedundantVisibilityModifier",
    "RedundantUnitReturnType",
    "RedundantModalityModifier",
    "unused",

    // IntelliJ inspections (apply to detached file scope)
    "RemoveRedundantBackticks",
    "RedundantQualifierName",
    "RedundantComma",
    "RedundantLambdaArrow",
    "LiftReturnOrAssignment",
    "ExplicitTypeArgumentsCanBeInferred",
    "CollectionAddAllCanBeReplacedWithConstructor",
    "ReplaceWithOperatorAssignment",
    "LambdaArgumentCouldBeLastArgument",
    "UsePropertyAccessSyntax",
    "UnnecessaryParenthesisBeforeTrailingLambda",
    "RemoveEmptyPrimaryConstructor",
    "SpellCheckingInspection",
    "GrammaticalInspection",
    "UnnecessaryExplicitTypeArguments",
    "UNUSED_VARIABLE",
    "UNUSED_PARAMETER",
    "UnusedReturnValue",
    "PrivatePropertyName",
    "ProtectedInFinal",
    "ClassName",
    "FunctionName",
    "PropertyName",
    "LocalVariableName",
    "ConstructorParameterNaming",
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
        r"^public\s+(?=(?:abstract\s+|open\s+|final\s+|sealed\s+|data\s+|"
        r"enum\s+|inner\s+|override\s+|suspend\s+|inline\s+|operator\s+|"
        r"infix\s+|private\s+|internal\s+|protected\s+|public\s+|"
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
    pattern = re.compile(r"^[ \t]+Unit\s*$", re.MULTILINE)
    return pattern.sub("", text)


def simplify_zero_arg_lambdas(text: str) -> str:
    """`{ -> ... }` lambdas -> `{ ... }`.

    Only matches the simple empty-parameter case (`{ ->` followed by code,
    with `->` being the very first non-space content inside the braces).
    """
    # Walk the text and rewrite, being careful with nested braces.
    out: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == "{":
            # Look ahead for ` ->` (whitespace then arrow).
            j = i + 1
            # Skip whitespace inside.
            k = j
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


def strip_trailing_commas_in_constructors(text: str) -> str:
    """Remove `,)` (closing paren immediately after a comma) at end of lines.

    Also handles `, ,)` (double comma followed by close) which appears in
    some of the generator's nested constructor calls.

    Skips the leading @file:Suppress header, whose own argument list happens
    to contain a `,\n)` (a syntactically required trailing comma) that must
    not be stripped.
    """
    # If the big header is already present, skip past it.
    body_start = 0
    sentinel = "RemoveRedundantBackticks"
    idx = text.find(sentinel)
    if idx != -1:
        # Walk to the closing `)` of the header, then past the blank line(s).
        body_start = text.find(")", idx)
        if body_start != -1:
            body_start += 1
            # Skip trailing whitespace/newlines.
            while body_start < len(text) and text[body_start] in " \t\r\n":
                body_start += 1
    head = text[:body_start]
    body = text[body_start:]
    # First: `, ,` followed by `)` -> `,`
    body = re.sub(r",\s*,\s*\)", ", )", body)
    # `,\n)` -> `\n)` when the comma is the last non-ws char on a line.
    body = re.sub(r",(\s*\n\s*)\)", r"\1)", body)
    return head + body


def replace_header(text: str) -> str:
    """Strip the generator's small @file:Suppress + banner, prepend the big one.

    Idempotent: if the big header is already present (detected by the presence
    of two unique inspection names that only appear in it), do nothing.
    """
    if "RemoveRedundantBackticks" in text and "SpellCheckingInspection" in text:
        return text
    small_header = re.compile(r"@file:Suppress\([^)]*\)\s*\n\s*\n?")
    text = small_header.sub("", text, count=1)
    banner = re.compile(
        r"// This file was autogenerated by.*?Trust me, you don't want to mess with it!\s*\n+\n*",
        re.DOTALL,
    )
    text = banner.sub("", text, count=1)
    return SUPPRESS_HEADER + text


def main() -> int:
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_PATH
    if not path.exists():
        print(f"skipping: {path} does not exist (regen not yet run?)")
        return 0
    original = path.read_text()
    text = original

    text = strip_public_modifier(text)
    text = strip_unit_return_type(text)
    text = strip_kotlin_qualifier(text)
    text = strip_redundant_backticks(text)
    text = strip_redundant_unit_expressions(text)
    text = simplify_zero_arg_lambdas(text)
    text = strip_trailing_commas_in_constructors(text)
    text = replace_header(text)

    if text == original:
        print(f"no changes applied to {path}")
        return 0

    path.write_text(text)
    print(f"wrote {len(text) - len(original):+d} bytes to {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

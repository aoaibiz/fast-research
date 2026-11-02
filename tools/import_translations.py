#!/usr/bin/env python3
"""Generate src/i18n.rs from the SearXNG gettext catalogs.

The Rust port does not use GNU gettext at runtime; instead this script bakes the
subset of the upstream catalogs (searx/translations/*/LC_MESSAGES/messages.po)
that corresponds to the English UI strings used by the templates in this
repository into static Rust tables.

Usage:
    python3 tools/import_translations.py /path/to/searxng [templates_dir] [output.rs]

The script
  1. extracts every translatable string from <templates_dir>/*.html (text nodes
     outside script/style elements plus the placeholder, title, aria-label, alt
     and value attributes),
  2. adds a small list of UI strings that are produced by the Rust code instead
     of the templates (see EXTRA_MSGIDS),
  3. keeps the msgids that at least one upstream catalog translates as the
     reference key set,
  4. imports every locale under searx/translations/ that translates at least one
     of those msgids (locales with zero coverage are skipped; the source locale
     'en' is always kept), and
  5. writes src/i18n.rs, with every catalog using exactly the reference keys.

A locale without a translation for a msgid repeats the English msgid, which
matches the runtime fallback of Translator::gettext().

Upstream catalogs contain no msgid_plural entries at all, so no plural
translations can be imported; the generated ngettext therefore selects the
singleton translation for the singular form and falls back to the English plural
for the remaining forms.  The plural-form selection rules (the Plural-Forms
headers) are imported verbatim.
"""
import html
import re
import sys
from pathlib import Path

# The source language.  Its catalog repeats the reference msgids and still
# appears in available_locales().
SOURCE_LOCALE = "en"

# UI strings that the Rust code renders (they do not appear in the templates):
# src/web/mod.rs (time range options) and src/web/handlers.rs (safesearch /
# method options, stats page).
EXTRA_MSGIDS = [
    "Any time",
    "Past day",
    "Past week",
    "Past month",
    "Past year",
    "Off",
    "Moderate",
    "Strict",
    "POST",
    "GET",
    "Engine requests",
    "Engine errors",
    "Engine timeouts",
    "Results served",
    "Cache hits",
    "Cache entries",
    "Searches",
    "Uptime",
    "No engine has been used yet.",
    "Back to the search page",
]

# UI strings of the bundled plugins and answerers (the Lead renders the plugin
# name / description on the preferences page).
EXTRA_MSGIDS += [
    "Unit converter plugin",
    "Convert between units",
    "Ahmia blacklist",
    "Filter out onion results that appear in Ahmia's blacklist.",
    "Tor check plugin",
    "This plugin checks if the address of the request is a Tor exit-node, and"
    " informs the user if it is; like check.torproject.org, but from SearXNG.",
    "Infinite scroll",
    "Automatically loads the next page when scrolling to bottom of the current page",
    "Could not download the list of Tor exit-nodes from",
    "You are using Tor and it looks like you have the external IP address",
    "You are not using Tor and you have the external IP address",
    "Generate different random values",
    "Compute {func} of the arguments",
]

# Our templates are a rewrite, so a few UI strings differ from the upstream
# wording while meaning the same thing.  Map them onto the upstream msgid so the
# translation can be reused (the mapping is by hand, one entry per string).
ALIASES = {
    "Safe search": "SafeSearch",
    "Interface locale": "Interface language",
    "Search method": "HTTP Method",
    "Proxy image results": "Proxy image results through SearXNG",
    "Reset to defaults": "Reset defaults",
    "Any time": "Anytime",
    "Past day": "Last day",
    "Past week": "Last week",
    "Past month": "Last month",
    "Past year": "Last year",
    "Off": "None",
    "Previous": "Previous page",
    "Next": "Next page",
}

# (singular, plural) pairs that ngettext is used with.  Only the singular form
# exists in the upstream catalogs, see the module docstring.
PLURAL_PAIRS = [
    ("Engine", "Engines"),
    ("Time", "Times"),
]


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def unquote(value: str) -> str:
    """Decode a (possibly multi-line) PO string literal fragment."""
    value = value.strip()
    if len(value) >= 2 and value.startswith('"') and value.endswith('"'):
        value = value[1:-1]
    out = []
    index = 0
    escapes = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\", "a": "\a", "b": "\b", "f": "\f", "v": "\v"}
    while index < len(value):
        char = value[index]
        if char == "\\" and index + 1 < len(value):
            out.append(escapes.get(value[index + 1], value[index + 1]))
            index += 2
        else:
            out.append(char)
            index += 1
    return "".join(out)


def parse_po(path: Path) -> dict:
    """Return {msgid: msgstr} for the non-fuzzy translations of one catalog."""
    entries = {}
    for block in re.split(r"\n\s*\n", read_text(path)):
        if "msgid" not in block:
            continue
        if any(line.startswith("#,") and "fuzzy" in line for line in block.splitlines()):
            continue
        msgid = None
        msgstr = None
        current = None
        for line in block.splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("msgid_plural"):
                current = "plural"
                continue
            if line.startswith("msgid "):
                current = "msgid"
                msgid = unquote(line[len("msgid "):])
                continue
            if line.startswith("msgstr["):
                current = "plural_str"
                continue
            if line.startswith("msgstr "):
                current = "msgstr"
                msgstr = unquote(line[len("msgstr "):])
                continue
            if line.startswith('"'):
                if current == "msgid":
                    msgid += unquote(line)
                elif current == "msgstr":
                    msgstr += unquote(line)
                continue
        if msgid and msgstr:
            entries[msgid] = msgstr
    return entries


def plural_header(path: Path):
    """Return (nplurals, expression) from a catalog header, or None."""
    if not path.is_file():
        return None
    header = ""
    for line in read_text(path).splitlines():
        line = line.strip()
        if line.startswith('"') and line.endswith('"'):
            header += unquote(line)
    match = re.search(r"Plural-Forms:\s*([^\n]+)", header)
    if not match:
        return None
    value = match.group(1).strip().rstrip(";")
    nplurals = re.search(r"nplurals=(\d+)", value)
    if not nplurals:
        return None
    expression = value.split("plural=", 1)[1] if "plural=" in value else "0"
    return int(nplurals.group(1)), expression


# Tokens of the C plural expressions in the PO headers.  The expressions only
# use n, integer literals, comparisons, &&, ||, !, %, parentheses and the
# ternary operator.
PLURAL_TOKEN_RE = re.compile(r"\d+|[A-Za-z_]\w*|&&|\|\||==|!=|<=|>=|[?:!()%<>+\-*/]")


class PluralParser:
    """Recursive-descent parser for one Plural-Forms expression."""

    def __init__(self, text: str):
        self.tokens = PLURAL_TOKEN_RE.findall(text)
        self.index = 0

    def peek(self):
        if self.index < len(self.tokens):
            return self.tokens[self.index]
        return None

    def take(self, expected=None):
        token = self.peek()
        if expected is not None and token != expected:
            raise ValueError("expected %r, got %r" % (expected, token))
        self.index += 1
        return token

    def parse(self):
        node = self.ternary()
        if self.peek() is not None:
            raise ValueError("trailing tokens: %r" % (self.tokens[self.index:],))
        return node

    def ternary(self):
        condition = self.disjunction()
        if self.peek() == "?":
            self.take("?")
            yes = self.ternary()
            self.take(":")
            no = self.ternary()
            return ("?", condition, yes, no)
        return condition

    def disjunction(self):
        node = self.conjunction()
        while self.peek() == "||":
            self.take()
            node = ("binary", "||", node, self.conjunction())
        return node

    def conjunction(self):
        node = self.equality()
        while self.peek() == "&&":
            self.take()
            node = ("binary", "&&", node, self.equality())
        return node

    def equality(self):
        node = self.relational()
        while self.peek() in ("==", "!="):
            operator = self.take()
            node = ("binary", operator, node, self.relational())
        return node

    def relational(self):
        node = self.additive()
        while self.peek() in ("<", ">", "<=", ">="):
            operator = self.take()
            node = ("binary", operator, node, self.additive())
        return node

    def additive(self):
        node = self.multiplicative()
        while self.peek() in ("+", "-"):
            operator = self.take()
            node = ("binary", operator, node, self.multiplicative())
        return node

    def multiplicative(self):
        node = self.unary()
        while self.peek() in ("*", "/", "%"):
            operator = self.take()
            node = ("binary", operator, node, self.unary())
        return node

    def unary(self):
        if self.peek() == "!":
            self.take()
            return ("not", self.unary())
        return self.primary()

    def primary(self):
        token = self.peek()
        if token == "(":
            self.take()
            node = self.ternary()
            self.take(")")
            return node
        if token == "n":
            self.take()
            return ("n",)
        if token is not None and token.isdigit():
            self.take()
            return ("number", int(token))
        raise ValueError("unexpected token %r" % (token,))


def render_plural(node) -> str:
    """Render a parsed plural expression as a Rust expression."""
    kind = node[0]
    if kind == "number":
        return str(node[1])
    if kind == "n":
        return "n"
    if kind == "not":
        return "!(%s)" % render_plural(node[1])
    if kind == "binary":
        return "(%s %s %s)" % (render_plural(node[2]), node[1], render_plural(node[3]))
    if kind == "?":
        return "if %s { %s } else { %s }" % (
            render_condition(node[1]),
            render_plural(node[2]),
            render_plural(node[3]),
        )
    raise ValueError("unknown plural node %r" % (kind,))


def render_condition(node) -> str:
    """Render a boolean plural expression without a redundant outer pair."""
    rendered = render_plural(node)
    if rendered.startswith("(") and rendered.endswith(")"):
        return rendered[1:-1]
    return rendered


def plural_body(path: Path) -> str:
    """The body of one plural_index() match arm."""
    parsed = plural_header(path)
    if parsed is None:
        return "if n == 1 { 0 } else { 1 }"
    nplurals, expression = parsed
    if nplurals == 1:
        return "0"
    node = PluralParser(expression).parse()
    if node[0] == "?":
        return render_plural(node)
    if node[0] == "number":
        return str(node[1])
    # A bare condition is the gettext shorthand for "index 1 when true".
    return "if %s { 1 } else { 0 }" % render_condition(node)


def template_strings(directory: Path) -> list:
    """Translatable strings of the askama templates, in template order."""
    seen = {}
    for path in sorted(directory.glob("*.html")):
        text = read_text(path)
        text = re.sub(r"\{#.*?#\}", "", text, flags=re.S)
        text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
        text = re.sub(r"<(script|style)\b.*?</\1>", "", text, flags=re.S | re.I)
        text = re.sub(r"\{\{.*?\}\}", " ", text, flags=re.S)
        text = re.sub(r"\{%.*?%\}", " ", text, flags=re.S)
        for attr in re.finditer(r'(?:placeholder|title|aria-label|alt)="([^"]+)"', text):
            add(seen, html.unescape(attr.group(1)))
        for chunk in re.split(r"<[^>]*>", text):
            add(seen, html.unescape(chunk))
    return list(seen)


# Characters that decorate a string but are not part of the message
# (em dashes, arrows, quote marks, ...).  Full stops / colons / question marks
# are kept because they are part of the message.
DECORATION = set("\u2014\u2013-\u00b7\u2022\u2190\u2192\u00ab\u00bb\"'\u201c\u201d\u2018\u2019\u00a0|/\\,;")


def clean(raw: str) -> str:
    """Collapse whitespace and drop decorative punctuation around the string."""
    value = " ".join(raw.split())
    value = re.sub(r"\s*\(\s*\)\s*$", "", value)
    while value and (value[0] in DECORATION or value[0] in "()"):
        value = value[1:].lstrip()
    while value and value[-1] in DECORATION:
        value = value[:-1].rstrip()
    return value


def add(seen: dict, raw: str) -> None:
    value = clean(raw)
    if not value:
        return
    if not any(char.isalpha() for char in value):
        return
    if value.startswith("{") or value.startswith("%"):
        return
    seen[value] = True


def lookup(catalog: dict, msgid: str) -> str | None:
    """Exact match, then a unique case-insensitive match, then the msgid without
    its final punctuation (upstream templates often add a full stop or colon)."""
    if msgid in catalog:
        return catalog[msgid]
    aliased = ALIASES.get(msgid)
    if aliased is not None and aliased in catalog:
        return catalog[aliased]
    lowered = msgid.lower()
    matches = [value for key, value in catalog.items() if key.lower() == lowered]
    if len(set(matches)) == 1 and matches:
        return matches[0]
    stripped = msgid.rstrip(".:?!... ")
    if stripped != msgid:
        matches = [value for key, value in catalog.items() if key.rstrip(".:?!... ") == stripped]
        if len(set(matches)) == 1 and matches:
            return matches[0]
    return None


def rust_string(value: str) -> str:
    out = ['"']
    for char in value:
        if char == "\\":
            out.append("\\\\")
        elif char == '"':
            out.append('\\"')
        elif char == "\n":
            out.append("\\n")
        elif char == "\r":
            out.append("\\r")
        elif char == "\t":
            out.append("\\t")
        elif ord(char) < 0x20 or ord(char) == 0x7F:
            out.append("\\u{%x}" % ord(char))
        else:
            out.append(char)
    out.append('"')
    return "".join(out)


def const_name(locale: str) -> str:
    return "MESSAGES_%s" % re.sub(r"\W", "_", locale).upper()


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    upstream = Path(sys.argv[1])
    templates = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("templates")
    output = Path(sys.argv[3]) if len(sys.argv) > 3 else Path("src/i18n.rs")

    candidates = template_strings(templates)
    for extra in EXTRA_MSGIDS:
        if extra not in candidates:
            candidates.append(extra)

    translations = upstream / "searx" / "translations"
    catalog_paths = {}
    catalogs = {}
    for po in sorted(translations.glob("*/LC_MESSAGES/messages.po")):
        locale = po.parent.parent.name
        catalog_paths[locale] = po
        catalogs[locale] = parse_po(po)
    if not catalogs:
        print("error: no catalogs under %s" % translations, file=sys.stderr)
        return 1
    if SOURCE_LOCALE not in catalogs:
        print("warning: no catalog for %s" % SOURCE_LOCALE, file=sys.stderr)
        catalog_paths[SOURCE_LOCALE] = translations / SOURCE_LOCALE / "LC_MESSAGES" / "messages.po"
        catalogs[SOURCE_LOCALE] = {}

    # The reference key set: every UI msgid that at least one catalog translates.
    msgids = [
        msgid
        for msgid in candidates
        if any(lookup(catalog, msgid) is not None for catalog in catalogs.values())
    ]

    shipped = []
    for locale in sorted(catalogs):
        catalog = catalogs[locale]
        translated = sum(1 for msgid in msgids if lookup(catalog, msgid) not in (None, msgid))
        if locale == SOURCE_LOCALE or translated:
            shipped.append(locale)
        else:
            print("skipping %s (no translations for the UI msgids)" % locale, file=sys.stderr)

    lines = []
    lines.append("//! gettext-style message catalog, ported from searx/translations/*/LC_MESSAGES/messages.po.")
    lines.append("//!")
    lines.append("//! Generated by tools/import_translations.py; do not edit by hand.")
    lines.append("//!")
    lines.append("//! Only strings used by templates/*.html (plus a handful of UI strings the")
    lines.append("//! Rust code renders, see the generator) are kept.  Every catalog uses the")
    lines.append("//! same msgid keys; a locale without a translation repeats the English msgid,")
    lines.append("//! and a locale the instance does not ship falls back to 'en'.")
    lines.append("")
    lines.append("/// The translator of one UI locale.")
    lines.append("#[derive(Debug, Clone, Copy, PartialEq, Eq)]")
    lines.append("pub struct Translator {")
    lines.append("    locale: &'static str,")
    lines.append("}")
    lines.append("")
    lines.append("/// The locales shipped with the port (the source locale 'en' included).")
    lines.append("#[rustfmt::skip]")
    lines.append("static SHIPPED_LOCALES: &[&str] = &[")
    lines.append("    " + ", ".join('"%s"' % locale for locale in shipped) + ",")
    lines.append("];")
    lines.append("")

    stats = {}
    for locale in shipped:
        catalog = catalogs[locale]
        pairs = []
        translated = 0
        for msgid in msgids:
            value = lookup(catalog, msgid)
            if value is None:
                value = msgid
            elif value != msgid:
                translated += 1
            pairs.append((msgid, value))
        stats[locale] = translated
        if locale == SOURCE_LOCALE:
            lines.append("/// The English source catalog (every msgid maps to itself).")
        else:
            lines.append("/// Translations of %s." % locale)
        lines.append("#[rustfmt::skip]")
        lines.append("static %s: &[(&str, &str)] = &[" % const_name(locale))
        for msgid, value in pairs:
            lines.append("    (%s, %s)," % (rust_string(msgid), rust_string(value)))
        lines.append("];")
        lines.append("")

    lines.append("struct Catalog {")
    lines.append("    locale: &'static str,")
    lines.append("    messages: &'static [(&'static str, &'static str)],")
    lines.append("}")
    lines.append("")
    lines.append("#[rustfmt::skip]")
    lines.append("static CATALOGS: &[Catalog] = &[")
    for locale in shipped:
        lines.append('    Catalog { locale: "%s", messages: %s },' % (locale, const_name(locale)))
    lines.append("];")
    lines.append("")
    lines.append("/// Plural form selection, copied from the Plural-Forms header of each")
    lines.append("/// upstream catalog.")
    lines.append("#[rustfmt::skip]")
    lines.append("#[allow(unused_comparisons)] // the rules are copied verbatim from upstream")
    lines.append("fn plural_index(locale: &str, n: u64) -> usize {")
    lines.append("    match locale {")
    for locale in shipped:
        lines.append('        "%s" => %s,' % (locale, plural_body(catalog_paths[locale])))
    lines.append("        _ => if n == 1 { 0 } else { 1 },")
    lines.append("    }")
    lines.append("}")
    lines.append("")
    lines.append("/// The (singular, plural) pairs the UI uses with ngettext.")
    pairs = ", ".join(
        "(%s, %s)" % (rust_string(singular), rust_string(plural))
        for singular, plural in PLURAL_PAIRS
    )
    lines.append("pub static NGETTEXT_PAIRS: &[(&str, &str)] = &[%s];" % pairs)
    lines.append("")
    lines.append("/// Plural form translations.  The upstream catalogs contain no msgid_plural")
    lines.append("/// entries, so this table stays empty and ngettext falls back to the English")
    lines.append("/// plural for every form except the singular (which uses the catalog entry).")
    lines.append("fn plural_forms(_locale: &str, _msgid: &str) -> Option<&'static [&'static str]> {")
    lines.append("    None")
    lines.append("}")
    lines.append("")
    lines.append("/// The catalog of a locale, or 'en' for an unknown one.")
    lines.append("pub fn for_locale(locale: &str) -> Translator {")
    lines.append("    Translator {")
    lines.append("        locale: match_locale(locale),")
    lines.append("    }")
    lines.append("}")
    lines.append("")
    lines.append("/// Resolve a (possibly regional) locale tag onto a shipped locale.")
    lines.append("fn match_locale(locale: &str) -> &'static str {")
    lines.append("    let normalized = locale.trim().replace('-', \"_\");")
    lines.append("    if normalized.is_empty() {")
    lines.append("        return \"en\";")
    lines.append("    }")
    lines.append("    for candidate in SHIPPED_LOCALES {")
    lines.append("        if candidate.eq_ignore_ascii_case(&normalized) {")
    lines.append("            return candidate;")
    lines.append("        }")
    lines.append("    }")
    lines.append("    let language = normalized.split('_').next().unwrap_or(\"\");")
    lines.append("    if language.eq_ignore_ascii_case(\"zh\") {")
    lines.append("        let lower = normalized.to_ascii_lowercase();")
    lines.append("        if lower.starts_with(\"zh_hant\")")
    lines.append("            || lower.starts_with(\"zh_tw\")")
    lines.append("            || lower.starts_with(\"zh_hk\")")
    lines.append("            || lower.starts_with(\"zh_mo\")")
    lines.append("        {")
    lines.append("            return \"zh_Hant_TW\";")
    lines.append("        }")
    lines.append("        return \"zh_Hans_CN\";")
    lines.append("    }")
    lines.append("    for candidate in SHIPPED_LOCALES {")
    lines.append("        if candidate.eq_ignore_ascii_case(language) {")
    lines.append("            return candidate;")
    lines.append("        }")
    lines.append("    }")
    lines.append("    \"en\"")
    lines.append("}")
    lines.append("")
    lines.append("/// The locales the port ships translations for.")
    lines.append("pub fn available_locales() -> &'static [&'static str] {")
    lines.append("    SHIPPED_LOCALES")
    lines.append("}")
    lines.append("")
    lines.append("impl Translator {")
    lines.append("    /// The resolved locale of this translator.")
    lines.append("    pub fn locale(&self) -> &'static str {")
    lines.append("        self.locale")
    lines.append("    }")
    lines.append("")
    lines.append("    fn catalog(&self) -> Option<&'static Catalog> {")
    lines.append("        CATALOGS")
    lines.append("            .iter()")
    lines.append("            .find(|catalog| catalog.locale == self.locale)")
    lines.append("    }")
    lines.append("")
    lines.append("    /// The translation of msgid, or msgid itself when the catalog has none.")
    lines.append("    pub fn gettext<'a>(&'a self, msgid: &'a str) -> &'a str {")
    lines.append("        match self.catalog() {")
    lines.append("            Some(catalog) => catalog")
    lines.append("                .messages")
    lines.append("                .iter()")
    lines.append("                .find(|(key, _)| *key == msgid)")
    lines.append("                .map(|(_, value)| *value)")
    lines.append("                .unwrap_or(msgid),")
    lines.append("            None => msgid,")
    lines.append("        }")
    lines.append("    }")
    lines.append("")
    lines.append("    /// The singular or plural translation of singular / plural for n.")
    lines.append("    pub fn ngettext<'a>(&'a self, singular: &'a str, plural: &'a str, n: u64) -> &'a str {")
    lines.append("        let index = plural_index(self.locale, n);")
    lines.append("        if index == 0 {")
    lines.append("            return self.gettext(singular);")
    lines.append("        }")
    lines.append("        if let Some(forms) = plural_forms(self.locale, singular) {")
    lines.append("            if let Some(form) = forms.get(index) {")
    lines.append("                if !form.is_empty() {")
    lines.append("                    return form;")
    lines.append("                }")
    lines.append("            }")
    lines.append("        }")
    lines.append("        plural")
    lines.append("    }")
    lines.append("}")
    lines.append("")
    lines.append("#[cfg(test)]")
    lines.append("mod tests {")
    lines.append("    use super::*;")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn english_is_the_fallback_for_unknown_locales() {")
    lines.append("        let translator = for_locale(\"xx\");")
    lines.append("        assert_eq!(translator.locale(), \"en\");")
    lines.append("        assert_eq!(translator.gettext(\"Preferences\"), \"Preferences\");")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn regional_tags_resolve_onto_a_shipped_locale() {")
    lines.append("        assert_eq!(for_locale(\"de-DE\").locale(), \"de\");")
    lines.append("        assert_eq!(for_locale(\"pt-BR\").locale(), \"pt_BR\");")
    lines.append("        assert_eq!(for_locale(\"zh-CN\").locale(), \"zh_Hans_CN\");")
    lines.append("        assert_eq!(for_locale(\"zh-TW\").locale(), \"zh_Hant_TW\");")
    lines.append("        assert_eq!(for_locale(\"zh-Hant\").locale(), \"zh_Hant_TW\");")
    lines.append("        assert_eq!(for_locale(\"ja_JP\").locale(), \"ja\");")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn available_locales_contain_the_source_locale() {")
    lines.append("        let locales = available_locales();")
    lines.append("        assert!(locales.contains(&\"en\"));")
    lines.append("        assert!(locales.contains(&\"ja\"));")
    lines.append("        assert!(locales.contains(&\"de\"));")
    lines.append("        assert!(locales.contains(&\"zh_Hans_CN\"));")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn every_shipped_locale_resolves_to_itself() {")
    lines.append("        for locale in available_locales() {")
    lines.append("            assert_eq!(for_locale(locale).locale(), *locale);")
    lines.append("        }")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn every_catalog_uses_the_english_key_set() {")
    lines.append("        let reference: Vec<&str> = MESSAGES_EN.iter().map(|(key, _)| *key).collect();")
    lines.append("        for catalog in CATALOGS {")
    lines.append("            let keys: Vec<&str> = catalog.messages.iter().map(|(key, _)| *key).collect();")
    lines.append("            assert_eq!(")
    lines.append("                keys, reference,")
    lines.append("                \"catalog {} does not use the reference msgids\",")
    lines.append("                catalog.locale")
    lines.append("            );")
    lines.append("        }")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn ngettext_uses_the_english_plural_for_the_plural_form() {")
    lines.append("        let translator = for_locale(\"en\");")
    lines.append("        assert_eq!(translator.ngettext(\"Engine\", \"Engines\", 1), \"Engine\");")
    lines.append("        assert_eq!(translator.ngettext(\"Engine\", \"Engines\", 2), \"Engines\");")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn ngettext_follows_the_locale_plural_rule() {")
    lines.append("        // Japanese has nplurals=1: every count uses the singular form.")
    lines.append("        let japanese = for_locale(\"ja\");")
    lines.append("        let singular = japanese.gettext(\"Engine\");")
    lines.append("        assert_eq!(japanese.ngettext(\"Engine\", \"Engines\", 1), singular);")
    lines.append("        assert_eq!(japanese.ngettext(\"Engine\", \"Engines\", 5), singular);")
    lines.append("        // French uses the plural for n > 1 (nplurals=2).")
    lines.append("        let french = for_locale(\"fr\");")
    lines.append("        assert_eq!(")
    lines.append("            french.ngettext(\"Engine\", \"Engines\", 1),")
    lines.append("            french.gettext(\"Engine\")")
    lines.append("        );")
    lines.append("        assert_eq!(french.ngettext(\"Engine\", \"Engines\", 2), \"Engines\");")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn plural_rules_follow_the_upstream_headers() {")
    lines.append("        assert_eq!(plural_index(\"ar\", 0), 0);")
    lines.append("        assert_eq!(plural_index(\"ar\", 2), 2);")
    lines.append("        assert_eq!(plural_index(\"ar\", 11), 4);")
    lines.append("        assert_eq!(plural_index(\"pl\", 2), 1);")
    lines.append("        assert_eq!(plural_index(\"pl\", 5), 2);")
    lines.append("        assert_eq!(plural_index(\"ro\", 0), 1);")
    lines.append("        assert_eq!(plural_index(\"ro\", 20), 2);")
    lines.append("        assert_eq!(plural_index(\"ga\", 7), 3);")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn every_ngettext_pair_is_translatable() {")
    lines.append("        let translator = for_locale(\"ja\");")
    lines.append("        for (singular, plural) in NGETTEXT_PAIRS {")
    lines.append("            let one = translator.ngettext(singular, plural, 1);")
    lines.append("            let many = translator.ngettext(singular, plural, 7);")
    lines.append("            assert!(!one.is_empty() && !many.is_empty());")
    lines.append("            // nplurals=1: both forms come from the catalog entry.")
    lines.append("            assert_eq!(one, many);")
    lines.append("        }")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn translations_come_from_the_upstream_catalogs() {")
    lines.append("        assert_eq!(for_locale(\"de\").gettext(\"Preferences\"), \"Einstellungen\");")
    lines.append("        assert_eq!(for_locale(\"de\").gettext(\"About\"), \"Über\");")
    lines.append("        assert_ne!(for_locale(\"ja\").gettext(\"Preferences\"), \"Preferences\");")
    lines.append("        // An unknown msgid is returned unchanged.")
    lines.append("        assert_eq!(for_locale(\"de\").gettext(\"no such msgid\"), \"no such msgid\");")
    lines.append("    }")
    lines.append("}")
    lines.append("")

    output.write_text("\n".join(lines), encoding="utf-8")
    print("wrote %s: %d msgids, %d locales" % (output, len(msgids), len(shipped)))
    for locale in shipped:
        print("  %-12s %4d translations" % (locale, stats[locale]))
    return 0


if __name__ == "__main__":
    sys.exit(main())

//! A minimal XML DOM for the declarative `xpath` engine.
//!
//! Why this exists: the xpath engine used to run every response through
//! `scraper`'s HTML parser. RDF/RSS feeds (JVN, JPCERT/CC, IPA) are XML, and the
//! HTML parser is wrong for them in a way that is easy to miss — `<link>` is a
//! *void* element in HTML, so `<link>https://jvn.jp/vu/1/</link>` keeps the
//! element but **drops its text**. The result set parsed fine (the `<item>`
//! elements survived), but every `./link` was empty, so the engine skipped
//! every result and reported zero.
//!
//! This module builds a tiny read-only DOM with `quick-xml` (already a
//! dependency, used by arxiv/torznab/pubmed/...), so no new crate is needed.
//! It only implements what `XPath` selection needs: child/descendant element
//! navigation, element names (with a namespace prefix kept as written),
//! attributes and text.
//!
//! Namespaces: an element keeps the **qualified** name exactly as it appears in
//! the document (`dc:date`, `rdf:RDF`). Lookups therefore match on either the
//! qualified name or the local name, so `//item` and `//rdf:item` both work for
//! `<rdf:item>` in a feed that binds the RSS 1.0 default namespace.

use quick_xml::Reader;
use quick_xml::events::Event;

/// One element of the XML tree.
#[derive(Debug, Clone)]
pub struct XmlElement {
    /// Qualified name as written in the document (`item`, `dc:date`, ...).
    pub name: String,
    /// Attributes, qualified names, unescaped values.
    pub attributes: Vec<(String, String)>,
    /// Direct text content (concatenated, already entity-decoded).
    pub text: String,
    /// Child elements.
    pub children: Vec<XmlElement>,
}

impl XmlElement {
    /// Attribute lookup by qualified name, then by local name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        if let Some((_, value)) = self.attributes.iter().find(|(key, _)| key == name) {
            return Some(value.as_str());
        }
        let local = local_name(name);
        self.attributes
            .iter()
            .find(|(key, _)| local_name(key) == local)
            .map(|(_, value)| value.as_str())
    }

    /// Whether this element matches a step tag.
    ///
    /// `*` matches everything. Otherwise the tag matches when it equals the
    /// qualified name, or when it equals the local name. That second rule is
    /// what makes `//item` work on `<rdf:item>`/RSS 1.0 feeds without the
    /// operator having to know the namespace prefix.
    pub fn matches_tag(&self, tag: &str) -> bool {
        tag == "*" || self.name == tag || local_name(&self.name) == local_name(tag)
    }

    /// All descendant elements in document order (excluding self).
    pub fn descendants(&self) -> Vec<&XmlElement> {
        let mut out = Vec::new();
        self.collect_descendants(&mut out);
        out
    }

    fn collect_descendants<'a>(&'a self, out: &mut Vec<&'a XmlElement>) {
        for child in &self.children {
            out.push(child);
            child.collect_descendants(out);
        }
    }
}

/// The local part of a qualified name (`dc:date` -> `date`).
pub fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// A parsed XML document: the children of the root element.
#[derive(Debug, Clone, Default)]
pub struct XmlDocument {
    pub roots: Vec<XmlElement>,
}

impl XmlDocument {
    /// Parse an XML/RDF document into a tree.
    ///
    /// Returns `None` when the input is not well-formed XML, so the caller can
    /// fall back to the HTML parser instead of silently producing empty results.
    pub fn parse(body: &str) -> Option<Self> {
        let mut reader = Reader::from_str(body);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = true;

        let mut roots: Vec<XmlElement> = Vec::new();
        // Open element stack; the last entry is the current parent.
        let mut stack: Vec<XmlElement> = Vec::new();

        loop {
            match reader.read_event() {
                Ok(Event::Start(start)) => {
                    stack.push(element_from_start(&start)?);
                }
                Ok(Event::Empty(start)) => {
                    push_element(&mut stack, &mut roots, element_from_start(&start)?);
                }
                Ok(Event::End(_)) => {
                    let finished = stack.pop()?;
                    push_element(&mut stack, &mut roots, finished);
                }
                Ok(Event::Text(text)) => {
                    if let Some(current) = stack.last_mut() {
                        current
                            .text
                            .push_str(&text.xml_content(quick_xml::XmlVersion::Explicit1_0).ok()?);
                    }
                }
                Ok(Event::CData(data)) => {
                    if let Some(current) = stack.last_mut() {
                        current
                            .text
                            .push_str(&String::from_utf8_lossy(&data.into_inner()));
                    }
                }
                // Entity references arrive as their own event in 0.38; resolve
                // the predefined ones and decode numeric character references.
                Ok(Event::GeneralRef(reference)) => {
                    if let Some(current) = stack.last_mut() {
                        if let Ok(Some(character)) = reference.resolve_char_ref() {
                            current.text.push(character);
                        } else if let Ok(name) = reference.decode() {
                            if let Some(value) = quick_xml::escape::resolve_predefined_entity(&name)
                            {
                                current.text.push_str(value);
                            }
                        }
                    }
                }
                // Declarations, comments, PIs and DOCTYPE carry no results.
                Ok(Event::Decl(_))
                | Ok(Event::Comment(_))
                | Ok(Event::PI(_))
                | Ok(Event::DocType(_)) => {}
                Ok(Event::Eof) => break,
                Err(_) => return None,
            }
        }

        // An unbalanced document is not XML; let the caller fall back to HTML.
        if !stack.is_empty() || roots.is_empty() {
            return None;
        }
        Some(Self { roots })
    }
}

/// Attach a finished element to its parent, or to the root list.
fn push_element(stack: &mut [XmlElement], roots: &mut Vec<XmlElement>, element: XmlElement) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(element),
        None => roots.push(element),
    }
}

/// Build an element, its name and its attributes from a start/empty tag.
///
/// A malformed tag (bad UTF-8, bad entity) makes the whole document non-XML;
/// that is reported as `None` so the caller can fall back to HTML.
fn element_from_start(start: &quick_xml::events::BytesStart<'_>) -> Option<XmlElement> {
    let name = String::from_utf8(start.name().as_ref().to_vec()).ok()?;
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.ok()?;
        let key = String::from_utf8(attribute.key.as_ref().to_vec()).ok()?;
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .ok()?
            .into_owned();
        attributes.push((key, value));
    }
    Some(XmlElement {
        name,
        attributes,
        text: String::new(),
        children: Vec::new(),
    })
}

/// Whether a response body looks like XML rather than HTML.
///
/// Detection is deliberately conservative: an XML declaration, or the
/// `Content-Type` saying XML/RSS/RDF. A body that merely contains the word
/// "xml" is not enough — engines scrape many HTML pages that mention it.
pub fn looks_like_xml(body: &str, content_type: Option<&str>) -> bool {
    if let Some(content_type) = content_type {
        let lowered = content_type.to_ascii_lowercase();
        if lowered.contains("xml") || lowered.contains("rss") || lowered.contains("rdf") {
            return true;
        }
    }
    let head = body.trim_start();
    head.starts_with("<?xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of JVN's `jvn.rdf`: RSS 1.0 with namespaced elements.
    const JVN_RDF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns="http://purl.org/rss/1.0/"
  xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
  xmlns:dc="http://purl.org/dc/elements/1.1/">
 <channel rdf:about="https://jvn.jp/">
  <title>JVN</title>
 </channel>
 <item rdf:about="https://jvn.jp/jp/JVN51223238/">
  <title>GROWI vulnerability</title>
  <link>https://jvn.jp/jp/JVN51223238/</link>
  <description>desc one</description>
  <dc:date>2026-10-27T15:00:00+09:00</dc:date>
 </item>
 <item rdf:about="https://jvn.jp/vu/JVNVU97400824/">
  <title>Brotli out-of-bounds write</title>
  <link>https://jvn.jp/vu/JVNVU97400824/</link>
  <description>desc two</description>
 </item>
</rdf:RDF>"#;

    #[test]
    fn parses_rss_rdf_into_items() {
        let document = XmlDocument::parse(JVN_RDF).expect("rdf must parse");
        let root = &document.roots[0];
        // The root keeps its namespace prefix, as written.
        assert_eq!(root.name, "rdf:RDF");
        let items: Vec<&XmlElement> = root
            .descendants()
            .into_iter()
            .filter(|element| element.matches_tag("item"))
            .collect();
        assert_eq!(items.len(), 2);
        // <link> must keep its text: this is exactly what the HTML parser lost.
        let link = items[0]
            .children
            .iter()
            .find(|child| child.name == "link")
            .expect("link element");
        assert_eq!(link.text, "https://jvn.jp/jp/JVN51223238/");
        assert_eq!(items[1].children[0].text, "Brotli out-of-bounds write");
    }

    #[test]
    fn local_name_matching_ignores_the_namespace_prefix() {
        let document = XmlDocument::parse(JVN_RDF).unwrap();
        let root = &document.roots[0];
        // //item works even though the document uses a default namespace.
        assert!(root.children.iter().any(|child| child.matches_tag("item")));
        // A prefixed tag also matches its local name.
        let dc = root
            .descendants()
            .into_iter()
            .find(|element| element.name == "dc:date")
            .unwrap();
        assert!(dc.matches_tag("dc:date"));
        assert!(dc.matches_tag("date"));
    }

    #[test]
    fn attributes_are_available_by_qualified_and_local_name() {
        let document = XmlDocument::parse(JVN_RDF).unwrap();
        let root = &document.roots[0];
        let item = root
            .children
            .iter()
            .find(|c| c.matches_tag("item"))
            .unwrap();
        assert_eq!(
            item.attr("rdf:about"),
            Some("https://jvn.jp/jp/JVN51223238/")
        );
        // Attribute lookup also falls back to the local name.
        assert_eq!(item.attr("about"), Some("https://jvn.jp/jp/JVN51223238/"));
    }

    #[test]
    fn malformed_xml_is_rejected_so_the_caller_can_fall_back() {
        // An unclosed element is not well-formed XML.
        assert!(XmlDocument::parse("<a><b>text</a>").is_none());
        // A plain HTML page is not XML either.
        assert!(XmlDocument::parse("<html><body><p>hello").is_none());
    }

    #[test]
    fn entities_are_decoded() {
        let document = XmlDocument::parse("<r><t>a &amp; b</t></r>").unwrap();
        assert_eq!(document.roots[0].children[0].text, "a & b");
    }

    #[test]
    fn detection_prefers_the_content_type_then_the_declaration() {
        assert!(looks_like_xml("<rdf:RDF/>", Some("application/rdf+xml")));
        assert!(looks_like_xml("<rss/>", Some("text/xml")));
        assert!(looks_like_xml("<?xml version=\"1.0\"?><a/>", None));
        // An HTML page that merely mentions xml is not XML.
        assert!(!looks_like_xml(
            "<!DOCTYPE html><html><body>xml</body></html>",
            Some("text/html")
        ));
    }
}

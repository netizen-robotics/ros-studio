//! Bounded XML tree for local static assets; never resolves entities or resources.
use super::{bad, limit, unsupported};
use crate::robot::{stl::cancelled, RobotError, MAX_MESH_BYTES, MAX_XML_DEPTH, MAX_XML_ELEMENTS};
use quick_xml::{events::Event, Reader};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
#[derive(Debug)]
pub(super) struct Node {
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<Node>,
    pub text: String,
}
impl Node {
    pub fn attr(&self, k: &str) -> Result<&str, RobotError> {
        self.attrs
            .get(k)
            .map(String::as_str)
            .ok_or_else(|| bad(&format!("{} requires {k}", self.tag)))
    }
    pub fn optional(&self, k: &str) -> Option<&str> {
        self.attrs.get(k).map(String::as_str)
    }
    pub fn one(&self, tag: &str) -> Result<Option<&Self>, RobotError> {
        let mut found = self.children.iter().filter(|n| n.tag == tag);
        let first = found.next();
        if found.next().is_some() {
            return Err(bad(&format!("repeated {tag}")));
        }
        Ok(first)
    }
    pub fn required(&self, tag: &str) -> Result<&Self, RobotError> {
        self.one(tag)?
            .ok_or_else(|| bad(&format!("{} requires {tag}", self.tag)))
    }
    pub fn only(&self, tags: &[&str], attrs: &[&str]) -> Result<(), RobotError> {
        if !self.text.trim().is_empty()
            || self
                .children
                .iter()
                .any(|n| !tags.contains(&n.tag.as_str()))
            || self.attrs.keys().any(|k| !attrs.contains(&k.as_str()))
        {
            return Err(unsupported(&format!(
                "unsupported {} element content/attributes",
                self.tag
            )));
        }
        Ok(())
    }
    pub fn leaf(&self, attrs: &[&str]) -> Result<&str, RobotError> {
        if !self.children.is_empty() || self.attrs.keys().any(|k| !attrs.contains(&k.as_str())) {
            return Err(bad("invalid value element"));
        }
        Ok(self.text.trim())
    }
}
fn xml_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next()
        .is_some_and(|v| v.is_ascii_alphabetic() || v == b'_')
        && b.all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'-' | b'.'))
}
fn attach(node: Node, stack: &mut [Node], root: &mut Option<Node>) -> Result<(), RobotError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.replace(node).is_some() {
        return Err(bad("multiple XML roots"));
    }
    Ok(())
}
pub(super) fn parse(bytes: &[u8], cancel: &AtomicBool) -> Result<Node, RobotError> {
    cancelled(cancel)?;
    if bytes.len() > MAX_MESH_BYTES {
        return Err(limit("mesh exceeds 16 MiB"));
    }
    let s = std::str::from_utf8(bytes).map_err(|_| bad("XML must be UTF-8"))?;
    if s.contains('&') {
        return Err(unsupported("XML entity references are unsupported"));
    }
    if s.chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(bad("invalid XML control character"));
    }
    let mut reader = Reader::from_str(s);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut ids = BTreeSet::new();
    let mut count = 0;
    let mut declared = false;
    loop {
        cancelled(cancel)?;
        let event = reader
            .read_event()
            .map_err(|e| bad(&format!("invalid XML: {e}")))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                count += 1;
                if count > MAX_XML_ELEMENTS || stack.len() + 1 > MAX_XML_DEPTH {
                    return Err(limit("XML depth/element limit exceeded"));
                }
                let tag = std::str::from_utf8(e.name().as_ref())
                    .map_err(|_| bad("invalid XML name"))?
                    .to_owned();
                if !xml_name(&tag) {
                    return Err(unsupported("prefixed namespaces or malformed XML names"));
                }
                let mut attrs = BTreeMap::new();
                for a in e.attributes() {
                    if attrs.len() == 32 {
                        return Err(limit("more than 32 XML attributes"));
                    }
                    let a = a.map_err(|_| bad("invalid or duplicate XML attribute"))?;
                    let k = std::str::from_utf8(a.key.as_ref())
                        .map_err(|_| bad("invalid attribute name"))?
                        .to_owned();
                    if !xml_name(&k) {
                        return Err(unsupported("prefixed XML attributes"));
                    }
                    let v = std::str::from_utf8(a.value.as_ref())
                        .map_err(|_| bad("attribute must be UTF-8"))?
                        .to_owned();
                    if v.contains('<') {
                        return Err(bad("unescaped XML attribute content"));
                    }
                    if k == "id" {
                        super::id(&v)?;
                        if !ids.insert(v.clone()) {
                            return Err(bad("duplicate document id"));
                        }
                    }
                    if attrs.insert(k, v).is_some() {
                        return Err(bad("duplicate attribute"));
                    }
                }
                let node = Node {
                    tag,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                if empty {
                    attach(node, &mut stack, &mut root)?;
                } else {
                    stack.push(node);
                }
            }
            Event::End(e) => {
                let node = stack.pop().ok_or_else(|| bad("unexpected XML close"))?;
                if node.tag.as_bytes() != e.name().as_ref() {
                    return Err(bad("mismatched XML close"));
                }
                attach(node, &mut stack, &mut root)?;
            }
            Event::Text(e) => {
                let text = std::str::from_utf8(e.as_ref()).map_err(|_| bad("invalid XML text"))?;
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(text);
                } else if !text.trim().is_empty() {
                    return Err(bad("text outside document"));
                }
            }
            Event::Decl(e) => {
                if declared || root.is_some() || !stack.is_empty() {
                    return Err(bad("misplaced XML declaration"));
                }
                declared = true;
                if e.version()
                    .map_err(|_| bad("invalid XML declaration"))?
                    .as_ref()
                    != b"1.0"
                {
                    return Err(unsupported("only XML 1.0 is supported"));
                }
                if let Some(enc) = e.encoding() {
                    if !enc
                        .map_err(|_| bad("invalid XML encoding"))?
                        .as_ref()
                        .eq_ignore_ascii_case(b"utf-8")
                    {
                        return Err(unsupported("only UTF-8 is supported"));
                    }
                }
            }
            Event::Comment(_) => {}
            Event::Eof => break,
            _ => {
                return Err(unsupported(
                    "DTD, entity, CDATA and processing instructions are unsupported",
                ))
            }
        }
    }
    if !stack.is_empty() {
        return Err(bad("unclosed XML element"));
    }
    root.ok_or_else(|| bad("missing document"))
}

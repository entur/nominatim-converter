use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// Read a whole element (the one whose `Start` event was just consumed) back
/// into an XML string, so it can be handed to `quick_xml::de::from_str`.
///
/// This is the project's workaround for a quick-xml limitation: `read_text`
/// doesn't work with `Reader<BufReader<File>>`, so large documents are walked
/// event-by-event and interesting elements are re-serialized for serde.
pub(crate) fn read_element_as_string(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut inner = Vec::new();
    append_tag(&mut inner, start, b">")?;
    let mut depth = 1u32;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                append_tag(&mut inner, e, b">")?;
                depth += 1;
            }
            Ok(Event::End(ref e)) => {
                depth -= 1;
                inner.extend_from_slice(b"</");
                inner.extend_from_slice(e.name().as_ref());
                inner.push(b'>');
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Empty(ref e)) => {
                append_tag(&mut inner, e, b"/>")?;
            }
            Ok(Event::Text(ref e)) => {
                inner.extend_from_slice(e.as_ref());
            }
            Ok(Event::CData(ref e)) => {
                inner.extend_from_slice(b"<![CDATA[");
                inner.extend_from_slice(e.as_ref());
                inner.extend_from_slice(b"]]>");
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }
    Ok(String::from_utf8(inner)?)
}

/// Append `<name attr="value" ...` plus `closer` (`>` for start tags,
/// `/>` for empty tags) to `inner`.
fn append_tag(
    inner: &mut Vec<u8>,
    e: &BytesStart,
    closer: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    inner.push(b'<');
    inner.extend_from_slice(e.name().as_ref());
    for attr in e.attributes() {
        let attr = attr?;
        inner.push(b' ');
        inner.extend_from_slice(attr.key.as_ref());
        inner.extend_from_slice(b"=\"");
        inner.extend_from_slice(&attr.value);
        inner.push(b'"');
    }
    inner.extend_from_slice(closer);
    Ok(())
}
